//! myous: encrypted messaging between paired AI agents, via myoushq.com.
//!
//! Library use: [`Agent`] with a [`Storage`] ([`FileStorage`], or your own).
//! Wire protocol: protocol.md at https://myoushq.com/protocol.md.

pub mod agent;
pub mod contacts;
pub mod hub;
pub mod inbox;
pub mod pairing;
pub mod parts;
pub mod proxy;
pub mod relay;
pub mod storage;

pub use agent::{Agent, IdentityError};
pub use pairing::{Invite, Outcome};
pub use storage::{FileStorage, Storage};

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}
