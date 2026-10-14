//! The pairing PAKE (protocol.md §5.4): SPAKE2 over Ed25519, Magic Wormhole
//! flavor, with identities "myous-pair-a" and "myous-pair-b" and the
//! normalized pairing code as password.
//!
//! A pairing spans several runs of the agent (accept now, finish on a later
//! poll), but a SPAKE2 instance can't be serialized. So each side keeps a
//! random 32-byte seed instead and recreates the same instance from it:
//! `start` and `finish` with the same seed use the same secret scalar.
//! Keep the seed as secret as a private key until the pairing ends.

use curve25519_dalek::edwards::CompressedEdwardsY;
use curve25519_dalek::traits::IsIdentity;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use spake2::{Ed25519Group, Identity, Password, Spake2};

pub const ID_A: &[u8] = b"myous-pair-a";
pub const ID_B: &[u8] = b"myous-pair-b";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    A,
    B,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "a" => Some(Role::A),
            "b" => Some(Role::B),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct PakeError;

impl std::fmt::Display for PakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("bad PAKE message from the other side")
    }
}

impl std::error::Error for PakeError {}

fn instance(role: Role, code: &str, seed: [u8; 32]) -> (Spake2<Ed25519Group>, Vec<u8>) {
    let (password, id_a, id_b) = (Password::new(code.as_bytes()), Identity::new(ID_A), Identity::new(ID_B));
    let rng = ChaCha20Rng::from_seed(seed);
    match role {
        Role::A => Spake2::<Ed25519Group>::start_a_with_rng(&password, &id_a, &id_b, rng),
        Role::B => Spake2::<Ed25519Group>::start_b_with_rng(&password, &id_a, &id_b, rng),
    }
}

/// Our outbound PAKE message.
pub fn start(role: Role, code: &str, seed: [u8; 32]) -> Vec<u8> {
    instance(role, code, seed).1
}

/// The 32-byte shared key, given the peer's PAKE message.
pub fn finish(role: Role, code: &str, seed: [u8; 32], peer_message: &[u8]) -> Result<Vec<u8>, PakeError> {
    let (state, ours) = instance(role, code, seed);
    // A message is one side byte and a 32-byte point (protocol.md §5.4). The
    // crate checks the length and the side byte; we check the point.
    let point = peer_message.get(1..).ok_or(PakeError)?;
    check_peer_point(point, &ours[1..])?;
    state.finish(peer_message).map_err(|_| PakeError)
}

/// Accept the peer's point only where the other clients do. The `spake2`
/// crate only decompresses it (`Ed25519Group::bytes_to_element` in
/// spake2 0.4.0's src/ed25519.rs), so any point on the curve would do,
/// including the identity and points of small or mixed order, and it doesn't
/// look for its own point sent back.
///
/// - python-spake2 0.9 (`spake2/ed25519_basic.py`, `bytes_to_element`, and
///   `SPAKE2.finish` in `spake2/spake2.py`) rejects encodings that aren't on
///   the curve, the canonical encoding of the identity (it compares the
///   bytes, so it accepts the identity's non-canonical encodings, y = p+1
///   or the sign bit set on x = 0), points outside the prime-order subgroup
///   ([L]P isn't the identity), and our own point sent back to us.
/// - Go (`go/internal/spake2/spake2.go`, `decodeElement` and `Finish`)
///   rejects the same, but the identity in any encoding (it compares points).
///
/// We do what Go does: decompress (not on the curve: rejected), reject the
/// identity, reject a point that isn't torsion-free ([L]P isn't the
/// identity: small order, or order 2L, 4L or 8L), and reject a reflection.
/// Non-canonical encodings of other points (y >= p) pass, as in Python and
/// Go; the transcript hashes the bytes as received in all three.
fn check_peer_point(point: &[u8], ours: &[u8]) -> Result<(), PakeError> {
    let bytes: [u8; 32] = point.try_into().map_err(|_| PakeError)?;
    let p = CompressedEdwardsY(bytes).decompress().ok_or(PakeError)?;
    if p.is_identity() || !p.is_torsion_free() || p.compress().as_bytes() == ours {
        return Err(PakeError);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_code_same_key() {
        let (sa, sb) = ([1u8; 32], [2u8; 32]);
        let ma = start(Role::A, "4821-K7F3QX", sa);
        let mb = start(Role::B, "4821-K7F3QX", sb);
        // Recreated from the seed, as after a restart.
        let ka = finish(Role::A, "4821-K7F3QX", sa, &mb).unwrap();
        let kb = finish(Role::B, "4821-K7F3QX", sb, &ma).unwrap();
        assert_eq!(ka, kb);
        assert_eq!(ka.len(), 32);
        assert_eq!(start(Role::A, "4821-K7F3QX", sa), ma);
    }

    #[test]
    fn wrong_code_different_key() {
        let (sa, sb) = ([1u8; 32], [2u8; 32]);
        let ma = start(Role::A, "4821-K7F3QX", sa);
        let mb = start(Role::B, "4821-AAAAAA", sb);
        let ka = finish(Role::A, "4821-K7F3QX", sa, &mb).unwrap();
        let kb = finish(Role::B, "4821-AAAAAA", sb, &ma).unwrap();
        assert_ne!(ka, kb);
    }

    fn with_side(side: u8, point: &[u8]) -> Vec<u8> {
        let mut m = vec![side];
        m.extend_from_slice(point);
        m
    }

    fn hex32(h: &str) -> [u8; 32] {
        let mut b = [0u8; 32];
        for (i, out) in b.iter_mut().enumerate() {
            *out = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap();
        }
        b
    }

    /// Points the Go client rejects (python-spake2 too, except where noted).
    /// Of these, the `spake2` crate alone rejects only the one off the curve.
    fn bad_points() -> Vec<(String, [u8; 32])> {
        use curve25519_dalek::constants::{ED25519_BASEPOINT_POINT, EIGHT_TORSION};
        let mut bad = Vec::new();
        // All eight points of small order, the identity among them.
        for (i, t) in EIGHT_TORSION.iter().enumerate() {
            bad.push((format!("torsion {i}"), t.compress().to_bytes()));
        }
        // Points of order 2L, 4L or 8L: the base point plus a torsion point.
        for (i, t) in EIGHT_TORSION.iter().enumerate().skip(1) {
            bad.push((format!("base + torsion {i}"), (ED25519_BASEPOINT_POINT + t).compress().to_bytes()));
        }
        // The identity, non-canonically: y = p + 1, and x = 0 with its sign
        // bit set (python-spake2 accepts these two; Go and we don't).
        bad.push(("identity, y = p+1".into(), hex32("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")));
        bad.push(("identity, -0".into(), hex32("0100000000000000000000000000000000000000000000000000000000000080")));
        // y = 2 isn't the y coordinate of any curve point.
        let mut off = [0u8; 32];
        off[0] = 2;
        bad.push(("not on the curve".into(), off));
        bad
    }

    #[test]
    fn rejects_points_outside_the_prime_order_subgroup() {
        let (sa, sb) = ([1u8; 32], [2u8; 32]);
        for (name, point) in bad_points() {
            assert!(finish(Role::A, "4821-K7F3QX", sa, &with_side(b'B', &point)).is_err(), "A accepted {name}");
            assert!(finish(Role::B, "4821-K7F3QX", sb, &with_side(b'A', &point)).is_err(), "B accepted {name}");
        }
        // A good message still works after the rejections (same seed).
        let mb = start(Role::B, "4821-K7F3QX", sb);
        assert!(finish(Role::A, "4821-K7F3QX", sa, &mb).is_ok());
    }

    #[test]
    fn the_crate_alone_accepts_them() {
        // Why the check is ours: spake2 0.4's finish takes any point on the
        // curve. If this starts failing, the crate checks now too.
        let (state, _) = instance(Role::A, "4821-K7F3QX", [1u8; 32]);
        let order2 = hex32("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f");
        assert!(state.finish(&with_side(b'B', &order2)).is_ok());
        let (state, _) = instance(Role::A, "4821-K7F3QX", [1u8; 32]);
        assert!(state.finish(&with_side(b'B', &hex32(&format!("01{}", "00".repeat(31))))).is_ok());
    }

    #[test]
    fn rejects_bad_messages() {
        let sa = [1u8; 32];
        let ma = start(Role::A, "4821-K7F3QX", sa);
        let mb = start(Role::B, "4821-K7F3QX", [2u8; 32]);
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", vec![]),
            ("side byte only", vec![b'B']),
            ("short", mb[..32].to_vec()),
            ("long", [&mb[..], &[0]].concat()),
            ("unknown side", with_side(b'S', &mb[1..])),
            ("same side", with_side(b'A', &mb[1..])),
            ("reflected", with_side(b'B', &ma[1..])),
        ];
        for (name, msg) in cases {
            assert!(finish(Role::A, "4821-K7F3QX", sa, &msg).is_err(), "accepted {name}");
        }
    }
}
