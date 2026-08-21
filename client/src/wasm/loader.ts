// Loader for the generated wasm-bindgen shim. The bindings
// (crucible_client_wasm.js / _bg.wasm / .d.ts) are produced by
// `scripts/build-wasm.sh` and are NOT committed — run `npm run wasm` first.
// This module is imported lazily so the main bundle never pulls the wasm in
// until spectate/replay is actually used.

import init, {
  replay_frame,
  replay_meta,
  replay_result,
  replay_snapshot_json,
  sim_version,
} from "./crucible_client_wasm";
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
  return JSON.parse(replay_meta(replayJson)) as ReplayMeta;
}

export async function frame(replayJson: string, turn: number): Promise<ReplayFrame> {
  await wasmInit();
  return JSON.parse(replay_frame(replayJson, turn)) as ReplayFrame;
}

export async function result(replayJson: string): Promise<{
  reason: string | null;
  duration_turns: number;
  hash: number;
}> {
  await wasmInit();
  return JSON.parse(replay_result(replayJson));
}

export async function snapshotJson(replayJson: string, turn: number): Promise<unknown> {
  await wasmInit();
  return JSON.parse(replay_snapshot_json(replayJson, turn)) as unknown;
}

export async function version(): Promise<string> {
  await wasmInit();
  return sim_version();
}
