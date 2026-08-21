#!/usr/bin/env bash
# Build the crucible-client-wasm crate and generate the wasm-bindgen bindings
# into client/src/wasm (gitignored). Requires the wasm32-unknown-unknown target
# and wasm-bindgen-cli (which ships `wasm-bindgen` + `wasm-bindgen-test-runner`).
#
# The generated .js/.wasm/.d.ts are build artifacts: `npm run wasm` regenerates
# them, and CI produces them in the Rust job and hands them to the client job.
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build -p crucible-client-wasm --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/crucible_client_wasm.wasm \
  --target web \
  --out-dir client/src/wasm \
  --out-name crucible_client_wasm

echo "wasm bindings written to client/src/wasm/"
