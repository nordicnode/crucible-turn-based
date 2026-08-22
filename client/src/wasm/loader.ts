// Loader for the generated wasm-bindgen shim. The bindings
// (crucible_client_wasm.js / _bg.wasm / .d.ts) are produced by
// `scripts/build-wasm.sh` and are NOT committed — run `npm run wasm` first.
// This module is imported lazily so the main bundle never pulls the wasm in
// until spectate/replay is actually used.

import init, { replay_frame, replay_meta } from "./crucible_client_wasm";
import wasmUrl from "./crucible_client_wasm_bg.wasm?url";

import type { ReplayFrame, ReplayMeta } from "../snapshot";

let ready: Promise<unknown> | null = null;

/** Instantiate the wasm module once. Safe to call repeatedly. */
export function wasmInit(): Promise<unknown> {
  if (!ready) ready = init(wasmUrl);
  return ready;
}

export async function meta(replayJson: string): Promise<ReplayMeta> {
  await wasmInit();
  const raw = replay_meta(replayJson);
  try {
    return JSON.parse(raw) as ReplayMeta;
  } catch (err) {
    // Surface a clear error for a corrupt/legacy replay; the caller (spectate)
    // catches this and degrades to an error card instead of crashing the page.
    throw new Error(`invalid replay metadata: ${err}`);
  }
}

export async function frame(replayJson: string, turn: number): Promise<ReplayFrame> {
  await wasmInit();
  const raw = replay_frame(replayJson, turn);
  try {
    return JSON.parse(raw) as ReplayFrame;
  } catch (err) {
    throw new Error(`invalid replay frame (turn ${turn}): ${err}`);
  }
}
