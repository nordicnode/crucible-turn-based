// Wire types mirroring the server's JSON protocol, plus command builders.
// The client never implements game rules; it only renders fogged state and
// sends the same commands the sim validates.

export const MAP_SIZE = 128;
export const MAP_TILES = MAP_SIZE * MAP_SIZE;

export type BuildingType =
  | "Hq"
  | "PowerPlant"
  | "Refinery"
  | "CrystalRefinery"
  | "Barracks"
  | "Factory"
  | "TechLab"
  | "Airfield"
  | "Radar"
  | "TeslaCoil"
  | "Turret"
  | "AATurret";
export type UnitType =
  | "Infantry"
  | "Scout"
  | "RocketTrooper"
  | "Tank"
  | "Artillery"
  | "MammothTank"
  | "Gunship"
  | "Interceptor"
  | "SamLauncher";

export type TechId =
  | "HighExplosive"
  | "CompositeArmor"
  | "TargetingOptics"
  | "EfficientRefining"
  | "RocketPropulsion"
  | "TitaniumAlloys"
  | "AerialSuperiority"
  | "Superconductors"
  | "CrystalNanotech"
  | "AdvancedBallistics";

/** Exhaustive presentation order for the authoritative sim content pack. */
export const PLAYABLE_BUILDING_TYPES: BuildingType[] = [
  "Hq",
  "PowerPlant",
  "Refinery",
  "CrystalRefinery",
  "Barracks",
  "Factory",
  "TechLab",
  "Airfield",
  "Radar",
  "TeslaCoil",
  "Turret",
  "AATurret",
];
export const PLAYABLE_UNIT_TYPES: UnitType[] = [
  "Infantry",
  "Scout",
  "RocketTrooper",
  "Tank",
  "Artillery",
  "MammothTank",
  "Gunship",
  "Interceptor",
  "SamLauncher",
];
export const TECH_TYPES: TechId[] = [
  "HighExplosive",
  "CompositeArmor",
  "TargetingOptics",
  "EfficientRefining",
  "RocketPropulsion",
  "TitaniumAlloys",
  "AerialSuperiority",
  "Superconductors",
  "CrystalNanotech",
  "AdvancedBallistics",
];

// The server's serde format serializes `Player` as the variant name ("P0" /
// "P1"), so commands must carry the string, not an index.
export type Player = "P0" | "P1";
export type ResourceType = "Ore" | "Steel" | "Coal" | "Crystal";

/** Authoritative presentation metadata for one sim terrain kind. */
export interface TerrainRule {
  kind: string;
  label: string;
  passable: boolean;
  moveMultiplier: number;
  defenseReduction: number;
  tacticalTag: string;
}

export interface ResourceBundle {
  ore: number;
  steel: number;
  coal: number;
  crystal: number;
}

export type Command =
  | { PlaceBuilding: { player: Player; btype: BuildingType; tile: [number, number] } }
  | { TrainUnit: { player: Player; building: number; utype: UnitType } }
  | { MoveGroup: { player: Player; units: number[]; waypoint: [number, number] } }
  | { ClearMove: { player: Player; units: number[] } }
  | { Attack: { player: Player; units: number[]; target: number } }
  | { StartResearch: { player: Player; tech: TechId } }
  | { Sell: { player: Player; building: number } }
  | { Repair: { player: Player; building: number } }
  | { EndTurn: { player: Player } };

export const PLAYER: Player = "P0";

export function placeBuilding(btype: BuildingType, tile: [number, number]): Command {
  return { PlaceBuilding: { player: PLAYER, btype, tile } };
}
export function trainUnit(building: number, utype: UnitType): Command {
  return { TrainUnit: { player: PLAYER, building, utype } };
}
export function moveGroup(units: number[], waypoint: [number, number]): Command {
  return { MoveGroup: { player: PLAYER, units, waypoint } };
}
/** Clear durable destinations without moving the selected units. */
export function clearMove(units: number[]): Command {
  return { ClearMove: { player: PLAYER, units } };
}
/** Focus-fire a specific enemy entity with the given units (C&C right-click). */
export function attack(units: number[], target: number): Command {
  return { Attack: { player: PLAYER, units, target } };
}
/** Start researching a technology (needs a Tech Lab; one at a time). */
export function startResearch(tech: TechId): Command {
  return { StartResearch: { player: PLAYER, tech } };
}
export function sell(building: number): Command {
  return { Sell: { player: PLAYER, building } };
}
export function repair(building: number): Command {
  return { Repair: { player: PLAYER, building } };
}
/** End the active player's turn (only valid while it is the player's turn). */
export function endTurn(): Command {
  return { EndTurn: { player: PLAYER } };
}

export interface DiffEntity {
  id: number;
  kind: string;
  owner: number;
  /** Tile-center coordinates (tile + 0.5), matching the replay spectate
   *  frames so live and replay rendering share one convention. */
  x: number;
  y: number;
  hp: number;
  maxHp: number;
  /** Turns since this enemy was last seen (own entities are never stale). */
  stale?: number;
  /** Own-building production queue (unit kind names, oldest first). */
  queue?: string[];
  /** Progress of the current queue head, in turns. */
  progress?: number;
  /** Build time of the current queue head, in turns. */
  buildTime?: number;
  /** Construction progress of a building, in turns. */
  constructionProgress?: number;
  /** Construction duration of a building, in turns. */
  constructionTime?: number;
  /** Current and maximum movement points for own units. */
  mp?: number;
  maxMp?: number;
  /** Durable destination and deterministic path preview for own units. */
  moveTarget?: [number, number] | null;
  movementPath?: [number, number][] | null;
  moved?: boolean;
  acted?: boolean;
}

export interface ResourceTile {
  x: number;
  y: number;
  resource: ResourceType;
  /** Legacy static marker retained for old replay/client payloads. */
  amount: number;
  richness: number;
  /** New deposits are inexhaustible; absent means a legacy finite-looking payload. */
  infinite?: boolean;
  /** Server-authoritative extraction estimate for the occupying refinery. */
  yieldPerTurn?: number;
  /** 0/1 owner of the refinery claiming this tile, if any. */
  refineryOwner?: number | null;
}

export type TileVisibility = "unexplored" | "explored" | "visible";

/** Fog-filtered, server-authoritative facts for the currently selected tile. */
export interface TileInspection {
  x: number;
  y: number;
  index: number;
  visibility: TileVisibility;
  terrain: TerrainRule | null;
  elevation?: number | null;
  moisture?: number | null;
  temperature?: number | null;
  resource: ResourceTile | null;
  occupants: DiffEntity[];
  movement: TileMovement[];
  routeTargets: RouteTarget[];
  placement: PlacementFacts;
}

export interface TileMovement {
  unitId: number;
  unitKind: string;
  movePoints: number;
  terrainCost: number;
  canEnter: boolean;
}

export interface RouteTarget {
  unitId: number;
  target: [number, number];
}

export interface PlacementFacts {
  known: boolean;
  passable: boolean | null;
  occupiedByBuilding: boolean;
  occupiedByUnit: boolean;
  resource: ResourceType | null;
  withinBaseRadius: boolean;
  structureSiteAvailable: boolean;
  refinerySiteAvailable: boolean;
}

export interface OreTile {
  x: number;
  y: number;
  amount: number;
}

export interface CrystalTile {
  x: number;
  y: number;
  amount: number;
}

/** The player's research dashboard from the server. */
export interface ResearchMsg {
  points: number;
  researching: TechId | null;
  researched: TechId[];
}

export interface DiffEvent {
  /** Legacy activation number. */
  turn: number;
  /** Player-facing round containing the event, when supplied by v6 servers. */
  round?: number;
  kind: string;
  /** Amount for a mined or sold event (undefined otherwise). */
  amount?: number;
  /** Resource name for generic mining events, when present. */
  resource?: ResourceType;
  /** Player index (0 = P0/friendly, 1 = P1/enemy). */
  player?: number;
}

export type ServerMsg =
  | {
      type: "matchStart";
      mapSeed: number;
      player: number;
      passable: boolean[];
      terrain: string[];
      terrainRules?: TerrainRule[];
      /** Deterministic climate fields for texture and tile inspection. */
      elevation?: number[];
      moisture?: number[];
      temperature?: number[];
      hq: [number, number][];
    }
  | {
      type: "stateDiff";
      /** Legacy activation counter. */
      turn: number;
      /** Player-facing human+AI round; old servers may omit it. */
      round?: number;
      activePlayer: number;
      ore: number;
      steel: number;
      coal: number;
      crystal: number;
      resources: ResourceBundle;
      income: ResourceBundle;
      powerProduced?: number;
      powerConsumed?: number;
      research: ResearchMsg;
      entities: DiffEntity[];
      resourceTiles: ResourceTile[];
      /** Legacy projections retained so older replay/client fixtures can load. */
      oreTiles?: OreTile[];
      crystalTiles?: CrystalTile[];
      visible: number[];
      events: DiffEvent[];
      actionsSpent?: number;
      actionsCap?: number;
    }
  | ({ type: "tileInspection" } & TileInspection)
  | { type: "commandRejected"; index: number; reason: string }
  | {
      type: "matchEnd";
      winner: number | null;
      reason: string | null;
      durationTurns: number;
      durationRounds?: number;
      replayId: number | null;
    }
  | { type: "serverBusy" };

export type ClientMsg =
  | { type: "joinMatch"; opponent: string }
  | { type: "commands"; cmds: Command[] }
  | { type: "inspectTile"; x: number; y: number }
  | { type: "endTurn" };

export const DEFAULT_TERRAIN_RULES: TerrainRule[] = [
  { kind: "Plains", label: "Plains", passable: true, moveMultiplier: 1, defenseReduction: 0, tacticalTag: "open ground" },
  { kind: "Forest", label: "Forest", passable: true, moveMultiplier: 2, defenseReduction: 20, tacticalTag: "tree cover" },
  { kind: "Hills", label: "Hills", passable: true, moveMultiplier: 2, defenseReduction: 30, tacticalTag: "high ground" },
  { kind: "Desert", label: "Desert", passable: true, moveMultiplier: 1, defenseReduction: 0, tacticalTag: "open arid ground" },
  { kind: "Swamp", label: "Swamp", passable: true, moveMultiplier: 3, defenseReduction: 10, tacticalTag: "slow wetland cover" },
  { kind: "Water", label: "Lake", passable: false, moveMultiplier: 1, defenseReduction: 0, tacticalTag: "impassable lake" },
  { kind: "River", label: "River", passable: true, moveMultiplier: 3, defenseReduction: 0, tacticalTag: "slow crossing" },
  { kind: "Mountain", label: "Mountain", passable: false, moveMultiplier: 1, defenseReduction: 0, tacticalTag: "impassable rock" },
];

export const BUILD_COSTS: Record<string, ResourceBundle> = {
  PowerPlant: { ore: 150, steel: 20, coal: 50, crystal: 0 },
  Refinery: { ore: 300, steel: 50, coal: 0, crystal: 0 },
  // Compatibility alias for old replays; the UI uses the generic Refinery.
  CrystalRefinery: { ore: 350, steel: 50, coal: 0, crystal: 0 },
  Barracks: { ore: 150, steel: 40, coal: 0, crystal: 0 },
  Factory: { ore: 250, steel: 100, coal: 30, crystal: 0 },
  TechLab: { ore: 200, steel: 80, coal: 50, crystal: 0 },
  Airfield: { ore: 250, steel: 80, coal: 100, crystal: 0 },
  Radar: { ore: 150, steel: 40, coal: 30, crystal: 0 },
  TeslaCoil: { ore: 250, steel: 100, coal: 100, crystal: 0 },
  Turret: { ore: 100, steel: 30, coal: 10, crystal: 0 },
  AATurret: { ore: 200, steel: 80, coal: 50, crystal: 0 },
};

export const BUILDING_POWER: Record<string, { produces: number; consumes: number }> = {
  Hq: { produces: 50, consumes: 0 },
  PowerPlant: { produces: 100, consumes: 0 },
  Refinery: { produces: 0, consumes: 20 },
  CrystalRefinery: { produces: 0, consumes: 25 },
  Barracks: { produces: 0, consumes: 15 },
  Factory: { produces: 0, consumes: 25 },
  TechLab: { produces: 0, consumes: 30 },
  Airfield: { produces: 0, consumes: 25 },
  Radar: { produces: 0, consumes: 10 },
  TeslaCoil: { produces: 0, consumes: 30 },
  Turret: { produces: 0, consumes: 20 },
  AATurret: { produces: 0, consumes: 25 },
};

export interface UnitStat {
  hp: number;
  damage: number;
  range_tiles: number;
  min_range_tiles: number;
  mp: number;
  vision_tiles: number;
  build_time_turns: number;
  air: boolean;
  aa: boolean;
}

export interface BuildingStat {
  hp: number;
  vision_tiles: number;
  damage: number;
  range_tiles: number;
  power: number;
  build_time_turns: number;
}

export const UNIT_STATS: Record<string, UnitStat> = {
  Infantry: { hp: 90, damage: 55, range_tiles: 1, min_range_tiles: 0, mp: 3, vision_tiles: 4, build_time_turns: 1, air: false, aa: false },
  Scout: { hp: 60, damage: 30, range_tiles: 1, min_range_tiles: 0, mp: 6, vision_tiles: 6, build_time_turns: 1, air: false, aa: false },
  RocketTrooper: { hp: 90, damage: 85, range_tiles: 2, min_range_tiles: 0, mp: 3, vision_tiles: 4, build_time_turns: 2, air: false, aa: true },
  Tank: { hp: 260, damage: 105, range_tiles: 1, min_range_tiles: 0, mp: 5, vision_tiles: 5, build_time_turns: 2, air: false, aa: false },
  Artillery: { hp: 120, damage: 110, range_tiles: 3, min_range_tiles: 2, mp: 3, vision_tiles: 6, build_time_turns: 2, air: false, aa: false },
  MammothTank: { hp: 550, damage: 170, range_tiles: 1, min_range_tiles: 0, mp: 4, vision_tiles: 5, build_time_turns: 3, air: false, aa: true },
  Gunship: { hp: 140, damage: 105, range_tiles: 2, min_range_tiles: 0, mp: 7, vision_tiles: 5, build_time_turns: 2, air: true, aa: false },
  Interceptor: { hp: 110, damage: 70, range_tiles: 2, min_range_tiles: 0, mp: 8, vision_tiles: 6, build_time_turns: 2, air: true, aa: false },
  SamLauncher: { hp: 110, damage: 35, range_tiles: 4, min_range_tiles: 1, mp: 2, vision_tiles: 5, build_time_turns: 2, air: false, aa: true },
};

/** Which building produces each unit, plus the gates on that production
 *  (U3 build tree): a unit may need a tech researched and/or another
 *  building present before it can be trained. Mirrors the sim's
 *  `building_produces` / `unit_requires_tech` / placement rules. */
export const UNIT_TREE: Record<
  string,
  { building: string; tech?: TechId; buildingReq?: string }
> = {
  Infantry: { building: "Barracks" },
  Scout: { building: "Barracks" },
  RocketTrooper: { building: "Barracks", tech: "RocketPropulsion" },
  Tank: { building: "Factory" },
  Artillery: { building: "Factory", buildingReq: "TechLab" },
  MammothTank: { building: "Factory", buildingReq: "TechLab" },
  SamLauncher: { building: "Factory", tech: "RocketPropulsion" },
  Gunship: { building: "Airfield" },
  Interceptor: { building: "Airfield" },
};

/** Buildings that require another building to be placed first. */
export const BUILDING_PREREQS: Record<string, string> = {
  TechLab: "Factory",
  Airfield: "Factory",
  Radar: "TechLab",
  TeslaCoil: "TechLab",
  AATurret: "TechLab",
};

export const BUILD_STATS: Record<string, BuildingStat> = {
  Hq: { hp: 1500, vision_tiles: 7, damage: 0, range_tiles: 0, power: 50, build_time_turns: 0 },
  PowerPlant: { hp: 300, vision_tiles: 3, damage: 0, range_tiles: 0, power: 100, build_time_turns: 1 },
  Refinery: { hp: 400, vision_tiles: 3, damage: 0, range_tiles: 0, power: -20, build_time_turns: 2 },
  CrystalRefinery: { hp: 400, vision_tiles: 3, damage: 0, range_tiles: 0, power: -25, build_time_turns: 2 },
  Barracks: { hp: 300, vision_tiles: 3, damage: 0, range_tiles: 0, power: -15, build_time_turns: 2 },
  Factory: { hp: 400, vision_tiles: 3, damage: 0, range_tiles: 0, power: -25, build_time_turns: 2 },
  TechLab: { hp: 250, vision_tiles: 3, damage: 0, range_tiles: 0, power: -30, build_time_turns: 3 },
  Airfield: { hp: 350, vision_tiles: 4, damage: 0, range_tiles: 0, power: -25, build_time_turns: 2 },
  Radar: { hp: 300, vision_tiles: 10, damage: 0, range_tiles: 0, power: -10, build_time_turns: 2 },
  TeslaCoil: { hp: 260, vision_tiles: 4, damage: 24, range_tiles: 4, power: -30, build_time_turns: 2 },
  Turret: { hp: 150, vision_tiles: 4, damage: 12, range_tiles: 3, power: -20, build_time_turns: 1 },
  AATurret: { hp: 200, vision_tiles: 5, damage: 45, range_tiles: 4, power: -25, build_time_turns: 2 },
};

export const UNIT_COSTS: Record<string, ResourceBundle> = {
  Infantry: { ore: 50, steel: 10, coal: 0, crystal: 0 },
  Scout: { ore: 40, steel: 8, coal: 0, crystal: 0 },
  RocketTrooper: { ore: 120, steel: 35, coal: 0, crystal: 0 },
  Tank: { ore: 150, steel: 60, coal: 20, crystal: 0 },
  Artillery: { ore: 200, steel: 80, coal: 30, crystal: 0 },
  MammothTank: { ore: 350, steel: 180, coal: 60, crystal: 0 },
  Gunship: { ore: 250, steel: 100, coal: 80, crystal: 0 },
  Interceptor: { ore: 200, steel: 80, coal: 100, crystal: 0 },
  SamLauncher: { ore: 180, steel: 100, coal: 50, crystal: 0 },
};

export function resourceBundleTotal(bundle: ResourceBundle): number {
  return bundle.ore + bundle.steel + bundle.coal + bundle.crystal;
}

export function resourceBundleAffordable(resources: ResourceBundle, cost: ResourceBundle): boolean {
  return resources.ore >= cost.ore
    && resources.steel >= cost.steel
    && resources.coal >= cost.coal
    && resources.crystal >= cost.crystal;
}

/** Compact Civ-style cost label used by command cards. */
export function formatResourceCost(cost: ResourceBundle): string {
  const parts: string[] = [];
  if (cost.ore > 0) parts.push(`${cost.ore} O`);
  if (cost.steel > 0) parts.push(`${cost.steel} S`);
  if (cost.coal > 0) parts.push(`${cost.coal} C`);
  if (cost.crystal > 0) parts.push(`${cost.crystal} X`);
  return parts.join(" · ");
}

/** Static tech-tree metadata for the research overlay. Mirrors the sim's
 *  tech.rs (the server validates; this is presentation only). */
export const TECH_INFO: Record<
  TechId,
  { name: string; description: string; researchCost: number; crystalCost: number; prereqs: TechId[] }
> = {
  HighExplosive: {
    name: "High-Explosive Payloads",
    description: "Army-wide +25% attack damage.",
    researchCost: 150,
    crystalCost: 0,
    prereqs: [],
  },
  CompositeArmor: {
    name: "Composite Armor",
    description: "Army-wide +25% hit points.",
    researchCost: 150,
    crystalCost: 0,
    prereqs: [],
  },
  TargetingOptics: {
    name: "Targeting Optics",
    description: "All units gain +1 attack range.",
    researchCost: 200,
    crystalCost: 0,
    prereqs: [],
  },
  EfficientRefining: {
    name: "Efficient Refining",
    description: "Refineries and crystal refineries yield +25%.",
    researchCost: 120,
    crystalCost: 0,
    prereqs: [],
  },
  RocketPropulsion: {
    name: "Rocket Propulsion",
    description: "Unlocks the Rocket Trooper and SAM launcher.",
    researchCost: 350,
    crystalCost: 0,
    prereqs: ["HighExplosive"],
  },
  TitaniumAlloys: {
    name: "Titanium Alloys",
    description: "Army-wide +25% hit points (stacks with Composite Armor).",
    researchCost: 400,
    crystalCost: 0,
    prereqs: ["CompositeArmor"],
  },
  AerialSuperiority: {
    name: "Aerial Superiority",
    description: "Air units deal +25% damage and gain +1 range.",
    researchCost: 350,
    crystalCost: 0,
    prereqs: ["HighExplosive"],
  },
  Superconductors: {
    name: "Superconductors",
    description: "Power plants generate +50% power.",
    researchCost: 300,
    crystalCost: 0,
    prereqs: ["EfficientRefining"],
  },
  CrystalNanotech: {
    name: "Crystal Nanotech",
    description: "Army-wide +25% hit points and damage. Requires crystal.",
    researchCost: 600,
    crystalCost: 20,
    prereqs: ["TitaniumAlloys"],
  },
  AdvancedBallistics: {
    name: "Advanced Ballistics",
    description: "All units gain +1 attack range (stacks with Optics).",
    researchCost: 600,
    crystalCost: 0,
    prereqs: ["TargetingOptics"],
  },
};

export const UNIT_KINDS = new Set([
  "Infantry",
  "Scout",
  "RocketTrooper",
  "Tank",
  "Artillery",
  "MammothTank",
  "Gunship",
  "Interceptor",
  "SamLauncher",
]);
export const BUILDING_KINDS = new Set([
  "Hq",
  "PowerPlant",
  "Refinery",
  "CrystalRefinery",
  "Barracks",
  "Factory",
  "TechLab",
  "Airfield",
  "Radar",
  "TeslaCoil",
  "Turret",
  "AATurret",
]);
