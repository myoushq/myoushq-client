package myous

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
)

// Storage is where an agent keeps its myous data. FileStorage keeps it in
// one directory; agents without a persistent disk implement Storage on
// whatever they have (a secrets store for the key, a database for the rest).
//
// Must be durable: the key (losing it loses the identity) and the
// "contacts" document (losing it loses every pairing). Should be durable:
// "state", "settings" and the history. Can be lost: "hub" (cached config),
// "pending/*" (pairings in progress, which expire in 15 minutes) and
// "claiming/*" (the id of a claim in flight, until its pairing is saved).
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

// HistoryAppender is optional. Implement it if appending several entries
// at once is cheaper than one at a time (one write, one sync): a poll or a
// listener's backlog stores its messages with one call.
type HistoryAppender interface {
	AppendHistoryMany(entries []Entry) error
}

// appendHistory is AppendHistoryMany, or AppendHistory for each entry.
func appendHistory(st Storage, entries []Entry) error {
	if a, ok := st.(HistoryAppender); ok {
		return a.AppendHistoryMany(entries)
	}
	for _, e := range entries {
		if err := st.AppendHistory(e); err != nil {
			return err
		}
	}
	return nil
}

// Locker is optional. Implement it if two runs of the same agent can
// overlap (a listener and a poll, or concurrent invocations). Lock returns
// ok=false instead of blocking when wait is false and the lock is taken.
type Locker interface {
	Lock(name string, wait bool) (unlock func(), ok bool, err error)
}

// HistoryTail is optional. Implement it if reading the whole history is
// expensive (it only grows): ReadHistorySince returns the entries whose
// Seq is seq or more, oldest first, as ReadHistory filtered would. Entries
// are appended in Seq order (under the "state" lock), so the newest are
// at the end. Listen and Unread use it to look at what is new.
type HistoryTail interface {
	ReadHistorySince(seq int) ([]Entry, error)
}

// historySince is ReadHistorySince, or ReadHistory filtered for a storage
// without it.
func historySince(st Storage, seq int) ([]Entry, error) {
	if t, ok := st.(HistoryTail); ok {
		return t.ReadHistorySince(seq)
	}
	history, err := st.ReadHistory()
	if err != nil {
		return nil, err
	}
	var entries []Entry
	for _, e := range history {
		if e.Seq >= seq {
			entries = append(entries, e)
		}
	}
	return entries, nil
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
	makePrivate(dir)
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
	return s.AppendHistoryMany([]Entry{e})
}

// AppendHistoryMany appends entries with one write and one sync.
func (s *FileStorage) AppendHistoryMany(entries []Entry) error {
	var lines []byte
	for _, e := range entries {
		b, err := json.Marshal(e)
		if err != nil {
			return err
		}
		lines = append(append(lines, b...), '\n')
	}
	if len(lines) == 0 {
		return nil
	}
	if err := mendHistoryTail(s.Path("messages.jsonl")); err != nil {
		return err
	}
	f, err := os.OpenFile(s.Path("messages.jsonl"), os.O_WRONLY|os.O_CREATE|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	if _, err := f.Write(lines); err != nil {
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
	endsWithNewline := lastByteIs(f, '\n')
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
			if !endsWithNewline && !scanner.Scan() && scanner.Err() == nil && tornLine([]byte(line)) {
				break // the last line, a write cut short: the next AppendHistory mends it
			}
			return nil, fmt.Errorf("messages.jsonl: %w", err)
		}
		entries = append(entries, e)
	}
	return entries, scanner.Err()
}

// tornLine is whether what follows the history's last newline is a write
// cut short (a crash, a full disk): anything but blank or a whole JSON
// object. The Python client tells it the same way.
func tornLine(line []byte) bool {
	var obj map[string]json.RawMessage
	return len(bytes.TrimSpace(line)) > 0 && json.Unmarshal(line, &obj) != nil
}

// lastByteIs is whether f's last byte is b.
func lastByteIs(f *os.File, b byte) bool {
	info, err := f.Stat()
	if err != nil || info.Size() == 0 {
		return err == nil // an empty file ends no line
	}
	var last [1]byte
	_, err = f.ReadAt(last[:], info.Size()-1)
	return err == nil && last[0] == b
}

// mendHistoryTail runs before an append: if the history doesn't end with
// a newline (a write was cut short), it ends the last line, or drops that
// line if it isn't a whole entry, so the new entry gets a line of its own
// instead of being glued to the fragment (which would make every read of
// the history fail from then on). Callers hold the state lock: no other
// append is under way. As the Python client's _mend_tail.
func mendHistoryTail(path string) error {
	f, err := os.OpenFile(path, os.O_RDWR, 0)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	defer f.Close()
	if lastByteIs(f, '\n') {
		return nil
	}
	info, err := f.Stat()
	if err != nil {
		return err
	}
	// The last line: back from the end, a block at a time, to the newline
	// before it.
	size := info.Size()
	start, tail := size, []byte(nil)
	for start > 0 && bytes.IndexByte(tail, '\n') < 0 {
		step := min(int64(historyChunk), start)
		start -= step
		block := make([]byte, step, step+int64(len(tail)))
		if _, err := f.ReadAt(block, start); err != nil {
			return err
		}
		tail = append(block, tail...)
	}
	cut := int64(bytes.LastIndexByte(tail, '\n') + 1) // 0: the file is that one line
	if tornLine(tail[cut:]) {
		return f.Truncate(start + cut)
	}
	_, err = f.WriteAt([]byte{'\n'}, size)
	return err
}

// historyChunk is how much of messages.jsonl ReadHistorySince reads at a
// time, from the end.
var historyChunk = 64 << 10

// ReadHistorySince reads messages.jsonl backwards, from the end, and stops
// at the first entry older than seq: what is new costs the same however
// long the history has grown (ReadHistory decodes all of it). Lines are as
// ReadHistory takes them: blank ones skipped, CRLF endings (Python on
// Windows) and a last line without its newline taken too.
func (s *FileStorage) ReadHistorySince(seq int) ([]Entry, error) {
	f, err := os.Open(s.Path("messages.jsonl"))
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	defer f.Close()
	end, err := f.Seek(0, io.SeekEnd)
	if err != nil {
		return nil, err
	}
	var newest []Entry // newest first
	last := true       // the first line met is the last, which a write cut short leaves torn
	// take decodes one line; done is true once it is older than seq.
	take := func(line []byte) (done bool, err error) {
		wasLast := last
		last = false
		line = bytes.TrimSpace(line)
		if len(line) == 0 {
			return false, nil
		}
		var e Entry
		if err := json.Unmarshal(line, &e); err != nil {
			if wasLast && tornLine(line) {
				return false, nil // as ReadHistory: the next AppendHistory mends it
			}
			return false, fmt.Errorf("messages.jsonl: %w", err)
		}
		if e.Seq < seq {
			return true, nil
		}
		newest = append(newest, e)
		return false, nil
	}
	// carry is the start of the file's last unfinished line: the bytes
	// after the last newline seen so far, which the next chunk (further
	// back) completes.
	var carry []byte
	size := int64(historyChunk)
	for pos := end; pos > 0; {
		n := min(size, pos)
		pos -= n
		chunk := make([]byte, n, n+int64(len(carry)))
		if _, err := f.ReadAt(chunk, pos); err != nil {
			return nil, err
		}
		buf := append(chunk, carry...)
		if bytes.IndexByte(chunk, '\n') < 0 {
			size *= 2 // a long line: read more at a time, so it isn't copied over and over
		}
		for {
			i := bytes.LastIndexByte(buf, '\n')
			if i < 0 {
				break
			}
			done, err := take(buf[i+1:])
			if err != nil {
				return nil, err
			}
			if done {
				slices.Reverse(newest)
				return newest, nil
			}
			buf = buf[:i]
		}
		carry = buf
	}
	if _, err := take(carry); err != nil { // the first line
		return nil, err
	}
	slices.Reverse(newest)
	return newest, nil
}

// Lock takes an exclusive lock on a file under locks/ (flock; on Windows,
// LockFileEx).
func (s *FileStorage) Lock(name string, wait bool) (func(), bool, error) {
	dir := s.Path("locks")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, false, err
	}
	f, err := os.OpenFile(filepath.Join(dir, strings.ReplaceAll(name, "/", "_")), os.O_WRONLY|os.O_CREATE, 0o600)
	if err != nil {
		return nil, false, err
	}
	got, err := lockFile(f, wait)
	if err != nil || !got {
		f.Close()
		return nil, false, err
	}
	return func() {
		unlockFile(f)
		f.Close()
	}, true, nil
}

// writePrivate writes atomically, readable only by this user. The
// temporary file is this write's own (name.json.<random>.tmp): two
// processes writing the same document at once (the hub's cached config,
// refreshed by whichever command finds it stale) must not share one, or
// one's rename fails and the other's content lands torn in the document.
func writePrivate(path string, data []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), filepath.Base(path)+".*.tmp") // 0600
	if err != nil {
		return err
	}
	tmp := f.Name()
	err = func() error {
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
	}()
	if err != nil {
		os.Remove(tmp)
	}
	return err
}

// lock takes a lock if the storage supports it.
func lock(st Storage, name string, wait bool) (func(), bool, error) {
	if l, ok := st.(Locker); ok {
		return l.Lock(name, wait)
	}
	return func() {}, true, nil
}
