// Pure mapping from the wasm replay shim's meta/frame JSON into the client's
// World render state. No game rules here — this only reshapes serialized
// sim state into the shapes the renderer already draws. Kept wasm-free so it
// is unit-testable.

import { MAP_SIZE, MAP_TILES } from "./types";
import type {
  CrystalTile,
  DiffEntity,
  OreTile,
  ResourceBundle,
  ResourceTile,
  ResourceType,
  TerrainRule,
} from "./types";
import { DEFAULT_TERRAIN_RULES } from "./types";
import { World } from "./world";

/** A single entity in a spectate frame (both players, full state, no fog). */
export interface FrameEntity {
  id: number;
  kind: string;
  owner: number;
  x: number;
  y: number;
  hp: number;
  max_hp: number;
  mp?: number;
  max_mp?: number;
  move_target?: [number, number] | null;
  movement_path?: [number, number][] | null;
  moved?: boolean;
  acted?: boolean;
  queue?: string[];
  progress?: number | null;
  build_time?: number | null;
  construction_progress?: number | null;
  construction_time?: number | null;
}

/** Static metadata for a replay: map + recorded outcome. */
export interface ReplayMeta {
  map_seed: number;
  passable: boolean[];
  terrain: string[];
  terrain_rules?: TerrainRule[];
  elevation?: number[];
  moisture?: number[];
  temperature?: number[];
  hq_tiles: [number, number][];
  ore: number[];
  crystal: number[];
  steel?: number[];
  coal?: number[];
  resource_kind?: (ResourceType | null)[];
  richness?: number[];
  duration_turns: number;
  duration_rounds?: number;
  winner: number | null;
  win_reason: string | null;
}

/** One spectate frame at a turn. */
export interface ReplayFrame {
  /** Legacy activation number. */
  turn: number;
  /** Player-facing round, when supplied by the WASM shim. */
  round?: number;
  active: number;
  ore0: number;
  ore1: number;
  steel0?: number;
  steel1?: number;
  coal0?: number;
  coal1?: number;
  crystal0?: number;
  crystal1?: number;
  resources0?: ResourceBundle;
  resources1?: ResourceBundle;
  income0?: ResourceBundle;
  income1?: ResourceBundle;
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
  world.elevation = meta.elevation ?? [];
  world.moisture = meta.moisture ?? [];
  world.temperature = meta.temperature ?? [];
  world.terrainRules = new Map(
    (meta.terrain_rules ?? DEFAULT_TERRAIN_RULES).map((rule) => [rule.kind, rule]),
  );
  world.hq = meta.hq_tiles;
  world.resourceTiles = new Map<string, ResourceTile>();
  world.clearTileInspection();
  world.oreTiles = new Map<string, OreTile>();
  world.crystalTiles = new Map<string, CrystalTile>();
  for (let y = 0; y < MAP_SIZE; y++) {
    for (let x = 0; x < MAP_SIZE; x++) {
      const idx = y * MAP_SIZE + x;
      const amountByKind: Record<ResourceType, number> = {
        Ore: meta.ore[idx] ?? 0,
        Steel: meta.steel?.[idx] ?? 0,
        Coal: meta.coal?.[idx] ?? 0,
        Crystal: meta.crystal?.[idx] ?? 0,
      };
      const explicitKind = meta.resource_kind?.[idx] ?? null;
      const resource = explicitKind
        ?? (amountByKind.Ore > 0 ? "Ore" : amountByKind.Steel > 0 ? "Steel" : amountByKind.Coal > 0 ? "Coal" : amountByKind.Crystal > 0 ? "Crystal" : null);
      const marker = amountByKind[resource ?? "Ore"];
      if (!resource || (marker <= 0 && !((meta.richness?.[idx] ?? 0) > 0))) continue;
      const tile: ResourceTile = {
        x,
        y,
        resource,
        // Keep a positive compatibility marker for compact new payloads that
        // carry only kind/richness. This is never a remaining reserve.
        amount: Math.max(1, marker),
        richness: meta.richness?.[idx] ?? 1,
        infinite: true,
      };
      world.resourceTiles.set(`${x},${y}`, tile);
      if (resource === "Ore") world.oreTiles.set(`${x},${y}`, { x, y, amount: tile.amount });
      if (resource === "Crystal") world.crystalTiles.set(`${x},${y}`, { x, y, amount: tile.amount });
    }
  }
  // Spectate shows the whole map: no fog.
  const all = new Set<number>();
  for (let i = 0; i < MAP_TILES; i++) all.add(i);
  world.visible = all;
  world.explored = new Set(all);
  world.entities = new Map();
  world.turn = 0;
  world.round = 1;
  world.activePlayer = 0;
  world.ore = 0;
  world.steel = 0;
  world.coal = 0;
  world.crystal = 0;
  world.resources = { ore: 0, steel: 0, coal: 0, crystal: 0 };
  world.income = { ore: 0, steel: 0, coal: 0, crystal: 0 };
  world.events = [];
  world.result = null;
  world.clearTileInspection();
}

/** Replace the world's entities/score with one spectate frame. */
export function applyFrame(world: World, frame: ReplayFrame): void {
  world.turn = frame.turn;
  world.round = frame.round ?? Math.max(1, Math.floor((frame.turn + 1) / 2));
  world.activePlayer = frame.active;
  world.ore = frame.ore0;
  world.steel = frame.steel0 ?? 0;
  world.coal = frame.coal0 ?? 0;
  world.crystal = frame.crystal0 ?? 0;
  world.resources = frame.resources0 ?? {
    ore: world.ore,
    steel: world.steel,
    coal: world.coal,
    crystal: world.crystal,
  };
  world.income = frame.income0 ?? { ore: 0, steel: 0, coal: 0, crystal: 0 };
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
      mp: u.mp,
      maxMp: u.max_mp,
      moveTarget: u.move_target,
      movementPath: u.movement_path,
      moved: u.moved,
      acted: u.acted,
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
      queue: b.queue,
      progress: b.progress ?? undefined,
      buildTime: b.build_time ?? undefined,
      constructionProgress: b.construction_progress ?? undefined,
      constructionTime: b.construction_time ?? undefined,
    });
  }
  world.entities = entities;
  world.result =
    frame.winner == null
      ? null
      : { winner: frame.winner, reason: frame.win_reason };
}
