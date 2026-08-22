// Client-side render state: a fogged view of the match, updated by StateDiff.
// No game rules live here.

import type { CrystalTile, DiffEntity, OreTile, ResearchMsg, TechId } from "./types";
import { BUILDING_KINDS, BUILDING_POWER, UNIT_KINDS } from "./types";

export interface Entity extends DiffEntity {}

export class World {
  mapSeed = 0;
  passable: boolean[] = [];
  /** Serde terrain names per tile ("Plains", "Forest", …) for typed rendering. */
  terrain: string[] = [];
  hq: [number, number][] = [];
  turn = 0;
  /** Index of the player whose turn it is (0 = P0/human, 1 = P1/bot). */
  activePlayer = 0;
  ore = 0;
  /** Banked strategic crystal (spent on the deep research). */
  crystal = 0;
  /** Research dashboard (server-authoritative). */
  research: ResearchMsg = { points: 0, researching: null, researched: [] };
  entities = new Map<number, Entity>();
  oreTiles = new Map<string, OreTile>();
  crystalTiles = new Map<string, CrystalTile>();
  explored = new Set<number>();
  visible = new Set<number>();
  events: { turn: number; kind: string }[] = [];
  result: { winner: number | null; reason: string | null } | null = null;
  /** Authoritative power from the server's state diffs (null outside live
   *  matches, where the static table below is the fallback). */
  private serverPower: { produced: number; consumed: number } | null = null;

  get ownUnits(): Entity[] {
    return [...this.entities.values()].filter((e) => e.owner === 0 && UNIT_KINDS.has(e.kind));
  }
  get ownBuildings(): Entity[] {
    return [...this.entities.values()].filter((e) => e.owner === 0 && BUILDING_KINDS.has(e.kind));
  }
  get ownPower(): { produced: number; consumed: number } {
    // Prefer the server's authoritative readout; fall back to the static
    // table only for menu/spectate scenes that have no live diff.
    if (this.serverPower) return this.serverPower;
    let produced = 0;
    let consumed = 0;
    for (const b of this.ownBuildings) {
      const stats = BUILDING_POWER[b.kind];
      if (stats) {
        produced += stats.produces;
        consumed += stats.consumes;
      }
    }
    return { produced, consumed };
  }
  get enemyEntities(): Entity[] {
    return [...this.entities.values()].filter((e) => e.owner === 1);
  }
  /** The tech currently being researched, if any. */
  get researching(): TechId | null {
    return this.research.researching;
  }

  setMap(mapSeed: number, passable: boolean[], terrain: string[], hq: [number, number][]): void {
    this.mapSeed = mapSeed;
    this.passable = passable;
    this.terrain = terrain;
    this.hq = hq;
  }

  applyDiff(
    turn: number,
    activePlayer: number,
    ore: number,
    crystal: number,
    research: ResearchMsg,
    entities: DiffEntity[],
    oreTiles: OreTile[],
    crystalTiles: CrystalTile[],
    visible: number[],
    events: { turn: number; kind: string }[],
    power?: { produced: number; consumed: number },
  ): void {
    this.turn = turn;
    this.activePlayer = activePlayer;
    this.ore = ore;
    this.crystal = crystal;
    this.research = research;
    this.serverPower = power ?? null;
    this.entities = new Map(entities.map((e) => [e.id, e]));
    this.oreTiles = new Map(oreTiles.map((t) => [`${t.x},${t.y}`, t]));
    this.crystalTiles = new Map(crystalTiles.map((t) => [`${t.x},${t.y}`, t]));
    this.visible = new Set(visible);
    for (const v of visible) this.explored.add(v);
    if (events.length > 0) {
      this.events.push(...events);
      if (this.events.length > 12) this.events = this.events.slice(-12);
    }
  }
}
