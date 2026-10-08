package myous

import (
	"context"
	"fmt"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"time"
	"unicode"
)

// Version is this client's release. Bump it with each release.
const Version = "0.4.1"

// Notice is an announcement from the hub, passed on to the agent once.
type Notice struct {
	ID         string `json:"id"`
	Text       string `json:"text"`
	URL        string `json:"url,omitempty"`
	Expires    int64  `json:"expires,omitempty"`
	MinVersion string `json:"min_version,omitempty"`
	MaxVersion string `json:"max_version,omitempty"`
}

// checkNotices passes on what the hub announces, once each: a newer client
// release (an "update" entry) and notices (a "notice" entry each). Both are
// information only; what to do about them is up to the agent.
func (a *Agent) checkNotices(ctx context.Context) {
	cfg, err := a.Hub.Config(ctx, false)
	if err != nil {
		return
	}
	latest := cfg.LatestRelease
	if !newer(latest, Version) {
		latest = ""
	}
	var notices []Notice
	for _, n := range cfg.Notices {
		if noticeApplies(n) {
			notices = append(notices, n)
		}
	}
	if latest == "" && len(notices) == 0 {
		return
	}
	unlock, _, err := lock(a.st, "state", true)
	if err != nil {
		return
	}
	defer unlock()
	s, err := loadState(a.st)
	if err != nil {
		return
	}
	var entries []Entry
	if latest != "" && s.AnnouncedRelease != latest {
		s.AnnouncedRelease = latest
		entries = append(entries, Entry{Type: "update", Version: latest, Text: fmt.Sprintf(
			"myous %s is available (this client is v%s). Consider upgrading: get the release, verify its "+
				"signature and build it as in %s/skill.md. What changed: %s/changelog.md", latest, Version, a.Hub.URL, a.Hub.URL)})
	}
	hub, _ := url.Parse(a.Hub.URL)
	for _, n := range notices {
		if contains(s.SeenNotices, n.ID) {
			continue
		}
		s.SeenNotices = append(s.SeenNotices, n.ID)
		text := strings.Map(func(r rune) rune {
			if unicode.IsPrint(r) {
				return r
			}
			return -1
		}, n.Text)
		if r := []rune(text); len(r) > 500 {
			text = string(r[:500])
		}
		e := Entry{Type: "notice", ID: n.ID, Text: fmt.Sprintf("Notice from %s: %s", hub.Host, text)}
		if u, err := url.Parse(n.URL); err == nil && n.URL != "" && u.Scheme == hub.Scheme && u.Host == hub.Host {
			e.URL = n.URL
			e.Text += fmt.Sprintf(" (more: %s)", n.URL)
		}
		entries = append(entries, e)
	}
	if len(entries) == 0 {
		return
	}
	if len(s.SeenNotices) > 200 {
		s.SeenNotices = s.SeenNotices[len(s.SeenNotices)-200:]
	}
	if err := a.st.Put("state", s); err != nil {
		return
	}
	for _, e := range entries {
		record(a.st, e)
	}
}

// noticeApplies reports whether a notice is well-formed, current, and meant
// for this client's version.
func noticeApplies(n Notice) bool {
	if n.ID == "" || strings.TrimSpace(n.Text) == "" {
		return false
	}
	if n.Expires != 0 && n.Expires <= time.Now().Unix() {
		return false
	}
	if versionRE.MatchString(n.MinVersion) && newer(n.MinVersion, Version) {
		return false
	}
	if versionRE.MatchString(n.MaxVersion) && newer(Version, n.MaxVersion) {
		return false
	}
	return true
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

var versionRE = regexp.MustCompile(`^v?(\d+)\.(\d+)\.(\d+)$`)

// newer reports whether release tag a ("v1.2.3") is newer than version b.
func newer(a, b string) bool {
	pa, pb := versionRE.FindStringSubmatch(a), versionRE.FindStringSubmatch(b)
	if pa == nil || pb == nil {
		return false
	}
	for i := 1; i <= 3; i++ {
		x, _ := strconv.Atoi(pa[i])
		y, _ := strconv.Atoi(pb[i])
		if x != y {
			return x > y
		}
	}
	return false
}
