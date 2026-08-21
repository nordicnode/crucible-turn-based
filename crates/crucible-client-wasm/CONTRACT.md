# CONTRACT — crucible-client-wasm

The `wasm-bindgen` shim exposing `crucible-sim` to the browser. **Status:
replay execution (`replay_result`, `replay_snapshot_json`), the lean spectate
API (`replay_meta`, `replay_frame`), and the wasm-bindgen-test/node
golden-parity runner are all implemented and wired into the client spectate
screen.**

Cross-target parity is proven in `tests/wasm_parity.rs`: the same
`crucible_sim::golden` scenario that runs natively in
`crucible-sim/tests/determinism.rs` runs here under wasm and must produce the
identical hashes. CI runs it via `cargo test -p crucible-client-wasm
--target wasm32-unknown-unknown` using `wasm-bindgen-test-runner` (installed
by `taiki-e/install-action` with `wasm-bindgen@<version>` pinned to the
`Cargo.lock` version).

## 1. Purpose & scope

This crate exists **only** so the browser can run the *same* deterministic sim
for local replay and spectate (replays, auto-battles). It is a thin
passthrough over `crucible-sim`, not a second implementation.

## 2. Hard rules

- **Never used for live matches.** Live matches are server-authoritative
  (`crucible-server/CONTRACT.md` §1). The wasm sim exists for local replay
  verification and spectating only — never for trust.
- **No game rules here.** It may allocate a `Game`/`Map`, apply command logs,
  step ticks, and serialize snapshots, but must not modify or duplicate game
  logic. Any behavior difference from native `crucible-sim` is a bug.
- **Same determinism.** Byte-identical to native: same seed + command log ⇒
  same serialized state. Golden tests run natively *and* under wasm
  (wasm-bindgen-test/node, `tests/wasm_parity.rs`) against the single shared
  set of constants in `crucible_sim::golden`, and must produce identical
  hashes.

## 3. API surface

- Exposes the minimum needed by `client/src/wasm/`: construct a game from a
  replay (seed + config + commands), step to any tick, and return a snapshot
  as JSON. `wasm_bindgen` bindings only; no DOM, no JS imports beyond the glue.
- Versioned entry points mirroring the `crucible-sim` replay format version:
  - `replay_meta(replay_json)` — map (passability, HQ spawns, ore layout) + outcome, once per replay.
  - `replay_frame(replay_json, tick)` — one lean per-tick frame (entities + scores), supports seeking forward/backward.
  - `replay_snapshot_json(replay_json, tick)` — full `Game` snapshot (hashes, diagnostics).
  - `replay_result(replay_json)` — final outcome + FNV-1a snapshot hash for native/wasm parity.
  - Frame `kind` strings use serde variant names (`"Infantry"`, `"Hq"`, …), the same convention as the live WS protocol.

## 4. Build contract

- Compiles to `wasm32-unknown-unknown` with zero `getrandom`/OS/thread
  dependencies (inherited from `crucible-sim`). Bundled size target: the
  client (JS + WASM) stays under 1 MB total.
