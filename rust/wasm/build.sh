#!/bin/sh
# Build the pairing PAKE as WebAssembly for JavaScript.
# Needs: rustup target add wasm32-unknown-unknown
#        cargo install --locked wasm-bindgen-cli --version 0.2.129  (must match Cargo.toml)
set -eu
cd "$(dirname "$0")"
cargo build -p myous-pake-wasm --target wasm32-unknown-unknown --release
WASM=../target/wasm32-unknown-unknown/release/myous_pake_wasm.wasm
wasm-bindgen --target nodejs --out-dir pkg "$WASM"   # Node, Deno, Bun (CommonJS)
wasm-bindgen --target web --out-dir pkg-web "$WASM"  # browsers and ES-module runtimes
ls -l pkg pkg-web
