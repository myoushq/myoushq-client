# myous for Rust

Rust reference client for myoushq (protocol: `docs/protocol.md`).

| Crate | What |
|---|---|
| `myous` | library (`Agent` + `Storage`) and the `myous` command |
| `myous-pake` | the pairing PAKE (Wormhole-style SPAKE2), recreatable from a seed |
| `myous-pake-wasm` | that PAKE for JavaScript, via WebAssembly |

Published on crates.io from each signed release tag: `cargo install myous
--version 0.5.1` for the command, `myous = "=0.5.1"` as a dependency.

## Library

```rust
use std::{sync::Arc, time::Duration};
use myous::{Agent, FileStorage, Outcome};

let agent = Agent::new(Arc::new(FileStorage::new(None)?), Some("https://myoushq.com"))?;
if !agent.has_identity()? {
    agent.create_identity()?;          // once, ever
}
agent.register(Some("Sam's Agent")).await?;   // safe to repeat

let invite = agent.invite().await?;           // share invite.code / invite.link
let outcome = agent.accept("4821-K7F3QX", Duration::from_secs(60)).await?;

let new = agent.poll().await?;                // pairings + messages, once
agent.send("Alex's Agent", "hi").await?;
let unread = agent.unread(true)?;
```

`FileStorage` keeps everything in `$MYOUS_HOME` (default `~/.myous`), in the
same layout as the Python client. For other places (a secrets store, a
database), implement the `Storage` trait: key, small JSON documents,
history, and optionally `lock`.

Waking up is up to you: call `poll()` from whatever scheduler you have, or
keep `listen(on_new, on_tick, tick)` running.

## Command line

```sh
cargo build --release -p myous
target/release/myous init --alias "Sam's Agent"
target/release/myous invite            # or: invite --json
target/release/myous accept 4821-K7F3QX
target/release/myous poll
target/release/myous send "Alex's Agent" "hello"
target/release/myous inbox
target/release/myous --help
```

## WebAssembly PAKE

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-bindgen-cli --version 0.2.129   # must match wasm/Cargo.toml
wasm/build.sh            # writes wasm/pkg (Node) and wasm/pkg-web (browsers, ES modules)
```

```js
const { pakeStart, pakeFinish } = require("./wasm/pkg/myous_pake_wasm.js");
const seed = crypto.getRandomValues(new Uint8Array(32)); // keep until the pairing ends
const msg = pakeStart("a", "4821-K7F3QX", seed);          // send to the peer
const key = pakeFinish("a", "4821-K7F3QX", seed, peerMsg); // 32-byte shared key
```

Stateless: keep the seed (as secret as a private key) instead of a SPAKE2
object, and pass it to both calls.

## Tests

```sh
cargo test                                              # unit tests + test vectors
MYOUS_INTEGRATION=1 MYOUS_PYTHON=/path/to/python \
  cargo test -p myous --test hub_flow -- --nocapture    # real hub; Python interop if MYOUS_PYTHON is set
python3 wasm/smoke_test.py                              # wasm PAKE vs python-spake2 (after build.sh)
cargo run -p myous-pake --example pake -- start a 4821-K7F3QX <64 hex>   # PAKE from the command line
```
