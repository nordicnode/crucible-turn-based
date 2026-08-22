// Wire types mirroring the server's JSON protocol, plus command builders.
// The client never implements game rules; it only renders fogged state and
// sends the same commands the sim validates.

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

// The server's serde format serializes `Player` as the variant name ("P0" /
// "P1"), so commands must carry the string, not an index.
export type Player = "P0" | "P1";

export type Command =
  | { PlaceBuilding: { player: Player; btype: BuildingType; tile: [number, number] } }
  | { TrainUnit: { player: Player; building: number; utype: UnitType } }
  | { MoveGroup: { player: Player; units: number[]; waypoint: [number, number] } }
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
  turn: number;
  kind: string;
  /** Amount for `ore_mined` / `sold` events (undefined otherwise). */
  amount?: number;
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
      hq: [number, number][];
    }
  | {
      type: "stateDiff";
      turn: number;
      activePlayer: number;
      ore: number;
      crystal: number;
      powerProduced?: number;
      powerConsumed?: number;
      research: ResearchMsg;
      entities: DiffEntity[];
      oreTiles: OreTile[];
      crystalTiles: CrystalTile[];
      visible: number[];
      events: DiffEvent[];
    }
  | { type: "commandRejected"; index: number; reason: string }
  | { type: "matchEnd"; winner: number | null; reason: string | null; durationTurns: number; replayId: number | null }
  | { type: "serverBusy" };

export type ClientMsg =
  | { type: "joinMatch"; opponent: string }
  | { type: "commands"; cmds: Command[] }
  | { type: "endTurn" };

export const BUILD_COSTS: Record<string, number> = {
  PowerPlant: 150,
  Refinery: 300,
  CrystalRefinery: 350,
  Barracks: 150,
  Factory: 250,
  TechLab: 200,
  Airfield: 250,
  Radar: 150,
  TeslaCoil: 250,
  Turret: 100,
  AATurret: 200,
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

export const UNIT_COSTS: Record<string, number> = {
  Infantry: 50,
  Scout: 40,
  RocketTrooper: 120,
  Tank: 150,
  Artillery: 200,
  MammothTank: 350,
  Gunship: 250,
  Interceptor: 200,
  SamLauncher: 180,
};

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
