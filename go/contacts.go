package myous

import (
	"encoding/json"
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/nbd-wtf/go-nostr/nip19"
)

// Contact statuses. Only approved contacts can reach this agent. "pending"
// is reserved for future contact requests that the owner approves.
const (
	Approved = "approved"
	Blocked  = "blocked"
)

// Contact is a paired peer. The contacts document maps hex pubkey → Contact.
type Contact struct {
	Alias    string `json:"alias"`
	Npub     string `json:"npub"`
	Status   string `json:"status"`
	PairedAt int64  `json:"paired_at"`
	// Relationship context, kept only here: how the owner knows this contact
	// (one of Relationships) and what may be shared with it.
	Relationship string `json:"relationship,omitempty"`
	Sharing      string `json:"sharing,omitempty"`
	// AddedBy records who made the pairing when it wasn't the agent itself
	// ("owner": the owner, through the myous desktop app).
	AddedBy string `json:"added_by,omitempty"`
	// Card is the latest card the contact sent about itself (protocol.md
	// section 4); PeerKnows is what this agent last told the contact about
	// itself (the alias at pairing, then each card sent), so a change is
	// sent once. Alias is this agent's own label for the contact and never
	// follows a card.
	Card      *Card      `json:"card,omitempty"`
	PeerKnows *PeerKnows `json:"peer_knows,omitempty"`
}

// Card is what a contact says about itself: its current alias and its
// self-description in its owner's words, received at At.
type Card struct {
	Name  string `json:"name"`
	About string `json:"about"`
	At    int64  `json:"at"`
}

// PeerKnows is the alias and card a contact was last told.
type PeerKnows struct {
	Name  string `json:"name"`
	About string `json:"about"`
}

// Relationships are the accepted values of Contact.Relationship.
var Relationships = []string{"family", "friend", "colleague", "business", "service", "other"}

const (
	maxSharing = 500
	maxName    = 64  // an alias, as in the pairing payload
	maxAbout   = 500 // a card's self-description
)

// ContactContext is relationship context to record on a contact; empty
// fields are left as they are.
type ContactContext struct {
	Relationship string
	Sharing      string
	AddedBy      string
}

func (cc ContactContext) validate() error {
	if cc.Relationship != "" && !slices.Contains(Relationships, cc.Relationship) {
		return fmt.Errorf("relationship must be one of: %s", strings.Join(Relationships, ", "))
	}
	if len([]rune(cc.Sharing)) > maxSharing {
		return fmt.Errorf("sharing guidance is limited to %d characters", maxSharing)
	}
	return nil
}

func (cc ContactContext) apply(c *Contact) {
	if cc.Relationship != "" {
		c.Relationship = cc.Relationship
	}
	if s := strings.TrimSpace(cc.Sharing); s != "" {
		c.Sharing = s
	}
	if s := strings.TrimSpace(cc.AddedBy); s != "" {
		c.AddedBy = s
	}
}

// contextOf returns the relationship context of the contact with this npub,
// and what the contact says about itself (its card's about).
func contextOf(st Storage, npub string) (relationship, sharing, about string) {
	contacts, _ := loadContacts(st)
	for _, c := range contacts {
		if c.Npub == npub {
			if c.Card != nil {
				about = c.Card.About
			}
			return c.Relationship, c.Sharing, about
		}
	}
	return "", "", ""
}

// parseCard recognizes a card message (protocol.md section 4) in a text:
// a JSON object {"myous": "card", "name": ..., "about": ...}. Anything
// else, including a card with missing, mistyped or too long fields, is
// not a card.
func parseCard(text string) (name, about string, ok bool) {
	if !strings.HasPrefix(text, "{") {
		return "", "", false
	}
	var obj map[string]any
	if json.Unmarshal([]byte(text), &obj) != nil || obj["myous"] != "card" {
		return "", "", false
	}
	name, ok = obj["name"].(string)
	if !ok {
		return "", "", false
	}
	if v, present := obj["about"]; present {
		if about, ok = v.(string); !ok {
			return "", "", false
		}
	}
	name, about = strings.TrimSpace(name), strings.TrimSpace(about)
	if n := len([]rune(name)); n < 1 || n > maxName || len([]rune(about)) > maxAbout {
		return "", "", false
	}
	return name, about, true
}

// cardText is the JSON of this agent's card.
func cardText(name, about string) string {
	b, _ := json.Marshal(struct {
		Myous string `json:"myous"`
		Name  string `json:"name"`
		About string `json:"about"`
	}{"card", name, about})
	return string(b)
}

// receiveCard stores a contact's card and returns the contact and a line
// for the history saying what changed: a new or changed description, a
// new name (announced, never applied: the alias is ours), or both.
func receiveCard(st Storage, pubkey, name, about string, at int64) (Contact, string, error) {
	contacts, err := loadContacts(st)
	if err != nil {
		return Contact{}, "", err
	}
	c := contacts[pubkey]
	knownName, oldAbout := c.Alias, ""
	if c.Card != nil {
		knownName, oldAbout = c.Card.Name, c.Card.About
	}
	var bits []string
	if name != knownName {
		bits = append(bits, fmt.Sprintf(`now calls itself "%s"; you call it "%s" (keep that, or follow it: myous rename "%s" "%s")`,
			name, c.Alias, c.Alias, name))
	}
	if about != oldAbout {
		if about != "" {
			bits = append(bits, "describes itself: "+about)
		} else {
			bits = append(bits, "cleared its description")
		}
	}
	if len(bits) == 0 {
		bits = append(bits, "sent its card again, unchanged")
	}
	c.Card = &Card{Name: name, About: about, At: at}
	contacts[pubkey] = c
	return c, c.Alias + " " + strings.Join(bits, "; "), st.Put("contacts", contacts)
}

// peerKnows records what this agent has told a contact about itself.
func peerKnows(st Storage, pubkey, name, about string) error {
	contacts, err := loadContacts(st)
	if err != nil {
		return err
	}
	c, ok := contacts[pubkey]
	if !ok {
		return nil
	}
	c.PeerKnows = &PeerKnows{Name: name, About: about}
	contacts[pubkey] = c
	return st.Put("contacts", contacts)
}

// Callers hold the "state" lock around changes.

func loadContacts(st Storage) (map[string]Contact, error) {
	contacts := map[string]Contact{}
	_, err := st.Get("contacts", &contacts)
	return contacts, err
}

// addContact pins a peer as approved. Re-pairing with a known key keeps its alias.
func addContact(st Storage, pubkey, alias string) (Contact, error) {
	contacts, err := loadContacts(st)
	if err != nil {
		return Contact{}, err
	}
	c, ok := contacts[pubkey]
	if ok {
		c.Status = Approved
	} else {
		npub, err := nip19.EncodePublicKey(pubkey)
		if err != nil {
			return Contact{}, err
		}
		if alias == "" {
			alias = "peer"
		}
		c = Contact{Alias: uniqueAlias(contacts, alias), Npub: npub, Status: Approved, PairedAt: time.Now().Unix()}
	}
	contacts[pubkey] = c
	return c, st.Put("contacts", contacts)
}

// findContact looks a contact up by alias, npub or hex key.
func findContact(st Storage, name string) (string, Contact, error) {
	contacts, err := loadContacts(st)
	if err != nil {
		return "", Contact{}, err
	}
	for pk, c := range contacts {
		if name == c.Alias || name == c.Npub || name == pk {
			return pk, c, nil
		}
	}
	for pk, c := range contacts {
		if strings.EqualFold(name, c.Alias) {
			return pk, c, nil
		}
	}
	return "", Contact{}, fmt.Errorf("no contact named %q", name)
}

func updateContact(st Storage, name string, change func(*Contact)) (Contact, error) {
	pk, _, err := findContact(st, name)
	if err != nil {
		return Contact{}, err
	}
	contacts, err := loadContacts(st)
	if err != nil {
		return Contact{}, err
	}
	c := contacts[pk]
	change(&c)
	for other, oc := range contacts {
		if other != pk && oc.Alias == c.Alias {
			return Contact{}, fmt.Errorf("alias %q is already used", c.Alias)
		}
	}
	contacts[pk] = c
	return c, st.Put("contacts", contacts)
}

func approvedContact(st Storage, pubkey string) (*Contact, error) {
	contacts, err := loadContacts(st)
	if err != nil {
		return nil, err
	}
	if c, ok := contacts[pubkey]; ok && c.Status == Approved {
		return &c, nil
	}
	return nil, nil
}

func uniqueAlias(contacts map[string]Contact, alias string) string {
	taken := map[string]bool{}
	for _, c := range contacts {
		taken[c.Alias] = true
	}
	candidate := alias
	for n := 2; taken[candidate]; n++ {
		candidate = fmt.Sprintf("%s-%d", alias, n)
	}
	return candidate
}
