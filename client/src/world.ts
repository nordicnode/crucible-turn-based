// Client-side render state: a fogged view of the match, updated by StateDiff.
// No game rules live here.

import { MAP_SIZE } from "./types";
import type {
  CrystalTile,
  DiffEntity,
  OreTile,
  ResearchMsg,
  ResourceBundle,
  ResourceTile,
  TechId,
  TerrainRule,
  TileInspection,
} from "./types";
import { BUILDING_KINDS, BUILDING_POWER, DEFAULT_TERRAIN_RULES, UNIT_KINDS } from "./types";

export interface Entity extends DiffEntity {}

export class World {
  mapSeed = 0;
  passable: boolean[] = [];
  /** Serde terrain names per tile ("Plains", "Forest", …) for typed rendering. */
  terrain: string[] = [];
  /** Server-supplied terrain behavior used by the tile inspector. */
  terrainRules = new Map<string, TerrainRule>(DEFAULT_TERRAIN_RULES.map((rule) => [rule.kind, rule]));
  /** Deterministic climate metadata for richer local terrain presentation. */
  elevation: number[] = [];
  moisture: number[] = [];
  temperature: number[] = [];
  hq: [number, number][] = [];
  /** Legacy activation counter. */
  turn = 0;
  /** Player-facing human+AI round. */
  round = 1;
  /** Index of the player whose activation it is (0 = P0/human, 1 = P1/bot). */
  activePlayer = 0;
  ore = 0;
  steel = 0;
  coal = 0;
  /** Banked strategic crystal (spent on the deep research). */
  crystal = 0;
  resources: ResourceBundle = { ore: 0, steel: 0, coal: 0, crystal: 0 };
  income: ResourceBundle = { ore: 0, steel: 0, coal: 0, crystal: 0 };
  /** Research dashboard (server-authoritative). */
  research: ResearchMsg = { points: 0, researching: null, researched: [] };
  entities = new Map<number, Entity>();
  /** Generic resource deposits known to the player. */
  resourceTiles = new Map<string, ResourceTile>();
  /** Latest server-authoritative inspection response for the selected tile. */
  tileInspection: TileInspection | null = null;
  /** Legacy projections used by old fixtures and replay metadata. */
  oreTiles = new Map<string, OreTile>();
  crystalTiles = new Map<string, CrystalTile>();
  explored = new Set<number>();
  visible = new Set<number>();
  events: { turn: number; kind: string }[] = [];
  result: { winner: number | null; reason: string | null } | null = null;
  /** Actions spent this turn (from the server diff). */
  budgetSpent = 0;
  /** Max actions per turn. */
  budgetCap = 16;
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

  get resourceCounts(): Record<string, number> {
    const counts: Record<string, number> = { Ore: 0, Steel: 0, Coal: 0, Crystal: 0 };
    for (const tile of this.resourceTiles.values()) {
      // `amount` is a legacy static marker; presence is authoritative even
      // when a compact payload omits it. Deposits are not depleted.
      if (tile.infinite === true || tile.amount > 0) {
        counts[tile.resource] = (counts[tile.resource] ?? 0) + 1;
      }
    }
    return counts;
  }

  setMap(
    mapSeed: number,
    passable: boolean[],
    terrain: string[],
    hq: [number, number][],
    terrainRules: TerrainRule[] = DEFAULT_TERRAIN_RULES,
    elevation: number[] = [],
    moisture: number[] = [],
    temperature: number[] = [],
  ): void {
    this.mapSeed = mapSeed;
    this.passable = passable;
    this.terrain = terrain;
    this.terrainRules = new Map(terrainRules.map((rule) => [rule.kind, rule]));
    this.elevation = elevation;
    this.moisture = moisture;
    this.temperature = temperature;
    this.hq = hq;
    this.tileInspection = null;
  }

  applyTileInspection(inspection: TileInspection): void {
    this.tileInspection = inspection;
  }

  clearTileInspection(): void {
    this.tileInspection = null;
  }

  terrainRuleAt(x: number, y: number): TerrainRule | null {
    if (x < 0 || y < 0 || x >= MAP_SIZE || y >= MAP_SIZE) return null;
    return this.terrainRules.get(this.terrain[y * MAP_SIZE + x] ?? "Plains") ?? null;
  }

  applyDiff(
    turn: number,
    activePlayer: number,
    ore: number,
    crystal: number,
    research: ResearchMsg,
    entities: DiffEntity[],
    oreTiles: OreTile[] = [],
    crystalTiles: CrystalTile[] = [],
    visible: number[] = [],
    events: { turn: number; kind: string }[] = [],
    power?: { produced: number; consumed: number },
    steel = 0,
    coal = 0,
    resources?: ResourceBundle,
    income?: ResourceBundle,
    resourceTiles: ResourceTile[] = [],
    actionsSpent?: number,
    actionsCap?: number,
    round?: number,
  ): void {
    this.turn = turn;
    this.round = round ?? Math.max(1, Math.floor((turn + 1) / 2));
    this.activePlayer = activePlayer;
    if (actionsSpent != null) this.budgetSpent = actionsSpent;
    if (actionsCap != null) this.budgetCap = actionsCap;
    this.ore = ore;
    this.steel = steel;
    this.coal = coal;
    this.crystal = crystal;
    this.resources = resources ?? { ore, steel, coal, crystal };
    this.income = income ?? { ore: 0, steel: 0, coal: 0, crystal: 0 };
    this.research = research;
    this.serverPower = power ?? null;
    this.tileInspection = null;
    this.entities = new Map(entities.map((e) => [e.id, e]));
    this.resourceTiles = new Map(resourceTiles.map((t) => [`${t.x},${t.y}`, t]));
    this.oreTiles = new Map(oreTiles.map((t) => [`${t.x},${t.y}`, t]));
    this.crystalTiles = new Map(crystalTiles.map((t) => [`${t.x},${t.y}`, t]));
    // Older server/replay frames only carry split ore/crystal arrays. Keep
    // them useful while preferring the generic deposit stream when present.
    if (this.resourceTiles.size === 0) {
      for (const t of oreTiles) {
        this.resourceTiles.set(`${t.x},${t.y}`, {
          x: t.x,
          y: t.y,
          resource: "Ore",
          amount: t.amount,
          richness: 1,
          infinite: true,
        });
      }
      for (const t of crystalTiles) {
        this.resourceTiles.set(`${t.x},${t.y}`, {
          x: t.x,
          y: t.y,
          resource: "Crystal",
          amount: t.amount,
          richness: 1,
          infinite: true,
        });
      }
    }
    this.visible = new Set(visible);
    for (const v of visible) this.explored.add(v);
    if (events.length > 0) {
      this.events.push(...events);
      if (this.events.length > 12) this.events = this.events.slice(-12);
    }
  }
}
