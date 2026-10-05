package spake2

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"os"
	"os/exec"
	"strings"
	"testing"

	"filippo.io/edwards25519"
)

var idA, idB = []byte("myous-pair-a"), []byte("myous-pair-b")

func secret() []byte {
	b := make([]byte, 64)
	rand.Read(b)
	return b
}

func pair(t *testing.T, pwA, pwB string) (*State, *State) {
	a, err := New(SideA, []byte(pwA), idA, idB, secret())
	if err != nil {
		t.Fatal(err)
	}
	b, err := New(SideB, []byte(pwB), idA, idB, secret())
	if err != nil {
		t.Fatal(err)
	}
	return a, b
}

func TestRoundTrip(t *testing.T) {
	for i := 0; i < 50; i++ {
		a, b := pair(t, "4821-K7F3QX", "4821-K7F3QX")
		kA, errA := a.Finish(b.Message())
		kB, errB := b.Finish(a.Message())
		if errA != nil || errB != nil || !bytes.Equal(kA, kB) || len(kA) != 32 {
			t.Fatalf("keys differ: %x %x (%v %v)", kA, kB, errA, errB)
		}
	}
	a, b := pair(t, "4821-K7F3QX", "4821-K7F3QY")
	kA, _ := a.Finish(b.Message())
	kB, _ := b.Finish(a.Message())
	if bytes.Equal(kA, kB) {
		t.Fatal("different passwords gave the same key")
	}
}

func TestDeterministic(t *testing.T) {
	s := secret()
	a1, _ := New(SideA, []byte("pw"), idA, idB, s)
	a2, _ := New(SideA, []byte("pw"), idA, idB, s)
	if !bytes.Equal(a1.Message(), a2.Message()) || len(a1.Message()) != MessageSize {
		t.Fatal("same secret must give the same 33-byte message")
	}
}

func TestRejectsBadMessages(t *testing.T) {
	a, b := pair(t, "pw", "pw")
	good := b.Message()
	identity := edwards25519.NewIdentityPoint().Bytes()
	// (0, -1): the point of order 2.
	order2, _ := hex.DecodeString("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")
	// A valid point outside the prime-order subgroup: B + (0, -1).
	t2, _ := new(edwards25519.Point).SetBytes(order2)
	mixed := new(edwards25519.Point).Add(edwards25519.NewGeneratorPoint(), t2).Bytes()
	// y = 2 isn't the y coordinate of any curve point.
	offCurve := append([]byte{2}, make([]byte, 31)...)

	cases := []struct {
		name string
		msg  []byte
		want error
	}{
		{"empty", nil, ErrBadMessage},
		{"short", good[:32], ErrBadMessage},
		{"long", append(append([]byte{}, good...), 0), ErrBadMessage},
		{"unknown side", append([]byte{'S'}, good[1:]...), ErrBadMessage},
		{"same side", append([]byte{'A'}, good[1:]...), ErrBadSide},
		{"identity", append([]byte{'B'}, identity...), ErrBadPoint},
		{"small order", append([]byte{'B'}, order2...), ErrBadPoint},
		{"wrong subgroup", append([]byte{'B'}, mixed...), ErrBadPoint},
		{"not on curve", append([]byte{'B'}, offCurve...), ErrBadPoint},
		{"reflected", append([]byte{'B'}, a.Message()[1:]...), ErrReflected},
	}
	for _, c := range cases {
		if _, err := a.Finish(c.msg); !errors.Is(err, c.want) {
			t.Errorf("%s: got %v, want %v", c.name, err, c.want)
		}
	}
	if _, err := a.Finish(good); err != nil {
		t.Errorf("good message rejected: %v", err)
	}
}

// python-spake2 is the reference. Set MYOUS_PYTHON to a Python with spake2
// installed (default /tmp/myous-venv/bin/python); skipped otherwise.
func python(t *testing.T) string {
	py := os.Getenv("MYOUS_PYTHON")
	if py == "" {
		py = "/tmp/myous-venv/bin/python"
	}
	if exec.Command(py, "-c", "import spake2").Run() != nil {
		t.Skip("no Python with spake2 at", py)
	}
	return py
}

// The hard-coded M and N, and the password scalar, must match python-spake2.
func TestConstantsMatchPython(t *testing.T) {
	out, err := exec.Command(python(t), "-c", `
from spake2.parameters.ed25519 import ParamsEd25519 as P
g = P.group
print(P.M.to_bytes().hex(), P.N.to_bytes().hex())
for pw in [b"", b"4821-K7F3QX", b"x" * 100]:
    print(g.scalar_to_bytes(g.password_to_scalar(pw)).hex())
`).Output()
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	mn := strings.Fields(lines[0])
	if hex.EncodeToString(pointM.Bytes()) != mn[0] || hex.EncodeToString(pointN.Bytes()) != mn[1] {
		t.Fatalf("M/N differ from python-spake2: %s", lines[0])
	}
	for i, pw := range [][]byte{{}, []byte("4821-K7F3QX"), bytes.Repeat([]byte("x"), 100)} {
		if got := hex.EncodeToString(passwordScalar(pw).Bytes()); got != lines[i+1] {
			t.Errorf("password scalar for %q: %s, python %s", pw, got, lines[i+1])
		}
	}
}
