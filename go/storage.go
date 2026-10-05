package myous

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
)

// Storage is where an agent keeps its myous data. FileStorage keeps it in
// one directory; agents without a persistent disk implement Storage on
// whatever they have (a secrets store for the key, a database for the rest).
//
// Must be durable: the key (losing it loses the identity) and the
// "contacts" document (losing it loses every pairing). Should be durable:
// "state", "settings" and the history. Can be lost: "hub" (cached config)
// and "pending/*" (pairings in progress, which expire in 15 minutes).
type Storage interface {
	// LoadKey returns the private key (nsec), or "" if there isn't one.
	LoadKey() (string, error)
	// SaveKey stores the private key. It must refuse to overwrite one.
	SaveKey(nsec string) error
	// Get decodes the JSON document called name into v. It reports
	// whether the document exists.
	Get(name string, v any) (bool, error)
	// Put replaces a document, atomically if possible.
	Put(name string, v any) error
	// Delete removes a document; no error if it doesn't exist.
	Delete(name string) error
	// Names lists documents whose names start with prefix ("pending/").
	Names(prefix string) ([]string, error)
	// AppendHistory adds one record to the message history.
	AppendHistory(e Entry) error
	// ReadHistory returns the whole history, oldest first.
	ReadHistory() ([]Entry, error)
}

// Locker is optional. Implement it if two runs of the same agent can
// overlap (a listener and a poll, or concurrent invocations). Lock returns
// ok=false instead of blocking when wait is false and the lock is taken.
type Locker interface {
	Lock(name string, wait bool) (unlock func(), ok bool, err error)
}

// FileStorage keeps everything in one directory: key, contacts.json,
// state.json, settings.json, hub.json, pending/*.json, messages.jsonl.
// The layout matches the Python client.
type FileStorage struct {
	Home string
}

// NewFileStorage uses dir, or $MYOUS_HOME, or ~/.myous.
func NewFileStorage(dir string) (*FileStorage, error) {
	if dir == "" {
		dir = os.Getenv("MYOUS_HOME")
	}
	if dir == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return nil, err
		}
		dir = filepath.Join(home, ".myous")
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, err
	}
	return &FileStorage{Home: dir}, nil
}

// Path returns the path of a file inside the data directory.
func (s *FileStorage) Path(name string) string {
	return filepath.Join(s.Home, filepath.FromSlash(name))
}

func (s *FileStorage) LoadKey() (string, error) {
	b, err := os.ReadFile(s.Path("key"))
	if errors.Is(err, os.ErrNotExist) {
		return "", nil
	}
	return strings.TrimSpace(string(b)), err
}

func (s *FileStorage) SaveKey(nsec string) error {
	// O_EXCL: never replace an existing key, even in a race.
	f, err := os.OpenFile(s.Path("key"), os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return err
	}
	if _, err := f.WriteString(nsec + "\n"); err != nil {
		f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return err
	}
	return f.Close()
}

func (s *FileStorage) Get(name string, v any) (bool, error) {
	b, err := os.ReadFile(s.Path(name + ".json"))
	if errors.Is(err, os.ErrNotExist) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return true, json.Unmarshal(b, v)
}

func (s *FileStorage) Put(name string, v any) error {
	b, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return err
	}
	return writePrivate(s.Path(name+".json"), append(b, '\n'))
}

func (s *FileStorage) Delete(name string) error {
	err := os.Remove(s.Path(name + ".json"))
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
}

func (s *FileStorage) Names(prefix string) ([]string, error) {
	dir, stem := "", prefix
	if i := strings.LastIndex(prefix, "/"); i >= 0 {
		dir, stem = prefix[:i], prefix[i+1:]
	}
	matches, err := filepath.Glob(filepath.Join(s.Path(dir), stem+"*.json"))
	if err != nil {
		return nil, err
	}
	var names []string
	for _, m := range matches {
		n := strings.TrimSuffix(filepath.Base(m), ".json")
		if dir != "" {
			n = dir + "/" + n
		}
		names = append(names, n)
	}
	sort.Strings(names)
	return names, nil
}

func (s *FileStorage) AppendHistory(e Entry) error {
	b, err := json.Marshal(e)
	if err != nil {
		return err
	}
	f, err := os.OpenFile(s.Path("messages.jsonl"), os.O_WRONLY|os.O_CREATE|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	if _, err := f.Write(append(b, '\n')); err != nil {
		f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return err
	}
	return f.Close()
}

func (s *FileStorage) ReadHistory() ([]Entry, error) {
	f, err := os.Open(s.Path("messages.jsonl"))
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	defer f.Close()
	var entries []Entry
	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 1024*1024), 16*1024*1024)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		var e Entry
		if err := json.Unmarshal([]byte(line), &e); err != nil {
			return nil, fmt.Errorf("messages.jsonl: %w", err)
		}
		entries = append(entries, e)
	}
	return entries, scanner.Err()
}

// Lock uses flock on files under locks/.
func (s *FileStorage) Lock(name string, wait bool) (func(), bool, error) {
	dir := s.Path("locks")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, false, err
	}
	f, err := os.OpenFile(filepath.Join(dir, strings.ReplaceAll(name, "/", "_")), os.O_WRONLY|os.O_CREATE, 0o600)
	if err != nil {
		return nil, false, err
	}
	how := syscall.LOCK_EX
	if !wait {
		how |= syscall.LOCK_NB
	}
	if err := syscall.Flock(int(f.Fd()), how); err != nil {
		f.Close()
		if errors.Is(err, syscall.EWOULDBLOCK) {
			return nil, false, nil
		}
		return nil, false, err
	}
	return func() {
		syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
		f.Close()
	}, true, nil
}

// writePrivate writes atomically, readable only by this user.
func writePrivate(path string, data []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	tmp := path + ".tmp"
	f, err := os.OpenFile(tmp, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o600)
	if err != nil {
		return err
	}
	if _, err := f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// lock takes a lock if the storage supports it.
func lock(st Storage, name string, wait bool) (func(), bool, error) {
	if l, ok := st.(Locker); ok {
		return l.Lock(name, wait)
	}
	return func() {}, true, nil
}
