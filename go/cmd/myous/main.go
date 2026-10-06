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
	"os"
	"os/signal"
	"strings"
	"time"

	myous "github.com/myoushq/myoushq-client/go"
)

const usage = `usage: myous <command> [flags]

  init --alias NAME [--hub URL]   create this agent's identity (once) and register
  invite [--json] [--wait]        start a pairing: get a link and code to share
  accept CODE [--wait SECONDS]    join a pairing from a link or code (default wait 60)
  send NAME TEXT...               send a message (TEXT "-" reads stdin)
  inbox [--json] [--peek]         new messages and pairing results, marked read
  history [--with NAME] [--limit N] [--json]
  contacts [--json]
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
	with := fs.String("with", "", "only this contact")
	limit := fs.Int("limit", 50, "how many entries")
	waitFlag := fs.String("wait", "", "accept: seconds to wait; invite: stay until done")
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
		if *alias == "" {
			settings := map[string]any{}
			st.Get("settings", &settings)
			*alias, _ = settings["alias"].(string)
		}
		if *alias == "" {
			return errors.New("give this agent a friendly name: myous init --alias NAME")
		}
		created := !agent.HasIdentity()
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
		inv, err := agent.Invite(ctx)
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
		p, err := agent.Accept(ctx, pos[0], time.Duration(wait*float64(time.Second)))
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

	case "inbox":
		entries, err := agent.Unread(!*peek)
		if err != nil {
			return err
		}
		if *asJSON {
			return printJSON(entries)
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
			return printJSON(entries)
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
			fmt.Printf("%-20s %-9s %s\n", c.Alias, c.Status, c.Npub)
		}

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
			"data_dir": st.Home, "hub": agent.Hub.URL, "identity": npub, "alias": agent.Alias(),
			"registered": agent.IsRegistered(), "contacts": len(contacts),
			"pending_pairings": pending, "unread": len(unread),
		}
		if *asJSON {
			return printJSON(info)
		}
		for _, k := range []string{"data_dir", "hub", "identity", "alias", "registered", "contacts", "pending_pairings", "unread"} {
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
		case e.Type != "message":
			fmt.Printf("[%s] (%s) %s\n", when, e.Type, e.Text)
		case e.Direction == "out":
			fmt.Printf("[%s] me -> %s: %s\n", when, e.Alias, e.Text)
		default:
			fmt.Printf("[%s] %s: %s\n", when, e.Alias, e.Text)
		}
	}
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
