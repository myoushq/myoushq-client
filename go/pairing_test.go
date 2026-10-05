package myous

import (
	"bufio"
	"bytes"
	"encoding/hex"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

// clientRoot is the root of the myoushq-client repository.
func clientRoot(t *testing.T) string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Join(filepath.Dir(file), "..")
}

// The published test vectors (docs/test-vectors.json) must match.
func TestVectors(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join(clientRoot(t), "docs", "test-vectors.json"))
	if err != nil {
		t.Fatal(err)
	}
	var v struct {
		CodeParsing []struct {
			Input, Nameplate, Secret, Password string
		} `json:"code_parsing"`
		KeyDerivation struct {
			K          string `json:"K_hex"`
			FromA      string `json:"from_a_hex"`
			FromB      string `json:"from_b_hex"`
			VerifyHex  string `json:"verify_bytes_hex"`
			VerifyCode string `json:"verify_code"`
		} `json:"key_derivation"`
		PayloadSeal struct {
			K         string   `json:"K_hex"`
			Role      string   `json:"role"`
			Nameplate string   `json:"nameplate"`
			Nonce     string   `json:"nonce_hex"`
			Plaintext string   `json:"plaintext"`
			Message   Envelope `json:"message"`
		} `json:"payload_seal"`
	}
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}

	for _, c := range v.CodeParsing {
		np, secret, err := ParseCode(c.Input, "https://myoushq.com/p/")
		if err != nil || np != c.Nameplate || secret != c.Secret || FormatCode(np, secret) != c.Password {
			t.Errorf("ParseCode(%q) = %q, %q, %v; want %q, %q", c.Input, np, secret, err, c.Nameplate, c.Secret)
		}
	}

	kd := v.KeyDerivation
	k := mustHex(t, kd.K)
	for info, want := range map[string]string{"from a": kd.FromA, "from b": kd.FromB} {
		if got := hex.EncodeToString(Derive(k, info, 32)); got != want {
			t.Errorf("Derive(%q) = %s, want %s", info, got, want)
		}
	}
	if got := hex.EncodeToString(Derive(k, "verify", 8)); got != kd.VerifyHex {
		t.Errorf("verify bytes = %s, want %s", got, kd.VerifyHex)
	}
	if got := VerifyCode(k); got != kd.VerifyCode {
		t.Errorf("VerifyCode = %s, want %s", got, kd.VerifyCode)
	}

	ps := v.PayloadSeal
	k = mustHex(t, ps.K)
	env, err := Seal(k, ps.Role, ps.Nameplate, []byte(ps.Plaintext), mustHex(t, ps.Nonce))
	if err != nil {
		t.Fatal(err)
	}
	if env != ps.Message {
		t.Errorf("Seal = %+v, want %+v", env, ps.Message)
	}
	plain, err := Open(k, ps.Role, ps.Nameplate, env)
	if err != nil || string(plain) != ps.Plaintext {
		t.Errorf("Open = %q, %v", plain, err)
	}
	if _, err := Open(k, ps.Role, "9999", env); err == nil {
		t.Error("Open with the wrong nameplate should fail")
	}
}

func TestParseCodeRejects(t *testing.T) {
	for _, bad := range []string{"K7F3QX", "4821-K7F3Q", "4821-K7F3QXX", "4821-K7F3QU", "https://evil.example/p/4821#K7F3QX", "https://myoushq.com/p/4821"} {
		if _, _, err := ParseCode(bad, "https://myoushq.com/p/"); err == nil {
			t.Errorf("ParseCode(%q) should fail", bad)
		}
	}
}

func TestPakeGo(t *testing.T) {
	code := "4821-K7F3QX"
	seedA, seedB := bytes.Repeat([]byte{1}, 32), bytes.Repeat([]byte{2}, 32)
	msgA, msgB := pakeStart("a", code, seedA), pakeStart("b", code, seedB)
	if !bytes.Equal(msgA, pakeStart("a", code, seedA)) {
		t.Fatal("the same seed must give the same message")
	}
	kA, err1 := pakeFinish("a", code, seedA, msgB)
	kB, err2 := pakeFinish("b", code, seedB, msgA)
	if err1 != nil || err2 != nil || !bytes.Equal(kA, kB) || len(kA) != 32 {
		t.Fatalf("keys differ: %x %x %v %v", kA, kB, err1, err2)
	}
	kWrong, _ := pakeFinish("b", "4821-K7F3QY", seedB, msgA)
	if bytes.Equal(kA, kWrong) {
		t.Fatal("a wrong code must give a different key")
	}
}

// pythonSide runs one side of SPAKE2 with python-spake2, the reference.
const pythonSide = `
import sys
from spake2 import SPAKE2_A, SPAKE2_B
role, code = sys.argv[1], sys.argv[2]
cls = SPAKE2_A if role == "a" else SPAKE2_B
s = cls(code.encode(), idA=b"myous-pair-a", idB=b"myous-pair-b")
print(s.start().hex(), flush=True)
peer = bytes.fromhex(sys.stdin.readline().strip())
print(s.finish(peer).hex(), flush=True)
`

// Our SPAKE2 must agree with python-spake2. Set MYOUS_PYTHON to a Python with
// spake2 installed (default /tmp/myous-venv/bin/python); skipped otherwise.
func TestPakeInteropPython(t *testing.T) {
	python := os.Getenv("MYOUS_PYTHON")
	if python == "" {
		python = "/tmp/myous-venv/bin/python"
	}
	if exec.Command(python, "-c", "import spake2").Run() != nil {
		t.Skip("no Python with spake2 at", python)
	}
	code := "4821-K7F3QX"
	for _, goRole := range []string{"a", "b"} {
		pyRole := map[string]string{"a": "b", "b": "a"}[goRole]
		cmd := exec.Command(python, "-c", pythonSide, pyRole, code)
		stdin, _ := cmd.StdinPipe()
		stdout, _ := cmd.StdoutPipe()
		if err := cmd.Start(); err != nil {
			t.Fatal(err)
		}
		out := bufio.NewReader(stdout)
		line, _ := out.ReadString('\n')
		pyMsg := mustHex(t, strings.TrimSpace(line))

		seed := bytes.Repeat([]byte{7}, 32)
		goMsg := pakeStart(goRole, code, seed)
		stdin.Write([]byte(hex.EncodeToString(goMsg) + "\n"))
		line, _ = out.ReadString('\n')
		pyKey := mustHex(t, strings.TrimSpace(line))
		cmd.Wait()

		goKey, err := pakeFinish(goRole, code, seed, pyMsg)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(goKey, pyKey) {
			t.Errorf("Go side %s and Python disagree: %x vs %x", goRole, goKey, pyKey)
		}
	}
}

func TestFileStorage(t *testing.T) {
	st, err := NewFileStorage(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if k, _ := st.LoadKey(); k != "" {
		t.Fatal("expected no key")
	}
	if err := st.SaveKey("nsec1abc"); err != nil {
		t.Fatal(err)
	}
	if err := st.SaveKey("nsec1other"); err == nil {
		t.Fatal("SaveKey must not overwrite")
	}
	if err := st.Put("pending/4821", map[string]int{"a": 1}); err != nil {
		t.Fatal(err)
	}
	st.Put("pending/77", map[string]int{})
	names, _ := st.Names("pending/")
	if strings.Join(names, ",") != "pending/4821,pending/77" {
		t.Fatalf("Names = %v", names)
	}
	unlock, ok, _ := st.Lock("state", false)
	if !ok {
		t.Fatal("lock should be free")
	}
	if _, ok2, _ := st.Lock("state", false); ok2 {
		t.Fatal("lock should be taken")
	}
	unlock()
}

func mustHex(t *testing.T, s string) []byte {
	b, err := hex.DecodeString(s)
	if err != nil {
		t.Fatal(err)
	}
	return b
}
