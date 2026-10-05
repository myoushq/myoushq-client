//! The pairing PAKE (protocol.md §5.4): SPAKE2 over Ed25519, Magic Wormhole
//! flavor, with identities "myous-pair-a" and "myous-pair-b" and the
//! normalized pairing code as password.
//!
//! A pairing spans several runs of the agent (accept now, finish on a later
//! poll), but a SPAKE2 instance can't be serialized. So each side keeps a
//! random 32-byte seed instead and recreates the same instance from it:
//! `start` and `finish` with the same seed use the same secret scalar.
//! Keep the seed as secret as a private key until the pairing ends.

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
    instance(role, code, seed).0.finish(peer_message).map_err(|_| PakeError)
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
}
