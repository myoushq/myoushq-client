// SPAKE2 for pairing, from the Rust client compiled to WebAssembly
// (clients/rust/wasm). It's Magic Wormhole's SPAKE2 variant, which has no
// maintained JavaScript implementation, so we reuse the Rust one rather
// than port the curve arithmetic.
//
// Stateless by seed: the same (role, code, seed) always gives the same
// SPAKE2 instance, so a pairing can be finished in a later run by storing
// only the seed.

import { createRequire } from "node:module";

interface PakeWasm {
  pakeStart(role: string, code: string, seed: Uint8Array): Uint8Array;
  pakeFinish(role: string, code: string, seed: Uint8Array, peerMessage: Uint8Array): Uint8Array;
}

let wasm: PakeWasm | undefined;

function load(): PakeWasm {
  if (!wasm) {
    // The nodejs build of wasm-bindgen is a CommonJS module that loads the
    // .wasm file next to it synchronously.
    const require = createRequire(import.meta.url);
    wasm = require("../wasm/myous_pake_wasm.js") as PakeWasm;
  }
  return wasm;
}

export type Role = "a" | "b";

/** Our outbound PAKE message. */
export function pakeStart(role: Role, code: string, seed: Uint8Array): Uint8Array {
  return load().pakeStart(role, code, seed);
}

/** The 32-byte shared key, given the peer's PAKE message. Throws on a malformed message. */
export function pakeFinish(role: Role, code: string, seed: Uint8Array, peerMessage: Uint8Array): Uint8Array {
  return load().pakeFinish(role, code, seed, peerMessage);
}
