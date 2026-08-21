// Wire types mirroring the server's JSON protocol, plus command builders.
// The client never implements game rules; it only renders fogged state and
// sends the same commands the sim validates.

export type BuildingType = "Hq" | "PowerPlant" | "Refinery" | "Barracks" | "Factory" | "TechLab" | "Airfield" | "Radar" | "TeslaCoil" | "Turret";
export type UnitType = "Infantry" | "Tank" | "Artillery" | "MammothTank" | "Gunship" | "Interceptor";
export type Upgrade = "None" | "Damage" | "Hp" | "Range";

// The server's serde format serializes `Player` as the variant name ("P0" /
// "P1"), so commands must carry the string, not an index.
export type Player = "P0" | "P1";

export type Command =
  | { PlaceBuilding: { player: Player; btype: BuildingType; tile: [number, number] } }
  | { TrainUnit: { player: Player; building: number; utype: UnitType } }
  | { MoveGroup: { player: Player; units: number[]; waypoint: [number, number] } }
  | { Attack: { player: Player; units: number[]; target: number } }
  | { ChooseUpgrade: { player: Player; lab: number; upgrade: Upgrade } }
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
export function chooseUpgrade(lab: number, upgrade: Upgrade): Command {
  return { ChooseUpgrade: { player: PLAYER, lab, upgrade } };
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

export interface DiffEvent {
  turn: number;
  kind: string;
  /** Amount for `ore_mined` / `sold` events (undefined otherwise). */
  amount?: number;
  /** Player index (0 = P0/friendly, 1 = P1/enemy). */
  player?: number;
}

export type ServerMsg =
  | { type: "matchStart"; mapSeed: number; player: number; passable: boolean[]; hq: [number, number][] }
  | {
      type: "stateDiff";
      turn: number;
      activePlayer: number;
      ore: number;
      powerProduced?: number;
      powerConsumed?: number;
      upgrade?: Upgrade;
      entities: DiffEntity[];
      oreTiles: OreTile[];
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
  Barracks: 150,
  Factory: 250,
  TechLab: 200,
  Airfield: 250,
  Radar: 150,
  TeslaCoil: 250,
  Turret: 100,
};

export const BUILDING_POWER: Record<string, { produces: number; consumes: number }> = {
  Hq: { produces: 50, consumes: 0 },
  PowerPlant: { produces: 100, consumes: 0 },
  Refinery: { produces: 0, consumes: 20 },
  Barracks: { produces: 0, consumes: 15 },
  Factory: { produces: 0, consumes: 25 },
  TechLab: { produces: 0, consumes: 30 },
  Airfield: { produces: 0, consumes: 25 },
  Radar: { produces: 0, consumes: 10 },
  TeslaCoil: { produces: 0, consumes: 30 },
  Turret: { produces: 0, consumes: 20 },
};

export const UNIT_COSTS: Record<string, number> = {
  Infantry: 50,
  Tank: 150,
  Artillery: 200,
  MammothTank: 350,
  Gunship: 250,
  Interceptor: 200,
};

export const UNIT_KINDS = new Set(["Infantry", "Tank", "Artillery", "MammothTank", "Gunship", "Interceptor"]);
export const BUILDING_KINDS = new Set(["Hq", "PowerPlant", "Refinery", "Barracks", "Factory", "TechLab", "Airfield", "Radar", "TeslaCoil", "Turret"]);
