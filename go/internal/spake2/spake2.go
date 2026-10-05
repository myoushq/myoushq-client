// Package spake2 is Magic Wormhole's SPAKE2 over Ed25519, byte-for-byte
// compatible with python-spake2 0.9 (the reference) and the Rust spake2
// crate, in its asymmetric (side A / side B) form.
//
// It's built on filippo.io/edwards25519, so everything touching secrets
// (our scalar, the password scalar, the shared point) runs in constant time.
//
//	x  = our secret scalar
//	X* = x*B + pw*M   (side A; side B uses N)
//	K  = x*(Y* - pw*N) (side A; side B uses M)
//	key = SHA256(SHA256(pw) || SHA256(idA) || SHA256(idB) || X* || Y* || K)
//
// A message is the side byte ('A' or 'B') followed by the 32-byte point.
// The side byte isn't part of the transcript.
package spake2

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"io"

	"filippo.io/edwards25519"
	"golang.org/x/crypto/hkdf"
)

// Side identifies which half of the exchange we are.
type Side byte

const (
	SideA Side = 'A'
	SideB Side = 'B'
)

// MessageSize is the length of a message: side byte plus a 32-byte point.
const MessageSize = 1 + 32

var (
	ErrBadMessage = errors.New("spake2: malformed message")
	ErrBadSide    = errors.New("spake2: message is from the wrong side")
	ErrBadPoint   = errors.New("spake2: message is not a valid group element")
	ErrReflected  = errors.New("spake2: our own message was reflected back")
)

// M and N are python-spake2's arbitrary_element(b"M") and (b"N"): points
// nobody knows the discrete log of, derived by hashing those seeds. They're
// hard-coded here and checked against python-spake2 in the tests.
var (
	pointM = mustPoint("15cfd18e385952982b6a8f8c7854963b58e34388c8e6dae891db756481a02312")
	pointN = mustPoint("f04f2e7eb734b2a8f8b472eaf9c3c632576ac64aea650b496a8a20ff00e583c3")
)

// State is one side of an exchange. Make it with New, send Message(), then
// call Finish with the peer's message.
type State struct {
	side     Side
	password []byte
	idA, idB []byte
	pw       *edwards25519.Scalar // password scalar
	x        *edwards25519.Scalar // our secret scalar
	message  []byte               // our point, without the side byte
}

// New starts an exchange. secret must be 64 uniformly random bytes (or the
// output of a hash over a secret seed); the same secret gives the same
// State, which is how a pairing is resumed in a later run.
func New(side Side, password, idA, idB, secret []byte) (*State, error) {
	if side != SideA && side != SideB {
		return nil, errors.New("spake2: side must be A or B")
	}
	x, err := edwards25519.NewScalar().SetUniformBytes(secret)
	if err != nil {
		return nil, err
	}
	s := &State{side: side, password: password, idA: idA, idB: idB, pw: passwordScalar(password), x: x}
	// X* = x*B + pw*M (or N for side B)
	blind := new(edwards25519.Point).ScalarMult(s.pw, s.blinding())
	s.message = new(edwards25519.Point).Add(new(edwards25519.Point).ScalarBaseMult(x), blind).Bytes()
	return s, nil
}

// Message is what to send to the peer.
func (s *State) Message() []byte {
	return append([]byte{byte(s.side)}, s.message...)
}

// Finish checks the peer's message and returns the 32-byte shared key.
// Peers that used different passwords get different keys; that's detected
// later, when decryption with the key fails.
func (s *State) Finish(peer []byte) ([]byte, error) {
	if len(peer) != MessageSize {
		return nil, ErrBadMessage
	}
	other := Side(peer[0])
	if other != SideA && other != SideB {
		return nil, ErrBadMessage
	}
	if other == s.side {
		return nil, ErrBadSide
	}
	inbound, err := decodeElement(peer[1:])
	if err != nil {
		return nil, err
	}
	// python-spake2 compares the re-encoded point, so non-canonical
	// encodings of our own point count as reflections too.
	if bytes.Equal(inbound.Bytes(), s.message) {
		return nil, ErrReflected
	}

	// K = x * (Y* - pw*N)   (or X* - pw*M for side B)
	unblind := new(edwards25519.Point).ScalarMult(s.pw, s.unblinding())
	k := new(edwards25519.Point).ScalarMult(s.x, new(edwards25519.Point).Subtract(inbound, unblind))

	// The transcript uses the received bytes as sent (python-spake2 keeps
	// the raw inbound message, not a re-encoding).
	xMsg, yMsg := s.message, peer[1:]
	if s.side == SideB {
		xMsg, yMsg = peer[1:], s.message
	}
	h := sha256.New()
	for _, part := range [][]byte{sum(s.password), sum(s.idA), sum(s.idB), xMsg, yMsg, k.Bytes()} {
		h.Write(part)
	}
	return h.Sum(nil), nil
}

func (s *State) blinding() *edwards25519.Point {
	if s.side == SideA {
		return pointM
	}
	return pointN
}

func (s *State) unblinding() *edwards25519.Point {
	if s.side == SideA {
		return pointN
	}
	return pointM
}

// decodeElement accepts exactly the points python-spake2 accepts: valid
// encodings of points in the prime-order subgroup, other than the identity.
func decodeElement(b []byte) (*edwards25519.Point, error) {
	p, err := new(edwards25519.Point).SetBytes(b)
	if err != nil {
		return nil, ErrBadPoint
	}
	if p.Equal(edwards25519.NewIdentityPoint()) == 1 {
		return nil, ErrBadPoint
	}
	if !inPrimeOrderSubgroup(p) {
		return nil, ErrBadPoint
	}
	return p, nil
}

// inPrimeOrderSubgroup reports whether [L]P is the identity. Any point is
// P = P_L + T with T of order dividing 8; (8^-1 mod L)*[8]P recovers P_L,
// which equals P exactly when T is the identity. P is public, so the cost
// of these operations doesn't leak anything.
func inPrimeOrderSubgroup(p *edwards25519.Point) bool {
	q := new(edwards25519.Point).ScalarMult(invEight, new(edwards25519.Point).MultByCofactor(p))
	return q.Equal(p) == 1
}

var invEight = edwards25519.NewScalar().Invert(must(edwards25519.NewScalar().SetUniformBytes(append([]byte{8}, make([]byte, 63)...))))

// passwordScalar is python-spake2's password_to_scalar: HKDF-SHA256 (empty
// salt, info "SPAKE2 pw") to 48 bytes, read as a big-endian integer, mod L.
func passwordScalar(password []byte) *edwards25519.Scalar {
	r := hkdf.New(sha256.New, password, []byte{}, []byte("SPAKE2 pw"))
	be := make([]byte, 48)
	if _, err := io.ReadFull(r, be); err != nil {
		panic(err)
	}
	le := make([]byte, 64) // SetUniformBytes wants 64 little-endian bytes
	for i, c := range be {
		le[len(be)-1-i] = c
	}
	return must(edwards25519.NewScalar().SetUniformBytes(le))
}

func sum(b []byte) []byte {
	h := sha256.Sum256(b)
	return h[:]
}

func mustPoint(h string) *edwards25519.Point {
	b, err := hex.DecodeString(h)
	if err != nil {
		panic(err)
	}
	p, err := decodeElement(b)
	if err != nil {
		panic("spake2: bad constant " + h)
	}
	return p
}

func must(s *edwards25519.Scalar, err error) *edwards25519.Scalar {
	if err != nil {
		panic(err)
	}
	return s
}
