//! JavaScript bindings for the pairing PAKE. Stateless: the caller keeps a
//! random 32-byte seed per pairing and passes it to both calls.
//!
//!   const msg = pakeStart("a", "4821-K7F3QX", seed);      // send to peer
//!   const key = pakeFinish("a", "4821-K7F3QX", seed, peerMsg); // 32 bytes

use myous_pake::{finish, start, Role};
use wasm_bindgen::prelude::*;

fn args(role: &str, seed: &[u8]) -> Result<(Role, [u8; 32]), JsError> {
    let role = Role::parse(role).ok_or_else(|| JsError::new("role must be \"a\" or \"b\""))?;
    let seed: [u8; 32] = seed.try_into().map_err(|_| JsError::new("seed must be 32 bytes"))?;
    Ok((role, seed))
}

/// Our outbound PAKE message.
#[wasm_bindgen(js_name = pakeStart)]
pub fn pake_start(role: &str, code: &str, seed: &[u8]) -> Result<Vec<u8>, JsError> {
    let (role, seed) = args(role, seed)?;
    Ok(start(role, code, seed))
}

/// The 32-byte shared key, given the peer's PAKE message.
#[wasm_bindgen(js_name = pakeFinish)]
pub fn pake_finish(role: &str, code: &str, seed: &[u8], peer_message: &[u8]) -> Result<Vec<u8>, JsError> {
    let (role, seed) = args(role, seed)?;
    finish(role, code, seed, peer_message).map_err(|e| JsError::new(&e.to_string()))
}
