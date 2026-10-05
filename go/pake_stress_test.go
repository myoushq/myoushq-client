package myous

import (
	"bufio"
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

// pythonLoop runs many SPAKE2 exchanges, one per pair of input lines:
// "<role> <code>" then the peer's message; it prints its message, then the key.
const pythonLoop = `
import sys
from spake2 import SPAKE2_A, SPAKE2_B
for line in sys.stdin:
    role, code = line.split()
    cls = SPAKE2_A if role == "a" else SPAKE2_B
    s = cls(code.encode(), idA=b"myous-pair-a", idB=b"myous-pair-b")
    print(s.start().hex(), flush=True)
    peer = bytes.fromhex(sys.stdin.readline().strip())
    try:
        print(s.finish(peer).hex(), flush=True)
    except Exception as e:
        print("error", e, flush=True)
`

// Many Go <-> python-spake2 exchanges with random seeds and codes, to catch
// encoding edge cases that a single fixed seed misses.
// MYOUS_PAKE_STRESS=N sets the number of rounds per role (skipped if unset).
func TestPakeStressPython(t *testing.T) {
	rounds, _ := strconv.Atoi(os.Getenv("MYOUS_PAKE_STRESS"))
	if rounds == 0 {
		t.Skip("set MYOUS_PAKE_STRESS=N to run")
	}
	python := os.Getenv("MYOUS_PYTHON")
	if python == "" {
		python = "/tmp/myous-venv/bin/python"
	}
	cmd := exec.Command(python, "-c", pythonLoop)
	stdin, _ := cmd.StdinPipe()
	stdout, _ := cmd.StdoutPipe()
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	defer cmd.Wait()
	defer stdin.Close()
	out := bufio.NewReader(stdout)
	readHex := func() []byte {
		line, _ := out.ReadString('\n')
		b, err := hex.DecodeString(strings.TrimSpace(line))
		if err != nil {
			t.Fatalf("python said %q", line)
		}
		return b
	}

	failures := 0
	for _, goRole := range []string{"a", "b"} {
		pyRole := map[string]string{"a": "b", "b": "a"}[goRole]
		for i := 0; i < rounds; i++ {
			seed := make([]byte, 32)
			rand.Read(seed)
			code := fmt.Sprintf("%d-%s", 1000+i%9000, makeSecretForTest())
			fmt.Fprintf(stdin, "%s %s\n", pyRole, code)
			pyMsg := readHex()
			goMsg := pakeStart(goRole, code, seed)
			fmt.Fprintf(stdin, "%x\n", goMsg)
			pyKey := readHex()
			goKey, err := pakeFinish(goRole, code, seed, pyMsg)
			if err != nil || !bytes.Equal(goKey, pyKey) {
				failures++
				if failures <= 5 {
					t.Errorf("go=%s seed=%x code=%s: keys differ (err %v)\n go msg %x\n py msg %x",
						goRole, seed, code, err, goMsg, pyMsg)
				}
			}
		}
	}
	t.Logf("%d failures in %d exchanges", failures, 2*rounds)
}

func makeSecretForTest() string {
	b := make([]byte, 6)
	rand.Read(b)
	out := make([]byte, 6)
	for i, c := range b {
		out[i] = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"[c%32]
	}
	return string(out)
}

// The same against the Rust client's PAKE (the spake2 crate), through its
// example CLI: cargo build -p myous-pake --example pake in clients/rust.
// MYOUS_PAKE_STRESS=N sets the rounds per role; MYOUS_RUST_PAKE overrides
// the binary path. Skipped if either is missing.
func TestPakeStressRust(t *testing.T) {
	rounds, _ := strconv.Atoi(os.Getenv("MYOUS_PAKE_STRESS"))
	if rounds == 0 {
		t.Skip("set MYOUS_PAKE_STRESS=N to run")
	}
	bin := os.Getenv("MYOUS_RUST_PAKE")
	if bin == "" {
		bin, _ = filepath.Abs("../rust/target/debug/examples/pake")
	}
	if _, err := os.Stat(bin); err != nil {
		t.Skip("no Rust PAKE CLI at", bin)
	}
	rust := func(args ...string) []byte {
		out, err := exec.Command(bin, args...).Output()
		if err != nil {
			t.Fatalf("pake %v: %v", args, err)
		}
		b, _ := hex.DecodeString(strings.TrimSpace(string(out)))
		return b
	}

	failures := 0
	for _, goRole := range []string{"a", "b"} {
		rustRole := map[string]string{"a": "b", "b": "a"}[goRole]
		for i := 0; i < rounds; i++ {
			goSeed, rustSeed := make([]byte, 32), make([]byte, 32)
			rand.Read(goSeed)
			rand.Read(rustSeed)
			code := fmt.Sprintf("%d-%s", 1000+i%9000, makeSecretForTest())
			rustMsg := rust("start", rustRole, code, hex.EncodeToString(rustSeed))
			goMsg := pakeStart(goRole, code, goSeed)
			rustKey := rust("finish", rustRole, code, hex.EncodeToString(rustSeed), hex.EncodeToString(goMsg))
			goKey, err := pakeFinish(goRole, code, goSeed, rustMsg)
			if err != nil || !bytes.Equal(goKey, rustKey) {
				failures++
				if failures <= 5 {
					t.Errorf("go=%s code=%s: keys differ (err %v)", goRole, code, err)
				}
			}
		}
	}
	t.Logf("%d failures in %d exchanges", failures, 2*rounds)
}
