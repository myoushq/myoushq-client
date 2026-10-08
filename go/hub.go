package myous

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// DefaultHub is used when the settings don't name another hub.
const DefaultHub = "https://myoushq.com"

const configMaxAge = 6 * 3600

// HubConfig is what GET /config.json returns, plus cache bookkeeping.
type HubConfig struct {
	Version       int      `json:"version"`
	Relays        []string `json:"relays"`
	PairAPI       string   `json:"pair_api"`
	PairLinkBase  string   `json:"pair_link_base"`
	PowDifficulty int      `json:"pow_difficulty"`
	BlobAPI       string   `json:"blob_api,omitempty"`
	LatestRelease string   `json:"latest_release,omitempty"`
	Notices       []Notice `json:"notices,omitempty"`
	URL           string   `json:"url,omitempty"`
	FetchedAt     int64    `json:"fetched_at,omitempty"`
}

// HubError is an error response from the hub's HTTP API.
type HubError struct {
	Status  int
	Message string
}

func (e *HubError) Error() string { return fmt.Sprintf("hub error %d: %s", e.Status, e.Message) }

// Hub talks to the hub over HTTPS: its config and the pairing mailbox.
type Hub struct {
	URL  string
	st   Storage
	http *http.Client
}

func newHub(st Storage, url string) *Hub {
	return &Hub{URL: strings.TrimRight(url, "/"), st: st, http: &http.Client{Timeout: 40 * time.Second}}
}

// Config returns the relay list and other settings, cached and refreshed
// every few hours. If the hub is unreachable, the cached copy is used.
func (h *Hub) Config(ctx context.Context, refresh bool) (*HubConfig, error) {
	var cached HubConfig
	ok, err := h.st.Get("hub", &cached)
	if err != nil {
		return nil, err
	}
	usable := ok && cached.URL == h.URL
	if usable && !refresh && time.Now().Unix()-cached.FetchedAt < configMaxAge {
		return &cached, nil
	}
	var cfg HubConfig
	if err := h.Request(ctx, "GET", "/config.json", nil, "", &cfg); err != nil {
		if usable {
			return &cached, nil
		}
		return nil, err
	}
	cfg.URL, cfg.FetchedAt = h.URL, time.Now().Unix()
	if err := h.st.Put("hub", cfg); err != nil {
		return nil, err
	}
	return &cfg, nil
}

// Request calls the hub's JSON API. body and out may be nil.
func (h *Hub) Request(ctx context.Context, method, endpoint string, body any, token string, out any) error {
	var rd io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return err
		}
		rd = bytes.NewReader(b)
	}
	req, err := http.NewRequestWithContext(ctx, method, h.URL+endpoint, rd)
	if err != nil {
		return err
	}
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	resp, err := h.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(resp.Body)
	if err != nil {
		return err
	}
	if resp.StatusCode >= 400 {
		var e struct {
			Error string `json:"error"`
		}
		if json.Unmarshal(raw, &e) != nil || e.Error == "" {
			e.Error = http.StatusText(resp.StatusCode)
		}
		return &HubError{Status: resp.StatusCode, Message: e.Error}
	}
	if out == nil || len(bytes.TrimSpace(raw)) == 0 {
		return nil
	}
	return json.Unmarshal(raw, out)
}

func hubStatus(err error) int {
	var he *HubError
	if errors.As(err, &he) {
		return he.Status
	}
	return 0
}
