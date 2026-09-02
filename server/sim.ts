// Authoritative simulation engine for Crucible turn-based tactical matches.

import {
  MAP_SIZE,
  MAP_TILES,
  BUILD_COSTS,
  BUILD_STATS,
  BUILDING_POWER,
  BUILDING_PREREQS,
  UNIT_COSTS,
  UNIT_STATS,
  UNIT_TREE,
  DEFAULT_TERRAIN_RULES,
  type BuildingType,
  type Command,
  type DiffEntity,
  type DiffEvent,
  type Player,
  type ResourceBundle,
  type ResourceTile,
  type ResourceType,
  type TechId,
  type TileInspection,
  type UnitType,
} from "../client/src/types";

export interface SimEntity {
  id: number;
  kind: string;
  owner: number; // 0 = human, 1 = bot
  x: number; // tile center (x + 0.5)
  y: number; // tile center (y + 0.5)
  hp: number;
  maxHp: number;
  mp: number;
  maxMp: number;
  queue: string[];
  progress: number;
  buildTime: number;
  constructionProgress: number;
  constructionTime: number;
  rally: [number, number] | null;
  moveTarget: [number, number] | null;
  movementPath: [number, number][] | null;
}

export interface SimMap {
  seed: number;
  terrain: string[];
  passable: boolean[];
  elevation: number[];
  moisture: number[];
  temperature: number[];
  hq: [[number, number], [number, number]];
  resourceTiles: Map<string, ResourceTile>;
}

export function generateMap(seed: number): SimMap {
  const terrain: string[] = new Array(MAP_TILES);
  const passable: boolean[] = new Array(MAP_TILES);
  const elevation: number[] = new Array(MAP_TILES);
  const moisture: number[] = new Array(MAP_TILES);
  const temperature: number[] = new Array(MAP_TILES);

  const hash = (x: number, y: number, salt: number): number => {
    let z = ((x * 2654435761 + y * 40503 + salt * 74996233) ^ (seed >>> 0)) >>> 0;
    z = (z ^ (z >> 13)) * 1274126143;
    return (z ^ (z >> 16)) >>> 0;
  };

  const noise = (x: number, y: number, salt: number): number => {
    const cell = 8;
    const gx = Math.floor(x / cell);
    const gy = Math.floor(y / cell);
    const fx = (x - gx * cell) / cell;
    const fy = (y - gy * cell) / cell;
    const a = hash(gx, gy, salt);
    const b = hash(gx + 1, gy, salt);
    const c = hash(gx, gy + 1, salt);
    const d = hash(gx + 1, gy + 1, salt);
    const top = a + (b - a) * fx;
    const bot = c + (d - c) * fx;
    return ((top + (bot - top) * fy) >>> 8) % 256;
  };

  const riverY = (x: number): number => Math.floor(MAP_SIZE / 2 + Math.sin(x / 14) * 10);

  for (let y = 0; y < MAP_SIZE; y++) {
    for (let x = 0; x < MAP_SIZE; x++) {
      const idx = y * MAP_SIZE + x;
      const elev = noise(x, y, 0x11);
      const moist = noise(x, y, 0x22);
      const temp = (40 + (32 - Math.abs(y - 32)) * 5 + (hash(x, y, 0x33) % 40) - 20) & 0xff;

      elevation[idx] = elev;
      moisture[idx] = moist;
      temperature[idx] = temp;

      let kind = "Plains";
      if (Math.abs(y - riverY(x)) <= 1) {
        kind = "River";
      } else if (elev > 205) {
        kind = "Mountain";
      } else if (elev > 175 && moist < 140) {
        kind = "Hills";
      } else if (moist > 200 && temp > 130) {
        kind = "Swamp";
      } else if (moist < 70 && temp > 90) {
        kind = "Desert";
      } else if (moist > 150) {
        kind = "Forest";
      }

      terrain[idx] = kind;
      passable[idx] = kind !== "Mountain";
    }
  }

  // Clear HQ areas
  const hq0: [number, number] = [24, 24];
  const hq1: [number, number] = [104, 104];
  for (const h of [hq0, hq1]) {
    for (let dy = -3; dy <= 3; dy++) {
      for (let dx = -3; dx <= 3; dx++) {
        const nx = h[0] + dx;
        const ny = h[1] + dy;
        if (nx >= 0 && nx < MAP_SIZE && ny >= 0 && ny < MAP_SIZE) {
          const idx = ny * MAP_SIZE + nx;
          terrain[idx] = "Plains";
          passable[idx] = true;
        }
      }
    }
  }

  // Place resource deposits
  const resourceTiles = new Map<string, ResourceTile>();
  const addDeposit = (x: number, y: number, resource: ResourceType, richness = 2) => {
    if (x < 0 || x >= MAP_SIZE || y < 0 || y >= MAP_SIZE) return;
    const key = `${x},${y}`;
    resourceTiles.set(key, {
      x,
      y,
      resource,
      amount: 500,
      richness,
      infinite: true,
      yieldPerTurn: richness * 20,
      refineryOwner: null,
    });
  };

  // Near HQ 0
  addDeposit(22, 28, "Ore", 3);
  addDeposit(28, 22, "Steel", 2);
  addDeposit(30, 30, "Coal", 2);
  addDeposit(18, 20, "Crystal", 1);

  // Near HQ 1
  addDeposit(106, 100, "Ore", 3);
  addDeposit(100, 106, "Steel", 2);
  addDeposit(98, 98, "Coal", 2);
  addDeposit(110, 108, "Crystal", 1);

  // Contested central resources
  addDeposit(60, 60, "Ore", 3);
  addDeposit(68, 68, "Ore", 3);
  addDeposit(64, 56, "Crystal", 3);
  addDeposit(56, 64, "Crystal", 3);
  addDeposit(72, 58, "Steel", 2);
  addDeposit(58, 72, "Coal", 2);

  return {
    seed,
    terrain,
    passable,
    elevation,
    moisture,
    temperature,
    hq: [hq0, hq1],
    resourceTiles,
  };
}

export class GameMatch {
  map: SimMap;
  turn = 0;
  round = 1;
  activePlayer = 0;
  nextEntityId = 1;

  entities = new Map<number, SimEntity>();
  resources: [ResourceBundle, ResourceBundle] = [
    { ore: 500, steel: 120, coal: 100, crystal: 50 },
    { ore: 500, steel: 120, coal: 100, crystal: 50 },
  ];

  research: [{ points: number; researching: TechId | null; researched: TechId[] }, { points: number; researching: TechId | null; researched: TechId[] }] = [
    { points: 0, researching: null, researched: [] },
    { points: 0, researching: null, researched: [] },
  ];

  exploredP0 = new Set<number>();
  visibleP0 = new Set<number>();

  events: DiffEvent[] = [];
  winner: number | null = null;
  winReason: string | null = null;

  constructor(seed = Math.floor(Math.random() * 1000000)) {
    this.map = generateMap(seed);
    this.initEntities();
    this.updateVisibility();
  }

  private initEntities(): void {
    // Player 0 HQ
    const hq0 = this.addEntity("Hq", 0, this.map.hq[0][0], this.map.hq[0][1]);
    // Starting units for P0
    this.addEntity("Infantry", 0, hq0.x + 1, hq0.y);
    this.addEntity("Infantry", 0, hq0.x, hq0.y + 1);
    this.addEntity("Scout", 0, hq0.x + 2, hq0.y + 1);

    // Player 1 HQ
    const hq1 = this.addEntity("Hq", 1, this.map.hq[1][0], this.map.hq[1][1]);
    // Starting units for P1
    this.addEntity("Infantry", 1, hq1.x - 1, hq1.y);
    this.addEntity("Infantry", 1, hq1.x, hq1.y - 1);
    this.addEntity("Scout", 1, hq1.x - 2, hq1.y - 1);
  }

  addEntity(kind: string, owner: number, x: number, y: number): SimEntity {
    const id = this.nextEntityId++;
    const isUnit = kind in UNIT_STATS;
    const uStat = UNIT_STATS[kind];
    const bStat = BUILD_STATS[kind];

    const hp = isUnit ? uStat.hp : bStat?.hp ?? 500;
    const mp = isUnit ? uStat.mp : 0;

    const e: SimEntity = {
      id,
      kind,
      owner,
      x: Math.floor(x) + 0.5,
      y: Math.floor(y) + 0.5,
      hp,
      maxHp: hp,
      mp,
      maxMp: mp,
      queue: [],
      progress: 0,
      buildTime: 0,
      constructionProgress: 0,
      constructionTime: 0,
      rally: null,
      moveTarget: null,
      movementPath: null,
    };
    this.entities.set(id, e);
    return e;
  }

  getPower(player: number): { produced: number; consumed: number } {
    let produced = 0;
    let consumed = 0;
    for (const e of this.entities.values()) {
      if (e.owner === player && e.kind in BUILDING_POWER) {
        const p = BUILDING_POWER[e.kind];
        produced += p.produces;
        consumed += p.consumes;
      }
    }
    return { produced, consumed };
  }

  getIncome(player: number): ResourceBundle {
    let ore = 25; // HQ trickle
    let steel = 0;
    let coal = 0;
    let crystal = 0;

    for (const e of this.entities.values()) {
      if (e.owner === player && (e.kind === "Refinery" || e.kind === "CrystalRefinery")) {
        const tx = Math.floor(e.x);
        const ty = Math.floor(e.y);
        const res = this.map.resourceTiles.get(`${tx},${ty}`);
        if (res) {
          const yieldAmount = (res.richness || 1) * 25;
          if (res.resource === "Ore") ore += yieldAmount;
          if (res.resource === "Steel") steel += yieldAmount;
          if (res.resource === "Coal") coal += yieldAmount;
          if (res.resource === "Crystal") crystal += yieldAmount;
        }
      }
    }
    return { ore, steel, coal, crystal };
  }

  updateVisibility(): void {
    this.visibleP0.clear();
    for (const e of this.entities.values()) {
      if (e.owner === 0) {
        const isUnit = e.kind in UNIT_STATS;
        const vision = isUnit
          ? UNIT_STATS[e.kind]?.vision_tiles ?? 4
          : BUILD_STATS[e.kind]?.vision_tiles ?? 5;
        const cx = Math.floor(e.x);
        const cy = Math.floor(e.y);

        for (let dy = -vision; dy <= vision; dy++) {
          for (let dx = -vision; dx <= vision; dx++) {
            if (dx * dx + dy * dy <= vision * vision) {
              const nx = cx + dx;
              const ny = cy + dy;
              if (nx >= 0 && nx < MAP_SIZE && ny >= 0 && ny < MAP_SIZE) {
                const idx = ny * MAP_SIZE + nx;
                this.visibleP0.add(idx);
                this.exploredP0.add(idx);
              }
            }
          }
        }
      }
    }
  }

  applyCommands(player: number, cmds: Command[]): void {
    for (const cmd of cmds) {
      if ("PlaceBuilding" in cmd) {
        const { btype, tile } = cmd.PlaceBuilding;
        this.executePlaceBuilding(player, btype, tile);
      } else if ("TrainUnit" in cmd) {
        const { building, utype } = cmd.TrainUnit;
        this.executeTrainUnit(player, building, utype);
      } else if ("SetRally" in cmd) {
        const { building, waypoint } = cmd.SetRally;
        const b = this.entities.get(building);
        if (b && b.owner === player) b.rally = waypoint;
      } else if ("MoveGroup" in cmd) {
        const { units, waypoint } = cmd.MoveGroup;
        for (const uid of units) {
          const u = this.entities.get(uid);
          if (u && u.owner === player) {
            this.executeMoveUnit(u, waypoint);
          }
        }
      } else if ("ClearMove" in cmd) {
        const { units } = cmd.ClearMove;
        for (const uid of units) {
          const u = this.entities.get(uid);
          if (u && u.owner === player) {
            u.moveTarget = null;
            u.movementPath = null;
          }
        }
      } else if ("Attack" in cmd) {
        const { units, target } = cmd.Attack;
        const targetEntity = this.entities.get(target);
        if (targetEntity && targetEntity.owner !== player) {
          for (const uid of units) {
            const u = this.entities.get(uid);
            if (u && u.owner === player) {
              this.executeAttack(u, targetEntity);
            }
          }
        }
      } else if ("StartResearch" in cmd) {
        const { tech } = cmd.StartResearch;
        this.research[player].researching = tech;
      } else if ("Sell" in cmd) {
        const { building } = cmd.Sell;
        const b = this.entities.get(building);
        if (b && b.owner === player && b.kind !== "Hq") {
          const cost = BUILD_COSTS[b.kind] ?? { ore: 100, steel: 0, coal: 0, crystal: 0 };
          this.resources[player].ore += Math.floor(cost.ore * 0.5);
          this.entities.delete(b.id);
          this.events.push({ turn: this.turn, round: this.round, kind: "sold", amount: Math.floor(cost.ore * 0.5), player });
        }
      } else if ("Repair" in cmd) {
        const { building } = cmd.Repair;
        const b = this.entities.get(building);
        if (b && b.owner === player && b.hp < b.maxHp && this.resources[player].ore >= 25) {
          this.resources[player].ore -= 25;
          b.hp = Math.min(b.maxHp, b.hp + 100);
        }
      }
    }
    this.updateVisibility();
  }

  private executePlaceBuilding(player: number, btype: BuildingType, tile: [number, number]): boolean {
    const cost = BUILD_COSTS[btype];
    if (!cost) return false;
    const res = this.resources[player];
    if (res.ore < cost.ore || res.steel < cost.steel || res.coal < cost.coal || res.crystal < cost.crystal) {
      return false;
    }

    const [x, y] = tile;
    if (x < 0 || x >= MAP_SIZE || y < 0 || y >= MAP_SIZE) return false;
    const idx = y * MAP_SIZE + x;
    if (!this.map.passable[idx]) return false;

    // Must be near existing owned building (within 5 tiles)
    const hasNearbyBuilding = Array.from(this.entities.values()).some(
      (e) => e.owner === player && !(e.kind in UNIT_STATS) && Math.max(Math.abs(Math.floor(e.x) - x), Math.abs(Math.floor(e.y) - y)) <= 5
    );
    if (!hasNearbyBuilding) return false;

    // Check tile occupancy
    const isOccupied = Array.from(this.entities.values()).some(
      (e) => Math.floor(e.x) === x && Math.floor(e.y) === y
    );
    if (isOccupied) return false;

    // Refinery must be on a resource tile
    const depositKey = `${x},${y}`;
    const deposit = this.map.resourceTiles.get(depositKey);
    if (btype === "Refinery" || btype === "CrystalRefinery") {
      if (!deposit) return false;
      deposit.refineryOwner = player;
    } else if (deposit) {
      return false; // Can't build non-refinery on a deposit
    }

    // Deduct cost
    res.ore -= cost.ore;
    res.steel -= cost.steel;
    res.coal -= cost.coal;
    res.crystal -= cost.crystal;

    this.addEntity(btype, player, x, y);
    return true;
  }

  private executeTrainUnit(player: number, buildingId: number, utype: UnitType): boolean {
    const b = this.entities.get(buildingId);
    if (!b || b.owner !== player) return false;

    const cost = UNIT_COSTS[utype];
    if (!cost) return false;
    const res = this.resources[player];
    if (res.ore < cost.ore || res.steel < cost.steel || res.coal < cost.coal || res.crystal < cost.crystal) {
      return false;
    }

    res.ore -= cost.ore;
    res.steel -= cost.steel;
    res.coal -= cost.coal;
    res.crystal -= cost.crystal;

    b.queue.push(utype);
    if (b.queue.length === 1) {
      b.progress = 0;
      b.buildTime = UNIT_STATS[utype]?.build_time_turns ?? 1;
    }
    return true;
  }

  private executeMoveUnit(u: SimEntity, target: [number, number]): void {
    if (u.mp <= 0) return;
    const [tx, ty] = target;
    const curX = Math.floor(u.x);
    const curY = Math.floor(u.y);

    let stepX = curX;
    let stepY = curY;
    let stepsLeft = u.mp;

    while (stepsLeft > 0 && (stepX !== tx || stepY !== ty)) {
      const dx = Math.sign(tx - stepX);
      const dy = Math.sign(ty - stepY);

      let nextX = stepX + dx;
      let nextY = stepY;
      let idx = nextY * MAP_SIZE + nextX;

      if (!this.map.passable[idx] || (dx !== 0 && dy !== 0 && Math.abs(tx - stepX) < Math.abs(ty - stepY))) {
        nextX = stepX;
        nextY = stepY + dy;
        idx = nextY * MAP_SIZE + nextX;
      }

      if (this.map.passable[idx]) {
        stepX = nextX;
        stepY = nextY;
        stepsLeft--;
      } else {
        break;
      }
    }

    u.x = stepX + 0.5;
    u.y = stepY + 0.5;
    u.mp = stepsLeft;
    u.moveTarget = [tx, ty];
  }

  private executeAttack(attacker: SimEntity, target: SimEntity): void {
    const stat = UNIT_STATS[attacker.kind] ?? BUILD_STATS[attacker.kind];
    if (!stat || !stat.damage) return;

    const dist = Math.hypot(attacker.x - target.x, attacker.y - target.y);
    const range = stat.range_tiles ?? 1;
    if (dist > range + 0.5) return;

    target.hp -= stat.damage;
    this.events.push({
      turn: this.turn,
      round: this.round,
      kind: "attacked",
      attacker: attacker.id,
      target: target.id,
      amount: stat.damage,
      player: attacker.owner,
    });

    if (target.hp <= 0) {
      this.entities.delete(target.id);
      this.events.push({
        turn: this.turn,
        round: this.round,
        kind: "destroyed",
        target: target.id,
        player: target.owner,
      });

      if (target.kind === "Hq") {
        this.winner = attacker.owner;
        this.winReason = "HqDestroyed";
      }
    }
  }

  stepLifecycle(player: number): void {
    // Add income
    const inc = this.getIncome(player);
    const res = this.resources[player];
    res.ore += inc.ore;
    res.steel += inc.steel;
    res.coal += inc.coal;
    res.crystal += inc.crystal;

    // Advance production
    for (const e of this.entities.values()) {
      if (e.owner === player) {
        // Reset MP for own units
        if (e.kind in UNIT_STATS) {
          e.mp = e.maxMp;
        }

        // Production queue
        if (e.queue.length > 0) {
          e.progress++;
          if (e.progress >= (e.buildTime || 1)) {
            const finishedUnit = e.queue.shift()!;
            const spawnX = e.rally ? e.rally[0] : Math.floor(e.x) + 1;
            const spawnY = e.rally ? e.rally[1] : Math.floor(e.y);
            this.addEntity(finishedUnit, player, spawnX, spawnY);
            e.progress = 0;
            if (e.queue.length > 0) {
              e.buildTime = UNIT_STATS[e.queue[0]]?.build_time_turns ?? 1;
            }
          }
        }
      }
    }

    // Passive defensive fire (Turrets, TeslaCoil)
    for (const e of this.entities.values()) {
      if (e.owner === player && (e.kind === "Turret" || e.kind === "TeslaCoil" || e.kind === "AATurret")) {
        const stat = BUILD_STATS[e.kind];
        if (stat && stat.damage > 0) {
          for (const target of this.entities.values()) {
            if (target.owner !== player) {
              const d = Math.hypot(e.x - target.x, e.y - target.y);
              if (d <= stat.range_tiles + 0.5) {
                this.executeAttack(e, target);
                break;
              }
            }
          }
        }
      }
    }
  }

  endHumanTurn(): void {
    this.stepLifecycle(0);
    this.turn++;
  }

  endBotTurn(): void {
    this.stepLifecycle(1);
    this.turn++;
    this.round++;

    // Max turns timeout check
    if (this.turn >= 160 && this.winner === null) {
      let army0 = 0;
      let army1 = 0;
      for (const e of this.entities.values()) {
        const val = e.maxHp;
        if (e.owner === 0) army0 += val;
        else army1 += val;
      }
      this.winner = army0 >= army1 ? 0 : 1;
      this.winReason = "TurnLimitReached";
    }

    this.updateVisibility();
  }

  getTileInspection(x: number, y: number): TileInspection {
    const idx = y * MAP_SIZE + x;
    const isVisible = this.visibleP0.has(idx);
    const isExplored = this.exploredP0.has(idx);
    const visibility = isVisible ? "visible" : isExplored ? "explored" : "unexplored";

    const terrainKind = this.map.terrain[idx] ?? "Plains";
    const terrainRule = DEFAULT_TERRAIN_RULES.find((r) => r.kind === terrainKind) ?? DEFAULT_TERRAIN_RULES[0];
    const resource = this.map.resourceTiles.get(`${x},${y}`) ?? null;

    const occupants: DiffEntity[] = [];
    let refinery: DiffEntity | null = null;
    for (const e of this.entities.values()) {
      if (Math.floor(e.x) === x && Math.floor(e.y) === y) {
        if (e.owner === 0 || isVisible) {
          const de: DiffEntity = {
            id: e.id,
            kind: e.kind,
            owner: e.owner,
            x: e.x,
            y: e.y,
            hp: e.hp,
            maxHp: e.maxHp,
            mp: e.mp,
            maxMp: e.maxMp,
          };
          occupants.push(de);
          if (e.kind === "Refinery" || e.kind === "CrystalRefinery") refinery = de;
        }
      }
    }

    const hasBuilding = occupants.some((o) => !(o.kind in UNIT_STATS));
    const hasUnit = occupants.some((o) => o.kind in UNIT_STATS);
    const withinBase = Array.from(this.entities.values()).some(
      (e) => e.owner === 0 && !(e.kind in UNIT_STATS) && Math.max(Math.abs(Math.floor(e.x) - x), Math.abs(Math.floor(e.y) - y)) <= 5
    );

    return {
      x,
      y,
      index: idx,
      visibility,
      terrain: isExplored ? terrainRule : null,
      elevation: isExplored ? this.map.elevation[idx] : null,
      moisture: isExplored ? this.map.moisture[idx] : null,
      temperature: isExplored ? this.map.temperature[idx] : null,
      resource: isExplored ? resource : null,
      occupants,
      movement: [],
      routeTargets: [],
      placement: {
        known: isExplored,
        passable: this.map.passable[idx],
        occupiedByBuilding: hasBuilding,
        occupiedByUnit: hasUnit,
        resource: resource?.resource ?? null,
        withinBaseRadius: withinBase,
        structureSiteAvailable: isExplored && this.map.passable[idx] && !hasBuilding && !hasUnit && !resource && withinBase,
        refinerySiteAvailable: isExplored && this.map.passable[idx] && !hasBuilding && resource !== null,
      },
    };
  }

  getDiffForP0(): any {
    const visibleEntities: DiffEntity[] = [];
    for (const e of this.entities.values()) {
      const idx = Math.floor(e.y) * MAP_SIZE + Math.floor(e.x);
      if (e.owner === 0 || this.visibleP0.has(idx)) {
        visibleEntities.push({
          id: e.id,
          kind: e.kind,
          owner: e.owner,
          x: e.x,
          y: e.y,
          hp: e.hp,
          maxHp: e.maxHp,
          mp: e.mp,
          maxMp: e.maxMp,
          queue: e.queue,
          progress: e.progress,
          buildTime: e.buildTime,
          moveTarget: e.moveTarget,
          movementPath: e.movementPath,
          rally: e.rally,
        });
      }
    }

    const visibleResources: ResourceTile[] = [];
    for (const res of this.map.resourceTiles.values()) {
      const idx = res.y * MAP_SIZE + res.x;
      if (this.exploredP0.has(idx)) {
        visibleResources.push(res);
      }
    }

    const pwr = this.getPower(0);
    const inc = this.getIncome(0);
    const res = this.resources[0];

    return {
      type: "stateDiff",
      turn: this.turn,
      round: this.round,
      activePlayer: 0,
      ore: res.ore,
      steel: res.steel,
      coal: res.coal,
      crystal: res.crystal,
      resources: { ...res },
      income: inc,
      powerProduced: pwr.produced,
      powerConsumed: pwr.consumed,
      research: { ...this.research[0] },
      entities: visibleEntities,
      resourceTiles: visibleResources,
      visible: Array.from(this.visibleP0),
      events: [...this.events],
    };
  }
}
