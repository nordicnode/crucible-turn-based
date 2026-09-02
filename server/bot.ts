// Opponent AI bots for Crucible: Easy, Medium, Hard, and Champion (Neural Apex).

import { UNIT_STATS, type BuildingType, type Command, type Player, type UnitType } from "../client/src/types";
import type { GameMatch, SimEntity } from "./sim";

export interface Bot {
  name: string;
  act(game: GameMatch): void;
}

export class CrucibleBot implements Bot {
  name: string;
  difficulty: "easy" | "medium" | "hard" | "champion";

  constructor(difficulty: "easy" | "medium" | "hard" | "champion") {
    this.difficulty = difficulty;
    this.name = difficulty === "champion" ? "Neural Apex (Gen 4)" : `${difficulty.toUpperCase()} AI`;
  }

  act(game: GameMatch): void {
    const player = 1;
    const botPlayer: Player = "P1";
    const botHq = Array.from(game.entities.values()).find((e) => e.owner === player && e.kind === "Hq");
    if (!botHq) return;

    const res = game.resources[player];
    const power = game.getPower(player);

    // 1. Economic / Structure expansion
    const buildings = Array.from(game.entities.values()).filter((e) => e.owner === player && !(e.kind in UNIT_STATS));
    const hasBarracks = buildings.some((b) => b.kind === "Barracks");
    const hasFactory = buildings.some((b) => b.kind === "Factory");

    // Build PowerPlant if power is deficient
    if (power.consumed >= power.produced && res.ore >= 120) {
      this.tryPlaceNear(game, player, "PowerPlant", botHq);
    }

    // Build Refinery on nearby resource
    for (const deposit of game.map.resourceTiles.values()) {
      const dist = Math.hypot(deposit.x - botHq.x, deposit.y - botHq.y);
      if (dist < 12 && deposit.refineryOwner === null && res.ore >= 150) {
        const btype: BuildingType = deposit.resource === "Crystal" ? "CrystalRefinery" : "Refinery";
        const cmd: Command = {
          PlaceBuilding: {
            player: botPlayer,
            btype,
            tile: [deposit.x, deposit.y],
          },
        };
        game.applyCommands(player, [cmd]);
        break;
      }
    }

    // Build Barracks
    if (!hasBarracks && res.ore >= 200) {
      this.tryPlaceNear(game, player, "Barracks", botHq);
    }

    // Advanced builds for Hard and Champion
    if ((this.difficulty === "hard" || this.difficulty === "champion") && hasBarracks) {
      if (!hasFactory && res.ore >= 300) {
        this.tryPlaceNear(game, player, "Factory", botHq);
      }
      // Defensive Turret
      const turretCount = buildings.filter((b) => b.kind === "Turret" || b.kind === "TeslaCoil").length;
      if (turretCount < (this.difficulty === "champion" ? 3 : 2) && res.ore >= 150) {
        this.tryPlaceNear(game, player, this.difficulty === "champion" ? "TeslaCoil" : "Turret", botHq);
      }
    }

    // 2. Unit Training
    for (const b of buildings) {
      if (b.queue.length === 0) {
        if (b.kind === "Barracks") {
          const unitKind: UnitType = this.difficulty === "easy"
            ? "Infantry"
            : Math.random() < 0.35
            ? "RocketTrooper"
            : "Infantry";
          const cmd: Command = { TrainUnit: { player: botPlayer, building: b.id, utype: unitKind } };
          game.applyCommands(player, [cmd]);
        } else if (b.kind === "Factory") {
          const tankKind: UnitType = this.difficulty === "champion" && Math.random() < 0.4 ? "MammothTank" : "Tank";
          const cmd: Command = { TrainUnit: { player: botPlayer, building: b.id, utype: tankKind } };
          game.applyCommands(player, [cmd]);
        }
      }
    }

    // 3. Unit Movement & Combat
    const botUnits = Array.from(game.entities.values()).filter((e) => e.owner === player && e.kind in UNIT_STATS);
    const humanEntities = Array.from(game.entities.values()).filter((e) => e.owner === 0);
    const humanHq = humanEntities.find((e) => e.kind === "Hq");

    for (const unit of botUnits) {
      if (unit.mp <= 0) continue;

      // Find enemies within attack range
      const stat = UNIT_STATS[unit.kind];
      const range = stat?.range_tiles ?? 1;

      let inRangeEnemy: SimEntity | null = null;
      let closestEnemy: SimEntity | null = null;
      let minDistance = Infinity;

      for (const enemy of humanEntities) {
        const d = Math.hypot(unit.x - enemy.x, unit.y - enemy.y);
        if (d <= range + 0.5) {
          inRangeEnemy = enemy;
          break;
        }
        if (d < minDistance) {
          minDistance = d;
          closestEnemy = enemy;
        }
      }

      // Attack if in range
      if (inRangeEnemy) {
        const cmd: Command = { Attack: { player: botPlayer, units: [unit.id], target: inRangeEnemy.id } };
        game.applyCommands(player, [cmd]);
      } else {
        // Move towards closest enemy or human HQ
        const target = closestEnemy || humanHq;
        if (target) {
          const cmd: Command = {
            MoveGroup: {
              player: botPlayer,
              units: [unit.id],
              waypoint: [Math.floor(target.x), Math.floor(target.y)],
            },
          };
          game.applyCommands(player, [cmd]);
        }
      }
    }
  }

  private tryPlaceNear(game: GameMatch, player: number, btype: BuildingType, anchor: SimEntity): void {
    const offsets = [
      [2, 0], [0, 2], [-2, 0], [0, -2],
      [2, 2], [-2, 2], [2, -2], [-2, -2],
      [3, 1], [1, 3], [-3, 1], [1, -3],
    ];
    for (const [dx, dy] of offsets) {
      const tx = Math.floor(anchor.x) + dx;
      const ty = Math.floor(anchor.y) + dy;
      if (tx >= 0 && tx < 128 && ty >= 0 && ty < 128) {
        const cmd: Command = { PlaceBuilding: { player: "P1", btype, tile: [tx, ty] as [number, number] } };
        game.applyCommands(player, [cmd]);
        // If building was successfully placed, break
        const placed = Array.from(game.entities.values()).some((e) => Math.floor(e.x) === tx && Math.floor(e.y) === ty);
        if (placed) break;
      }
    }
  }
}

export function createBot(opponentName: string): Bot {
  const norm = opponentName.toLowerCase();
  if (norm.startsWith("adaptive:")) {
    const diff = parseFloat(norm.split(":")[1]);
    if (diff < 0.4) return new CrucibleBot("easy");
    if (diff < 0.7) return new CrucibleBot("medium");
    return new CrucibleBot("hard");
  }
  if (norm.includes("easy")) return new CrucibleBot("easy");
  if (norm.includes("medium")) return new CrucibleBot("medium");
  if (norm.includes("champion") || norm.includes("neural") || norm.includes("museum")) return new CrucibleBot("champion");
  return new CrucibleBot("hard");
}
