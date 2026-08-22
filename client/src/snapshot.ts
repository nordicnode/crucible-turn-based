// Pure mapping from the wasm replay shim's meta/frame JSON into the client's
// World render state. No game rules here — this only reshapes serialized
// sim state into the shapes the renderer already draws. Kept wasm-free so it
// is unit-testable.

import type { CrystalTile, DiffEntity, OreTile } from "./types";
import { World } from "./world";

const MAP = 64;

/** A single entity in a spectate frame (both players, full state, no fog). */
export interface FrameEntity {
  id: number;
  kind: string;
  owner: number;
  x: number;
  y: number;
  hp: number;
  max_hp: number;
}

/** Static metadata for a replay: map + recorded outcome. */
export interface ReplayMeta {
  map_seed: number;
  passable: boolean[];
  terrain: string[];
  hq_tiles: [number, number][];
  ore: number[];
  crystal: number[];
  duration_turns: number;
  winner: number | null;
  win_reason: string | null;
}

/** One spectate frame at a turn. */
export interface ReplayFrame {
  turn: number;
  active: number;
  ore0: number;
  ore1: number;
  units: FrameEntity[];
  buildings: FrameEntity[];
  winner: number | null;
  win_reason: string | null;
}

/** Seed the world with a replay's map, shown with full visibility. */
export function applyMeta(world: World, meta: ReplayMeta): void {
  world.mapSeed = meta.map_seed;
  world.passable = meta.passable;
  world.terrain = meta.terrain ?? [];
  world.hq = meta.hq_tiles;
  world.oreTiles = new Map<string, OreTile>();
  world.crystalTiles = new Map<string, CrystalTile>();
  for (let y = 0; y < MAP; y++) {
    for (let x = 0; x < MAP; x++) {
      const amount = meta.ore[y * MAP + x];
      if (amount > 0) world.oreTiles.set(`${x},${y}`, { x, y, amount });
      const cAmount = (meta.crystal ?? [])[y * MAP + x] ?? 0;
      if (cAmount > 0) world.crystalTiles.set(`${x},${y}`, { x, y, amount: cAmount });
    }
  }
  // Spectate shows the whole map: no fog.
  const all = new Set<number>();
  for (let i = 0; i < MAP * MAP; i++) all.add(i);
  world.visible = all;
  world.explored = new Set(all);
  world.entities = new Map();
  world.turn = 0;
  world.activePlayer = 0;
  world.ore = 0;
  world.events = [];
  world.result = null;
}

/** Replace the world's entities/score with one spectate frame. */
export function applyFrame(world: World, frame: ReplayFrame): void {
  world.turn = frame.turn;
  world.activePlayer = frame.active;
  world.ore = frame.ore0;
  const entities = new Map<number, DiffEntity>();
  for (const u of frame.units) {
    entities.set(u.id, {
      id: u.id,
      kind: u.kind,
      owner: u.owner,
      x: u.x,
      y: u.y,
      hp: u.hp,
      maxHp: u.max_hp,
    });
  }
  for (const b of frame.buildings) {
    entities.set(b.id, {
      id: b.id,
      kind: b.kind,
      owner: b.owner,
      x: b.x,
      y: b.y,
      hp: b.hp,
      maxHp: b.max_hp,
    });
  }
  world.entities = entities;
  world.result =
    frame.winner == null
      ? null
      : { winner: frame.winner, reason: frame.win_reason };
}
