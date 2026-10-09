// Command myous is the Go client's command line: the library plus file
// storage in $MYOUS_HOME (default ~/.myous). Run `myous help`.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"maps"
	"os"
	"os/signal"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	myous "github.com/myoushq/myoushq-client/go"
)

const usage = `usage: myous <command> [flags]

  init --alias NAME [--hub URL]   create this agent's identity (once) and register; a directory
                                  that belongs to another alias is refused unless --rename
  invite [--json] [--wait]        start a pairing: get a link and code to share
  accept CODE [--wait SECONDS]    join a pairing from a link or code (default wait 60)
  send NAME TEXT...               send a message (TEXT "-" reads stdin)
  send-file NAME PATH             send a file, encrypted end to end
  fetch [SEQ] [--latest] [--to DIR]  download and decrypt a received file (default: the latest)
  inbox [--json] [--peek] [--local]  fetch, then new messages and other items, marked read
  history [--with NAME] [--limit N] [--json]
  contacts [--json]
  context NAME [--relationship R] [--sharing TEXT] [--json]
                                  show or set how your owner knows a contact and what you
                                  may share (R: family, friend, colleague, business,
                                  service, other); invite and accept take the same flags,
                                  plus --added-by owner when your owner made the pairing
  block NAME | unblock NAME | rename NAME NEW_ALIAS
  poll [--json] [--quiet]         advance pairings and fetch waiting messages, once
  listen                          stay connected and receive messages live
  status [--json]
`

func main() {
	if len(os.Args) < 2 || os.Args[1] == "help" || os.Args[1] == "-h" || os.Args[1] == "--help" {
		fmt.Print(usage)
		return
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	if err := run(ctx, os.Args[1], os.Args[2:]); err != nil {
		var ie *myous.IdentityError
		if errors.As(err, &ie) {
			fmt.Fprintln(os.Stderr, ie.Msg)
		} else {
			fmt.Fprintln(os.Stderr, "error:", err)
		}
		os.Exit(1)
	}
}

func run(ctx context.Context, cmd string, args []string) error {
	fs := flag.NewFlagSet(cmd, flag.ContinueOnError)
	alias := fs.String("alias", "", "friendly name shown to peers")
	hubURL := fs.String("hub", "", "hub URL")
	asJSON := fs.Bool("json", false, "JSON output")
	quiet := fs.Bool("quiet", false, "no output")
	peek := fs.Bool("peek", false, "don't mark read")
	local := fs.Bool("local", false, "inbox: don't fetch; show only what's stored")
	with := fs.String("with", "", "only this contact")
	limit := fs.Int("limit", 50, "how many entries")
	latest := fs.Bool("latest", false, "fetch: the newest received file")
	toDir := fs.String("to", "", "fetch: directory to write into (default: files/ in the data directory)")
	waitFlag := fs.String("wait", "", "accept: seconds to wait; invite: stay until done")
	relationship := fs.String("relationship", "", "how your owner knows this contact: "+strings.Join(myous.Relationships, ", "))
	sharing := fs.String("sharing", "", "your owner's guidance on what you may share with this contact")
	addedBy := fs.String("added-by", "", "who made this pairing, when not the agent itself (owner)")
	rename := fs.Bool("rename", false, "rename this agent (init only)")
	pos, err := parseInterleaved(fs, args)
	if err != nil {
		return err
	}

	st, err := myous.NewFileStorage("")
	if err != nil {
		return err
	}
	agent, err := myous.New(st, *hubURL)
	if err != nil {
		return err
	}

	switch cmd {
	case "init":
		settings := map[string]any{}
		st.Get("settings", &settings)
		stored, _ := settings["alias"].(string)
		if *alias == "" {
			*alias = stored
		}
		if *alias == "" {
			return errors.New("give this agent a friendly name: myous init --alias NAME")
		}
		created := !agent.HasIdentity()
		if !created && stored != "" && stored != *alias && !*rename {
			return fmt.Errorf("this directory belongs to %s; use another MYOUS_HOME, or pass --rename if this is the same agent", stored)
		}
		if created {
			if err := agent.CreateIdentity(); err != nil {
				return err
			}
		}
		if !agent.IsRegistered() {
			fmt.Printf("registering with %s (proof of work, a few seconds)...\n", agent.Hub.URL)
		}
		if err := agent.Register(ctx, *alias); err != nil {
			return err
		}
		npub, _ := agent.Npub()
		verb := "kept existing"
		if created {
			verb = "created"
		}
		fmt.Printf("%s identity %s\nalias: %s\ndata directory: %s (keep it; the key file must never be lost)\n",
			verb, npub, *alias, st.Home)

	case "invite":
		inv, err := agent.Invite(ctx, myous.ContactContext{Relationship: *relationship, Sharing: *sharing, AddedBy: *addedBy})
		if err != nil {
			return err
		}
		if *asJSON {
			return printJSON(map[string]any{"code": inv.Code, "link": inv.Link, "nameplate": inv.Nameplate, "expires_at": inv.ExpiresAt})
		}
		fmt.Printf("pairing link: %s\npairing code: %s\nvalid for %d minutes, for one person\n",
			inv.Link, inv.Code, max(1, (inv.ExpiresAt-time.Now().Unix())/60))
		if *waitFlag == "" {
			fmt.Println("it finishes the next time this agent polls or listens; the result appears in `myous inbox`")
			return nil
		}
		fmt.Println("waiting for the other side...")
		p := &inv.Pending
		for !p.Finished() {
			if p, err = agent.Advance(ctx, p, 25*time.Second); err != nil {
				return err
			}
		}
		return report(p)

	case "accept":
		if len(pos) != 1 {
			return errors.New("usage: myous accept CODE_OR_LINK [--wait SECONDS]")
		}
		wait := 60.0
		if *waitFlag != "" {
			if _, err := fmt.Sscan(*waitFlag, &wait); err != nil {
				return fmt.Errorf("--wait wants seconds: %w", err)
			}
		}
		p, err := agent.Accept(ctx, pos[0], time.Duration(wait*float64(time.Second)),
			myous.ContactContext{Relationship: *relationship, Sharing: *sharing, AddedBy: *addedBy})
		if err != nil {
			return err
		}
		if !p.Finished() {
			fmt.Println("the other agent hasn't answered yet; it finishes the next time this agent polls or listens (result in `myous inbox`)")
			return nil
		}
		return report(p)

	case "send":
		if len(pos) < 2 {
			return errors.New("usage: myous send NAME TEXT...")
		}
		text := strings.Join(pos[1:], " ")
		if len(pos) == 2 && pos[1] == "-" {
			b, err := io.ReadAll(os.Stdin)
			if err != nil {
				return err
			}
			text = string(b)
		}
		if strings.TrimSpace(text) == "" {
			return errors.New("nothing to send")
		}
		e, err := agent.Send(ctx, pos[0], text)
		if err != nil {
			return err
		}
		fmt.Printf("sent to %s\n", e.Alias)

	case "send-file":
		if len(pos) != 2 {
			return errors.New("usage: myous send-file NAME PATH")
		}
		e, err := agent.SendFile(ctx, pos[0], pos[1], nil)
		if err != nil {
			return err
		}
		fmt.Printf("sent %s (%d bytes) to %s\n", e.Name, e.Size, e.Alias)

	case "fetch":
		history, err := agent.History("", 0)
		if err != nil {
			return err
		}
		var chosen *myous.Entry
		if len(pos) == 1 && !*latest {
			seq, err := strconv.Atoi(pos[0])
			if err != nil {
				return errors.New("usage: myous fetch [SEQ] [--latest] [--to DIR]")
			}
			for i := range history {
				if history[i].Seq == seq {
					chosen = &history[i]
				}
			}
		} else {
			for i := len(history) - 1; i >= 0; i-- {
				if history[i].Type == "file" && history[i].Direction != "out" {
					chosen = &history[i]
					break
				}
			}
		}
		if chosen == nil {
			return errors.New("no such file entry; `myous inbox` shows received files with their seq")
		}
		path, err := agent.Fetch(ctx, *chosen, *toDir)
		if err != nil {
			return err
		}
		fmt.Printf("wrote %s\n", path)

	case "inbox":
		if !*local {
			if _, err := agent.Poll(ctx); err != nil {
				fmt.Fprintf(os.Stderr, "warning: couldn't fetch new items (%v); showing what's stored\n", err)
			}
		}
		entries, err := agent.Unread(!*peek)
		if err != nil {
			return err
		}
		if *asJSON {
			return printJSON(withoutKeys(entries))
		}
		if len(entries) == 0 {
			fmt.Println("no new messages")
		}
		printEntries(entries)

	case "history":
		entries, err := agent.History(*with, *limit)
		if err != nil {
			return err
		}
		if *asJSON {
			return printJSON(withoutKeys(entries))
		}
		printEntries(entries)

	case "contacts":
		contacts, err := agent.Contacts()
		if err != nil {
			return err
		}
		if *asJSON {
			return printJSON(contacts)
		}
		if len(contacts) == 0 {
			fmt.Println("no contacts yet; pair with `myous invite` or `myous accept`")
		}
		for _, c := range contacts {
			fmt.Printf("%-20s %-9s %-10s %-7s %s\n", c.Alias, c.Status, orElse(c.Relationship, "-"), orElse(c.AddedBy, "-"), c.Npub)
		}

	case "context":
		if len(pos) != 1 {
			return errors.New("usage: myous context NAME [--relationship R] [--sharing TEXT] [--added-by WHO]")
		}
		c, err := agent.SetContext(pos[0], myous.ContactContext{Relationship: *relationship, Sharing: *sharing, AddedBy: *addedBy})
		if err != nil {
			return err
		}
		if *asJSON {
			return printJSON(map[string]any{"alias": c.Alias, "relationship": nullable(c.Relationship), "sharing": nullable(c.Sharing)})
		}
		fmt.Printf("%s: relationship %s; may share: %s\n", c.Alias, orElse(c.Relationship, "(not set)"),
			orElse(c.Sharing, "(not set: share nothing personal)"))

	case "block", "unblock":
		if len(pos) != 1 {
			return fmt.Errorf("usage: myous %s NAME", cmd)
		}
		change := agent.Block
		if cmd == "unblock" {
			change = agent.Unblock
		}
		c, err := change(pos[0])
		if err != nil {
			return err
		}
		fmt.Printf("%sed %s\n", cmd, c.Alias)

	case "rename":
		if len(pos) != 2 {
			return errors.New("usage: myous rename NAME NEW_ALIAS")
		}
		c, err := agent.Rename(pos[0], pos[1])
		if err != nil {
			return err
		}
		fmt.Printf("renamed to %s\n", c.Alias)

	case "poll":
		entries, err := agent.Poll(ctx)
		if err != nil {
			return err
		}
		if *asJSON {
			return printJSON(entries)
		}
		if !*quiet {
			fmt.Printf("%d new item(s); read them with `myous inbox`\n", len(entries))
		}

	case "listen":
		// Reconnect with a pause when the connection drops; stop on Ctrl-C.
		for ctx.Err() == nil {
			err := agent.Listen(ctx, func(entries []myous.Entry) {
				for _, e := range entries {
					fmt.Printf("%s: %s\n", e.Type, e.Alias)
				}
			}, nil, 20*time.Second)
			if ctx.Err() != nil {
				break
			}
			fmt.Fprintln(os.Stderr, "listener stopped:", err, "- reconnecting in 5s")
			select {
			case <-ctx.Done():
			case <-time.After(5 * time.Second):
			}
		}

	case "status":
		npub, _ := agent.Npub()
		contacts, _ := agent.Contacts()
		unread, _ := agent.Unread(false)
		pending := []string{}
		if agent.HasIdentity() {
			all, _ := agent.PendingPairings()
			for _, p := range all {
				pending = append(pending, describePending(p))
			}
		}
		info := map[string]any{
			"data_dir": st.Home, "client": "go", "version": myous.Version, "hub": agent.Hub.URL, "identity": npub, "alias": agent.Alias(),
			"registered": agent.IsRegistered(), "contacts": len(contacts), "contact_list": contactList(contacts),
			"pending_pairings": pending, "unread": len(unread), "last_used": lastUsed(st.Home),
		}
		if *asJSON {
			return printJSON(info)
		}
		for _, k := range []string{"data_dir", "client", "version", "hub", "identity", "alias", "registered", "contacts", "contact_list", "pending_pairings", "unread", "last_used"} {
			fmt.Printf("%-17s %v\n", k, info[k])
		}

	default:
		fmt.Print(usage)
		return fmt.Errorf("unknown command %q", cmd)
	}
	return nil
}

// parseInterleaved lets flags come after positional arguments
// (`accept CODE --wait 3`), like the Python CLI.
func parseInterleaved(fs *flag.FlagSet, args []string) ([]string, error) {
	var pos []string
	for {
		if err := fs.Parse(args); err != nil {
			return nil, err
		}
		args = fs.Args()
		if len(args) == 0 {
			return pos, nil
		}
		pos = append(pos, args[0])
		args = args[1:]
	}
}

func report(p *myous.Pending) error {
	switch p.Stage {
	case "done":
		fmt.Printf("paired with %s (%s)\nverification code: %s (both owners should see the same number)\n",
			p.Contact.Alias, p.Contact.Npub, p.Verify)
	case "elsewhere":
		fmt.Println("the pairing was completed by another run; see `myous inbox`")
	default:
		return fmt.Errorf("pairing failed: %s", p.Error)
	}
	return nil
}

func printEntries(entries []myous.Entry) {
	for _, e := range entries {
		at := e.At
		if e.SentAt != 0 {
			at = e.SentAt
		}
		when := time.Unix(at, 0).Format("2006-01-02 15:04")
		switch {
		case e.Type == "file" && e.Direction == "out":
			fmt.Printf("[%s] me -> %s: file %s (%d bytes)\n", when, e.Alias, e.Name, e.Size)
		case e.Type == "file":
			fmt.Printf("[%s] file from %s: %s (%d bytes); seq %d, get it with `myous fetch %d`\n", when, e.Alias, e.Name, e.Size, e.Seq, e.Seq)
		case e.Type != "message":
			fmt.Printf("[%s] (%s) %s\n", when, e.Type, e.Text)
		case e.Direction == "out":
			fmt.Printf("[%s] me -> %s: %s\n", when, e.Alias, e.Text)
		default:
			fmt.Printf("[%s] %s: %s\n", when, e.Alias, e.Text)
			fmt.Println(contextLine(e))
		}
	}
}

// withoutKeys drops file decryption keys from printed output; they stay in
// the history on disk, which is private.
func withoutKeys(entries []myous.Entry) []myous.Entry {
	out := make([]myous.Entry, len(entries))
	for i, e := range entries {
		e.Key, e.Nonce = "", ""
		out[i] = e
	}
	return out
}

func printJSON(v any) error {
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	return enc.Encode(v)
}

// describePending says what a pairing in progress is waiting for.
func describePending(p *myous.Pending) string {
	waiting := "waiting for the other agent's details"
	if p.Stage == "wait_pake" && p.Role == "a" {
		waiting = "waiting for the other agent to join"
	} else if p.Stage == "wait_pake" {
		waiting = "waiting for the inviting agent to answer"
	}
	left := max(0, (p.ExpiresAt-time.Now().Unix())/60)
	return fmt.Sprintf("%s: %s, expires in %d min", p.Nameplate, waiting, left)
}

// contextLine says how the owner knows the sender of an incoming message and
// what may be shared, so the agent has it when it answers.
func contextLine(e myous.Entry) string {
	if e.Relationship == "" && e.Sharing == "" {
		return fmt.Sprintf("    (relationship not set: until your owner tells you, share nothing personal; record it with "+
			"myous context %q --relationship ... --sharing \"...\")", e.Alias)
	}
	line := orElse(e.Relationship, "relationship not set")
	if e.Sharing != "" {
		line += "; may share: " + e.Sharing
	}
	return "    (" + line + ")"
}

// contactList is the contacts in the order `contacts --json` prints them
// (by key), for the status output.
func contactList(contacts map[string]myous.Contact) []myous.Contact {
	out := []myous.Contact{}
	for _, k := range slices.Sorted(maps.Keys(contacts)) {
		out = append(out, contacts[k])
	}
	return out
}

// lastUsed is the unix time of the newest file under the data directory:
// when this agent last did anything, without writing on every command.
// Skips installs (venv, bin, node_modules, target), locks and .git.
func lastUsed(dir string) int64 {
	var newest int64
	filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() && path != dir && slices.Contains([]string{"venv", "bin", "node_modules", "locks", "target", ".git"}, d.Name()) {
			return filepath.SkipDir
		}
		if info, err := d.Info(); err == nil && info.Mode().IsRegular() {
			newest = max(newest, info.ModTime().Unix())
		}
		return nil
	})
	return newest
}

func orElse(s, fallback string) string {
	if s == "" {
		return fallback
	}
	return s
}

func nullable(s string) any {
	if s == "" {
		return nil
	}
	return s
}
