package myous

import (
	"fmt"
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
