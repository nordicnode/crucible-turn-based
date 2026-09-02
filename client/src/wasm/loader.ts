// Loader for replay playback. Replays store deterministic state frames and metadata
// so playback runs smoothly in the browser.

import type { ReplayFrame, ReplayMeta } from "../snapshot";

let ready: Promise<unknown> | null = null;

/** Instantiate the loader. Safe to call repeatedly. */
export function wasmInit(): Promise<unknown> {
  if (!ready) ready = Promise.resolve();
  return ready;
}

export async function meta(replayJson: string): Promise<ReplayMeta> {
  await wasmInit();
  try {
    const data = typeof replayJson === "string" ? JSON.parse(replayJson) : replayJson;
    if (data.meta) return data.meta as ReplayMeta;
    return data as ReplayMeta;
  } catch (err) {
    // Surface a clear error for a corrupt/legacy replay; the caller (spectate)
    // catches this and degrades to an error card instead of crashing the page.
    throw new Error(`invalid replay metadata: ${err}`);
  }
}

export async function frame(replayJson: string, turn: number): Promise<ReplayFrame> {
  await wasmInit();
  try {
    const data = typeof replayJson === "string" ? JSON.parse(replayJson) : replayJson;
    if (data.frames && Array.isArray(data.frames)) {
      const clampedTurn = Math.min(data.frames.length - 1, Math.max(0, turn));
      return data.frames[clampedTurn] as ReplayFrame;
    }
    return {
      turn,
      round: Math.max(1, Math.floor((turn + 1) / 2)),
      active: turn % 2,
      ore0: 400,
      ore1: 400,
      units: [],
      buildings: [],
      winner: null,
      win_reason: null,
    };
  } catch (err) {
    throw new Error(`invalid replay frame (turn ${turn}): ${err}`);
  }
}
