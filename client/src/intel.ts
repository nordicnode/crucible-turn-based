// Combat Intelligence tactical logging system.
// Filters out noise (e.g. ore income) and provides clear, prioritized alerts.

import type { DiffEntity, DiffEvent } from "./types";

export type LogLevel = "info" | "prod" | "warn" | "danger" | "kill";

export interface IntelLogEntry {
  id: number;
  turn: number;
  text: string;
  level: LogLevel;
  tag: string;
}

export function formatClock(turn: number): string {
  return String(Math.max(0, turn));
}

export function friendlyBuildingCompleteMsg(btype: string): string {
  const norm = btype.toLowerCase();
  switch (norm) {
    case "powerplant":
      return "Power Generator online";
    case "refinery":
      return "Refinery constructed";
    case "barracks":
      return "Barracks online";
    case "factory":
      return "Factory operational";
    case "turret":
      return "Defense Turret online";
    case "techlab":
      return "TechLab active";
    case "airfield":
      return "Airfield online";
    case "radar":
      return "Radar array online";
    case "teslacoil":
      return "Tesla Coil charged";
    case "crystalrefinery":
      return "Crystal Refinery online";
    case "aaturret":
      return "AA Turret online";
    default:
      return `${btype} complete`;
  }
}

export function friendlyUnitReadyMsg(utype: string): string {
  const norm = utype.toLowerCase();
  switch (norm) {
    case "infantry":
      return "Infantry squad ready";
    case "scout":
      return "Scout buggy ready";
    case "rockettrooper":
      return "Rocket trooper squad ready";
    case "tank":
      return "Tank roll out";
    case "artillery":
      return "Artillery operational";
    case "gunship":
      return "Gunship airborne";
    case "interceptor":
      return "Interceptor scrambled";
    case "mammothtank":
      return "Mammoth Tank deployed";
    case "samlauncher":
      return "SAM launcher deployed";
    default:
      return `${utype} ready for orders`;
  }
}

/** Humanize a tech name ("HighExplosive" or "highexplosive" →
 *  "High-Explosive Payloads"). The server lowercases event kinds, so both
 *  spellings arrive at the client. */
export function humanizeTech(name: string): string {
  switch (name.toLowerCase()) {
    case "highexplosive":
      return "High-Explosive Payloads";
    case "compositearmor":
      return "Composite Armor";
    case "targetingoptics":
      return "Targeting Optics";
    case "efficientrefining":
      return "Efficient Refining";
    case "rocketpropulsion":
      return "Rocket Propulsion";
    case "titaniumalloys":
      return "Titanium Alloys";
    case "aerialsuperiority":
      return "Aerial Superiority";
    case "superconductors":
      return "Superconductors";
    case "crystalnanotech":
      return "Crystal Nanotech";
    case "advancedballistics":
      return "Advanced Ballistics";
    default:
      return name;
  }
}

export class IntelLogger {
  private nextId = 1;
  entries: IntelLogEntry[] = [];
  readonly maxEntries: number;

  // Throttling for attack warnings (in turns)
  private lastAlertPerEntity = new Map<number, number>();
  private lastCategoryAlertTurn = new Map<string, number>();

  constructor(maxEntries = 40) {
    this.maxEntries = maxEntries;
  }

  clear(): void {
    this.entries = [];
    this.lastAlertPerEntity.clear();
    this.lastCategoryAlertTurn.clear();
    this.nextId = 1;
  }

  addEntry(turn: number, text: string, level: LogLevel, tag: string): IntelLogEntry {
    const entry: IntelLogEntry = {
      id: this.nextId++,
      turn,
      text,
      level,
      tag,
    };
    this.entries.push(entry);
    if (this.entries.length > this.maxEntries) {
      this.entries = this.entries.slice(-this.maxEntries);
    }
    return entry;
  }

  /**
   * Process incoming Server DiffEvents. Returns added entry if any.
   *
   * The server only sends events belonging to the human player (P0), so the
   * friendly branches below are the only ones that can ever fire; enemy
   * activity arrives via fog observations and entity-destruction detection
   * instead.
   */
  processDiffEvent(ev: DiffEvent): IntelLogEntry | null {
    // Explicitly ignore passive income events (refinery drain); the HUD's
    // income readout consumes them instead.
    if (ev.kind === "ore_mined") {
      return null;
    }

    // Building placed / constructed
    if (ev.kind.startsWith("built:")) {
      const btype = ev.kind.slice(6);
      return this.addEntry(
        ev.turn,
        friendlyBuildingCompleteMsg(btype),
        "prod",
        "BASE",
      );
    }

    // Unit trained / complete
    if (ev.kind.startsWith("trained:")) {
      const utype = ev.kind.slice(8);
      return this.addEntry(ev.turn, friendlyUnitReadyMsg(utype), "prod", "UNIT");
    }

    // Research started / completed
    if (ev.kind.startsWith("research:")) {
      const tech = humanizeTech(ev.kind.slice(9));
      return this.addEntry(ev.turn, `Research started: ${tech}`, "prod", "TECH");
    }
    if (ev.kind.startsWith("researched:")) {
      const tech = humanizeTech(ev.kind.slice(11));
      return this.addEntry(ev.turn, `Research complete: ${tech}`, "prod", "TECH");
    }

    // Crystal income
    if (ev.kind === "crystal_mined") {
      const amt = ev.amount != null ? ` (+${ev.amount} crystal)` : "";
      return this.addEntry(ev.turn, `Crystal refined${amt}`, "prod", "CRYSTAL");
    }

    // Structure sold / decommissioned
    if (ev.kind === "sold") {
      const refund = ev.amount != null ? ` (+${ev.amount} ore)` : "";
      return this.addEntry(ev.turn, `Structure sold${refund}`, "info", "SOLD");
    }

    return null;
  }

  /**
   * Check if a friendly entity taking damage warrants an "Under Attack" alert.
   * Debounces repeated hits on the same unit/structure or rapid spam.
   */
  processUnderAttack(turn: number, entity: DiffEntity): IntelLogEntry | null {
    if (entity.owner !== 0) return null;

    const lastAlert = this.lastAlertPerEntity.get(entity.id) ?? -9999;
    const ENTITY_COOLDOWN = 3; // 3 turns per specific entity

    if (turn - lastAlert < ENTITY_COOLDOWN) {
      return null;
    }

    let category = "unit";
    let text = "";
    let level: LogLevel = "warn";
    let tag = "ATTACK";

    if (entity.kind === "Hq") {
      category = "hq";
      text = "ALERT: Base HQ under attack!";
      level = "danger";
      tag = "ALERT";
    } else if (["Refinery", "CrystalRefinery", "Barracks", "Factory", "TechLab", "Airfield", "Radar", "TeslaCoil", "Turret", "AATurret"].includes(entity.kind)) {
      category = `building_${entity.kind}`;
      text = `ALERT: ${entity.kind} under fire!`;
      level = "danger";
      tag = "ALERT";
    } else {
      category = `combat_${entity.kind}`;
      text = `Forces under attack (${entity.kind})`;
      level = "warn";
      tag = "ATTACK";
    }

    const lastCatAlert = this.lastCategoryAlertTurn.get(category) ?? -9999;
    const CATEGORY_COOLDOWN = 2; // 2 turns per alert category
    if (turn - lastCatAlert < CATEGORY_COOLDOWN) {
      return null;
    }

    this.lastAlertPerEntity.set(entity.id, turn);
    this.lastCategoryAlertTurn.set(category, turn);

    return this.addEntry(turn, text, level, tag);
  }

  /**
   * Process entity destruction / loss.
   */
  processEntityDestroyed(turn: number, entity: { id: number; kind: string; owner: number }): IntelLogEntry {
    const isFriendly = entity.owner === 0;

    if (isFriendly) {
      if (entity.kind === "Hq") {
        return this.addEntry(turn, "CRITICAL: Base HQ destroyed!", "danger", "LOST");
      }
      if (["PowerPlant", "Refinery", "CrystalRefinery", "Barracks", "Factory", "TechLab", "Airfield", "Radar", "TeslaCoil", "Turret", "AATurret"].includes(entity.kind)) {
        return this.addEntry(turn, `CRITICAL: ${entity.kind} destroyed!`, "danger", "LOST");
      }
      return this.addEntry(turn, `Unit lost: ${entity.kind}`, "danger", "LOST");
    } else {
      if (["Hq", "PowerPlant", "Refinery", "CrystalRefinery", "Barracks", "Factory", "TechLab", "Airfield", "Radar", "TeslaCoil", "Turret", "AATurret"].includes(entity.kind)) {
        return this.addEntry(turn, `Enemy ${entity.kind} destroyed!`, "kill", "KILL");
      }
      return this.addEntry(turn, `Hostile neutralized: ${entity.kind}`, "kill", "KILL");
    }
  }

  /**
   * Alert commander if power consumption exceeds production.
   */
  processPowerStatus(turn: number, powerProduced: number, powerConsumed: number): IntelLogEntry | null {
    if (powerConsumed > powerProduced) {
      const last = this.lastCategoryAlertTurn.get("low_power") ?? -9999;
      if (turn - last >= 10) {
        // At most once every 10 turns
        this.lastCategoryAlertTurn.set("low_power", turn);
        return this.addEntry(turn, `LOW POWER WARNING: ${powerConsumed}/${powerProduced} PWR (50% Production Speed)`, "warn", "POWER");
      }
    }
    return null;
  }
}
