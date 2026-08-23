// Client entry point: lobby, match loop, input, combat FX, and compact command rail.
// All simulation rules are server-side; this renders tactical state and forwards commands.
// Commands apply immediately; EndTurn resolves the bot synchronously and the
// next state diff is one complete player-facing round. There is no wall-clock tick.

import { initDashboard } from "./dashboard";
import { inspectionForTile } from "./inspector";
import { fx } from "./fx";
import { IntelLogger } from "./intel";
import { Net } from "./net";
import { drawRadar, isBuildingPlacable, Renderer } from "./renderer";
import { spectate } from "./spectate";
import { getAssetUrl, getCursorDataUrl, getThumbnailDataUrl } from "./sprites";
import { World, type Entity } from "./world";
import { MAP_SIZE, MAP_TILES } from "./types";
import {
  BUILDING_KINDS,
  BUILDING_POWER,
  BUILD_COSTS,
  BUILD_STATS,
  TECH_INFO,
  UNIT_COSTS,
  UNIT_KINDS,
  UNIT_STATS,
  attack,
  clearMove,
  endTurn,
  moveGroup,
  placeBuilding,
  repair,
  sell,
  setRally,
  startResearch,
  trainUnit,
  formatResourceCost,
  resourceBundleAffordable,
  BUILDING_PREREQS,
  UNIT_TREE,
  type BuildingType,
  type Command,
  type ResourceBundle,
  type ServerMsg,
  type TechId,
  type UnitType,
} from "./types";

const canvas = document.getElementById("view") as HTMLCanvasElement;
// Non-null by construction (an unreachable init bail rather than a per-frame
// `!`); the declared non-null type keeps closures from re-narrowing to null.
const ctx: CanvasRenderingContext2D = (() => {
  const c = canvas.getContext("2d");
  if (!c) throw new Error("2D canvas context unavailable");
  return c;
})();

const net = new Net();
const world = new World();
const renderer = new Renderer();
const intel = new IntelLogger();

// Separate renderer, world, and state for background menu simulation
const menuRenderer = new Renderer();
const menuWorld = new World();
let menuInit = false;

let demoTime = 0;

let inGame = false;
let selection = new Set<number>();

/** One authoritative attack buffered for cinematic one-at-a-time playback. */
interface QueuedAttack {
  attackerId: number;
  targetId: number;
  kind: "bullet" | "shell" | "artillery" | "laser";
  color: string;
}
// Attacks delivered in a diff are replayed one at a time (never all in one
// frame) so an enemy turn's combat is never missed, and the camera focuses on
// each one. `nextAttackAt` paces the queue off `performance.now()`.
const combatQueue: QueuedAttack[] = [];
let nextAttackAt = 0;
let placementMode: BuildingType | null = null;
let placementCursor: [number, number] | null = null;
let selectedTile: [number, number] | null = null;
/** Tile under the mouse for the movement path preview (U4). */
let hoverTile: [number, number] | null = null;
let opponentLabel = "hard";

// Waypoint destination tracking for tactical movement lines
// (authoritative destinations are refreshed from each state diff).
const unitWaypoints = new Map<number, [number, number]>();
let lastInspectorSig = "";

// Input drag and pan state
let dragStart: [number, number] | null = null;
let dragCurrent: [number, number] | null = null;
let panning = false;
let lastPan: [number, number] | null = null;
let minimapDragging = false;
const keysPressed = new Set<string>();

// Previous entity states for combat trigger detection
let prevEntityHp = new Map<number, number>();

/** A ~20 Hz cosmetic clock (independent of turns): drives sprite shimmer,
 *  muzzle flashes and firing-age recoil purely cosmetically. */
function animClock(): number {
  return Math.floor(performance.now() / 50);
}

function el<T extends HTMLElement>(id: string): T {
  return document.getElementById(id) as T;
}

/** Human-readable climate label for a 0–255 temperature value. */
function tempLabel(t: number): string {
  if (t < 60) return "frigid";
  if (t < 90) return "cold";
  if (t < 140) return "cool";
  if (t < 185) return "warm";
  return "tropical";
}

// ---------------------------------------------------------------------------
// Server messages
// ---------------------------------------------------------------------------

function onServerMsg(msg: ServerMsg): void {
  switch (msg.type) {
    case "matchStart": {
      inGame = true;
      // Discard any combat still queued from a previous match so stale attacks
      // (resolved against the fresh match's unrelated entities) never replay.
      combatQueue.length = 0;
      nextAttackAt = 0;
      world.setMap(
        msg.mapSeed,
        msg.passable,
        msg.terrain ?? [],
        msg.hq,
        msg.terrainRules,
        msg.elevation ?? [],
        msg.moisture ?? [],
        msg.temperature ?? [],
      );
      const ownHq = msg.hq[msg.player];
      document.body.classList.add("in-match");
      // Re-measure after the HUD claims its own layer; the camera and input
      // surface must use the inset battlefield dimensions, not the window.
      resize();
      // Keep the player's HQ centered even when it spawns against a map edge;
      // the command rail never covers the opening position.
      renderer.camera.focusOn(
        ownHq[0] + 0.5,
        ownHq[1] + 0.5,
        18,
        canvas.width,
        canvas.height,
        true,
      );
      el("overlay").classList.add("hidden");
      el("lobby").classList.add("hidden");
      el("result").classList.add("hidden");
      el("dashboard").classList.add("hidden");
      el("spectate-list").classList.add("hidden");
      document.body.classList.add("in-match");
      el("sidebar").classList.remove("hidden");
      el("topbar").classList.remove("hidden");
      el("turn-ribbon").classList.remove("hidden");
      el("radar-block").classList.remove("hidden");
      el("log").classList.remove("hidden");
      el("opponent").textContent = opponentLabel.toUpperCase();
      unitWaypoints.clear();
      prevEntityHp.clear();
      selectedTile = null;
      world.clearTileInspection();
      lastInspectorSig = "";
      renderTileInspector();
      intel.clear();
      intel.addEntry(0, "Tactical link active. Operation underway.", "info", "LINK");
      lastRenderedLogCount = -1;
      renderTurnIndicator();
      break;
    }
    case "stateDiff": {
      // Detect combat / attacks by observing entity damage & positions
      for (const e of msg.entities) {
        const prevHp = prevEntityHp.get(e.id);
        if (prevHp != null && e.hp < prevHp) {
          // Floating damage number (U7)
          const dmg = prevHp - e.hp;
          const dmgColor = e.owner === 0 ? "#f87171" : "#fbbf24";
          fx.spawnFloatingText(e.x, e.y, `-${dmg}`, dmgColor);
          // Check for under-attack alert if friendly
          intel.processUnderAttack(msg.turn, e);
          // Impact sparks on the victim. The authoritative projectile + muzzle
          // flash + shooter recoil are drawn from the server's `attacked`
          // events below (the projectiles carry real attacker/target ids).
          fx.spawnImpactSparks(e.x, e.y, e.owner === 0 ? "#f87171" : "#fb923c");
        }
        prevEntityHp.set(e.id, e.hp);
      }

      // Check for destroyed entities
      for (const [id] of prevEntityHp) {
        if (!msg.entities.some((e) => e.id === id)) {
          const oldE = world.entities.get(id);
          if (oldE) {
            const angle = oldE.owner === 0 ? Math.PI / 4 : -3 * Math.PI / 4;
            fx.spawnDeath(oldE.x, oldE.y, angle, oldE.kind, oldE.owner);
            intel.processEntityDestroyed(msg.turn, oldE);
          }
          prevEntityHp.delete(id);
        }
      }

      world.applyDiff(
        msg.turn,
        msg.activePlayer,
        msg.ore,
        msg.crystal ?? 0,
        msg.research ?? { points: 0, researching: null, researched: [] },
        msg.entities,
        msg.oreTiles,
        msg.crystalTiles ?? [],
        msg.visible,
        msg.events,
        // Use the server's authoritative power numbers (the client's static
        // table is only a fallback for menus/spectate).
        { produced: msg.powerProduced ?? 0, consumed: msg.powerConsumed ?? 0 },
        msg.steel ?? 0,
        msg.coal ?? 0,
        msg.resources,
        msg.income,
        msg.resourceTiles ?? [],
        msg.actionsSpent,
        msg.actionsCap,
        msg.round,
      );

      renderTileInspector();

      // The server is authoritative for durable routes. Keep the local map
      // only as a transient optimistic hint until this diff arrives.
      for (const e of msg.entities) {
        if (e.owner !== 0 || !UNIT_KINDS.has(e.kind)) continue;
        if (e.moveTarget) unitWaypoints.set(e.id, e.moveTarget);
        else unitWaypoints.delete(e.id);
      }

      for (const ev of msg.events) {
        intel.processDiffEvent(ev);
        // Authoritative combat: buffer each attack so it can be replayed one at
        // a time with camera focus (never all in a single frame), instead of
        // spawning a pile of projectiles that are instantly missed.
        if (ev.kind !== "attacked" || ev.attacker == null || ev.target == null) continue;
        const attacker = world.entities.get(ev.attacker);
        const target = world.entities.get(ev.target);
        if (!attacker || !target) continue;
        combatQueue.push({
          attackerId: ev.attacker,
          targetId: ev.target,
          kind: combatAttackKind(attacker.kind),
          // Friendly fire reads teal (you attacking), incoming fire orange.
          color: attacker.owner === 0 ? "#9be8c9" : "#ffb35c",
        });
      }
      renderTurnIndicator();
      break;
    }
    case "matchEnd": {
      inGame = false;
      // Stop replaying the old match's combat once it's over; the next match
      // starts clean (see matchStart).
      combatQueue.length = 0;
      nextAttackAt = 0;
      world.result = { winner: msg.winner, reason: msg.reason };
      lastReplayId = msg.replayId ?? null;
      // F4: feed the adaptive-difficulty tracker (draws are neutral).
      if (msg.winner != null) {
        recordResult(opponentLabel, msg.winner === 0);
      }
      // A draw arrives with `winner: null`; it must not render as a defeat.
      const title =
        msg.winner === null ? "DRAW" : msg.winner === 0 ? "VICTORY" : "DEFEAT";
      el("result-title").textContent = title;
      el("result-title").className =
        msg.winner === null ? "draw" : msg.winner === 0 ? "win" : "lose";
      // Victory/defeat emblem (A8).
      const emblem = el("result-emblem");
      if (emblem) {
        emblem.textContent = msg.winner === null ? "=" : msg.winner === 0 ? "\u2713" : "\u2717";
        emblem.style.borderColor =
          msg.winner === null ? "var(--gold)" : msg.winner === 0 ? "var(--green)" : "var(--red)";
        emblem.style.color =
          msg.winner === null ? "var(--gold-bright)" : msg.winner === 0 ? "var(--green)" : "var(--red)";
        emblem.style.boxShadow =
          msg.winner === null
            ? "0 0 24px var(--gold-glow)"
            : msg.winner === 0
              ? "0 0 24px rgba(116,176,138,0.4)"
              : "0 0 24px rgba(208,120,104,0.4)";
      }
      el("result-detail").textContent =
        `${msg.reason} · ${formatTurns(msg.durationRounds ?? Math.ceil(msg.durationTurns / 2))} rounds · ${formatTurns(msg.durationTurns)} activations · replay #${msg.replayId ?? "?"}`;
      // Plan §8: tell the player their match feeds the trainer's ghost pool.
      el("result-ghost").textContent =
        msg.replayId != null
          ? "This match is now a training ghost — the AI will study it."
          : "";
      el("overlay").classList.remove("hidden");
      el("lobby").classList.add("hidden");
      el("result").classList.remove("hidden");
      break;
    }
    case "tileInspection": {
      if (
        selectedTile
        && selectedTile[0] === msg.x
        && selectedTile[1] === msg.y
      ) {
        world.applyTileInspection(msg);
        lastInspectorSig = "";
        renderTileInspector();
      }
      break;
    }
    case "commandRejected": {
      intel.addEntry(world.turn, `Order rejected: ${msg.reason}`, "warn", "ORDER");
      break;
    }
    case "serverBusy": {
      // Server is at its concurrent-match capacity: back to the lobby with a
      // visible reason instead of silently hanging on a dead connection.
      showLobby();
      el("lobby-status").textContent =
        "All tactical channels busy — try again in a moment.";
      break;
    }
    default: {
      // Every current ServerMsg variant is handled above; reaching the default
      // is a wire-format/protocol drift and must never be silently dropped.
      // (The union is exhaustive today, so a new variant that forgets its case
      // fails the `msg.type` assignment here rather than vanishing.)
      const unknown: never = msg;
      void unknown;
    }
  }
}

// P0: a stable pseudo-anonymous player id (UUID) persisted locally, sent on
// every joinMatch so the server can learn per-player tendencies (P1+).
function getOrCreatePlayerId(): string {
  try {
    let id = localStorage.getItem("crucible.playerId");
    if (!id) {
      id = crypto.randomUUID();
      localStorage.setItem("crucible.playerId", id);
    }
    return id;
  } catch {
    // storage unavailable (private mode) — send a stable anon fallback
    return "anon";
  }
}

function startMatch(which: string, label?: string): void {
  inGame = false;
  opponentLabel = label ?? which;
  selection = new Set();
  placementMode = null;
  placementCursor = null;
  selectedTile = null;
  world.clearTileInspection();
  lastInspectorSig = "";
  renderTileInspector();
  intel.clear();
  lastRenderedLogCount = -1;
  net.close();
  net.connect(onServerMsg, showLobby);
  net.send({ type: "joinMatch", opponent: which, playerId: getOrCreatePlayerId() });
}

function showLobby(): void {
  inGame = false;
  document.body.classList.remove("in-match");
  el("overlay").classList.remove("hidden");
  el("lobby").classList.remove("hidden");
  el("result").classList.add("hidden");
  const tierHint = el("lobby-tier");
  if (tierHint) {
    tierHint.textContent = `Recommended: ${adaptiveTier().toUpperCase()} (from your recent results)`;
  }
  document.body.classList.remove("in-match");
  el("sidebar").classList.add("hidden");
  el("topbar").classList.add("hidden");
  el("turn-ribbon").classList.add("hidden");
  el("radar-block").classList.add("hidden");
  el("log").classList.add("hidden");
  selectedTile = null;
  world.clearTileInspection();
  lastInspectorSig = "";
  renderTileInspector();
  el("lobby-status").textContent = "";
}

// ---------------------------------------------------------------------------
// Turn indicator
// ---------------------------------------------------------------------------

function renderTurnIndicator(): void {
  const round = document.getElementById("round");
  if (round) round.textContent = String(world.round);
  el("turn").textContent = String(world.turn);
  const ribbon = el("turn-ribbon");
  ribbon.classList.remove("hidden");
  const state = el("turn-state");
  const endBtn = el<HTMLButtonElement>("action-end-turn");
  // Topbar readouts: crystal stock and the research button's availability.
  el("crystal").textContent = String(world.crystal);
  const researchBtn = el<HTMLButtonElement>("research-open");
  if (researchBtn) {
    const hasLab = inGame && world.ownBuildings.some((b) => b.kind === "TechLab");
    researchBtn.disabled = !hasLab;
    if (researchOpen) renderResearch();
  }
  const isMine = inGame && world.activePlayer === 0;
  if (isMine) {
    ribbon.classList.add("your-turn");
    ribbon.classList.remove("their-turn");
    state.textContent = "— YOUR MOVE";
  } else {
    ribbon.classList.remove("your-turn");
    ribbon.classList.add("their-turn");
    state.textContent = "— OPPONENT TURN";
  }
  endBtn.disabled = !inGame || !isMine;
  endBtn.textContent = "END TURN";
  if (isMine) {
    endBtn.classList.add("armed");
  } else {
    endBtn.classList.remove("armed");
  }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

function sendCommands(cmds: Command[]): void {
  if (cmds.length > 0) net.send({ type: "commands", cmds });
}

function selectedUnits(): number[] {
  return [...selection].filter((id) => {
    const e = world.entities.get(id);
    return e && e.owner === 0 && UNIT_KINDS.has(e.kind);
  });
}

function selectedSingle(): number | null {
  return selection.size === 1 ? [...selection][0] : null;
}

/** Production building kinds that accept a rally point (mirrors the sim). */
const PRODUCER_KINDS = new Set(["Barracks", "Factory", "Airfield"]);

/** The id of the single own production building currently selected, if any. */
function singleProducerSelected(): number | null {
  if (selection.size !== 1) return null;
  const e = world.entities.get([...selection][0]);
  if (!e || e.owner !== 0 || !PRODUCER_KINDS.has(e.kind)) return null;
  return e.id;
}

function issueMove(tile: [number, number]): void {
  const units = selectedUnits();
  if (units.length > 0) {
    sendCommands([moveGroup(units, tile)]);
    for (const u of units) {
      unitWaypoints.set(u, tile);
    }
  }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Radar Canvas Navigation (In Command Sidebar)
// ---------------------------------------------------------------------------

const radarCanvas = document.getElementById("radar") as HTMLCanvasElement | null;
const radarCtx = radarCanvas?.getContext("2d");

function radarTileAt(ev: MouseEvent): [number, number] | null {
  if (!radarCanvas) return null;
  const r = radarCanvas.getBoundingClientRect();
  const rx = (ev.clientX - r.left) * (radarCanvas.width / Math.max(1, r.width));
  const ry = (ev.clientY - r.top) * (radarCanvas.height / Math.max(1, r.height));
  const s = radarCanvas.width / MAP_SIZE;
  const tx = rx / s;
  const ty = ry / s;
  if (tx >= 0 && tx < MAP_SIZE && ty >= 0 && ty < MAP_SIZE) {
    return [tx, ty];
  }
  return null;
}

if (radarCanvas) {
  radarCanvas.addEventListener("mousedown", (ev) => {
    if (ev.button === 0) {
      const tile = radarTileAt(ev);
      if (tile) {
        const cam = spectate.active ? spectate.renderer.camera : renderer.camera;
        cam.focusOn(tile[0], tile[1], cam.zoom, canvas.width, canvas.height);
      }
    }
  });

  radarCanvas.addEventListener("mousemove", (ev) => {
    if (ev.buttons === 1) {
      const tile = radarTileAt(ev);
      if (tile) {
        const cam = spectate.active ? spectate.renderer.camera : renderer.camera;
        cam.focusOn(tile[0], tile[1], cam.zoom, canvas.width, canvas.height);
      }
    }
  });
}

// ---------------------------------------------------------------------------
// Input Handling
// ---------------------------------------------------------------------------

function canvasPos(ev: MouseEvent): [number, number] {
  const r = canvas.getBoundingClientRect();
  return [
    (ev.clientX - r.left) * (canvas.width / Math.max(1, r.width)),
    (ev.clientY - r.top) * (canvas.height / Math.max(1, r.height)),
  ];
}

function tileAt(sx: number, sy: number): [number, number] {
  return [Math.floor(renderer.camera.worldX(sx)), Math.floor(renderer.camera.worldY(sy))];
}

function selectTile(tile: [number, number]): void {
  if (tile[0] < 0 || tile[0] >= MAP_SIZE || tile[1] < 0 || tile[1] >= MAP_SIZE) return;
  selectedTile = tile;
  world.clearTileInspection();
  lastInspectorSig = "";
  renderTileInspector();
  if (inGame) {
    net.send({ type: "inspectTile", x: tile[0], y: tile[1] });
  }
}

canvas.addEventListener("mousedown", (ev) => {
  const [sx, sy] = canvasPos(ev);

  if (spectate.active) {
    if (ev.button === 1 || ev.button === 0) {
      const mmPos = spectate.renderer.minimapToWorld(sx, sy, canvas.width, canvas.height);
      if (mmPos) {
        spectate.renderer.camera.centerOn(mmPos[0], mmPos[1], canvas.width, canvas.height);
        return;
      }
    }
    if (ev.button === 1) {
      panning = true;
      lastPan = [sx, sy];
    }
    return;
  }

  if (!inGame) return;

  // Main Game Canvas Input
  if (ev.button === 0) {
    const [tx, ty] = tileAt(sx, sy);
    if (toolMode === "sell") {
      selectTile([tx, ty]);
      const b = buildingAt(tx, ty);
      if (b && b.kind !== "Hq" && b.owner === 0) {
        sendCommands([sell(b.id)]);
      }
      return;
    }
    if (toolMode === "repair") {
      selectTile([tx, ty]);
      const b = buildingAt(tx, ty);
      if (b && b.hp < b.maxHp && b.owner === 0) {
        sendCommands([repair(b.id)]);
      }
      return;
    }
    if (placementMode) {
      selectTile([tx, ty]);
      if (isBuildingPlacable(placementMode, [tx, ty], world)) {
        sendCommands([placeBuilding(placementMode, [tx, ty])]);
        placementMode = null;
        placementCursor = null;
        lastPanelSig = "";
        renderCommandSidebar();
      }
      return;
    }
    dragStart = [sx, sy];
    dragCurrent = [sx, sy];
  } else if (ev.button === 1) {
    panning = true;
    lastPan = [sx, sy];
  } else if (ev.button === 2) {
    toolMode = null;
    placementMode = null;
    placementCursor = null;
    lastPanelSig = "";
    renderCommandSidebar();
    const [tx, ty] = tileAt(sx, sy);
    // C&C right-click: an enemy under the cursor gets focus-fired by the
    // selected combat units; a selected production building sets/clears its
    // rally point on open ground; otherwise open ground is an attack-move.
    const target = enemyEntityAt(tx, ty);
    const units = selectedUnits();
    if (target && units.length > 0) {
      // Attack stops the current march first (clearMove), then fires — so
      // ordering an attack really halts the unit instead of the one-shot
      // volley being drowned by a still-running move order.
      sendCommands([clearMove(units), attack(units, target.id)]);
    } else {
      const producer = singleProducerSelected();
      if (producer != null) {
        // Right-click the producer itself to clear its rally point.
        sendCommands([setRally(producer, [tx, ty])]);
        lastPanelSig = "";
        renderCommandSidebar();
      } else {
        issueMove([tx, ty]);
      }
    }
    // Intentionally keep the selection after a right-click order so the
    // player can chain move/attack commands without re-selecting each time.
  }
});

canvas.addEventListener("mousemove", (ev) => {
  const [sx, sy] = canvasPos(ev);

  if (spectate.active) {
    if (panning && lastPan) {
      spectate.renderer.camera.pan(sx - lastPan[0], sy - lastPan[1], canvas.width, canvas.height);
      lastPan = [sx, sy];
    }
    return;
  }

  if (!inGame) return;

  updateCursor(sx, sy);

  if (minimapDragging) {
    const mmPos = renderer.minimapToWorld(sx, sy, canvas.width, canvas.height);
    if (mmPos) {
      renderer.camera.centerOn(mmPos[0], mmPos[1], canvas.width, canvas.height);
    }
    return;
  }

  if (panning && lastPan) {
    renderer.camera.pan(sx - lastPan[0], sy - lastPan[1], canvas.width, canvas.height);
    lastPan = [sx, sy];
  } else if (dragStart) {
    dragCurrent = [sx, sy];
  }

  if (placementMode && !panning && !dragStart) {
    placementCursor = tileAt(sx, sy);
  } else if (!panning && !dragStart) {
    const t = tileAt(sx, sy);
    hoverTile = t[0] >= 0 && t[0] < MAP_SIZE && t[1] >= 0 && t[1] < MAP_SIZE ? t : null;
  }
});

canvas.addEventListener("mouseup", (ev) => {
  minimapDragging = false;
  if (ev.button === 1) {
    panning = false;
    lastPan = null;
  }
  if (!inGame || ev.button !== 0 || !dragStart) return;
  const start = dragStart;
  const [sx, sy] = canvasPos(ev);
  dragStart = null;
  dragCurrent = null;

  if (Math.hypot(sx - start[0], sy - start[1]) < 4) {
    const [tx, ty] = tileAt(sx, sy);
    const target = enemyEntityAt(tx, ty);
    const units = selectedUnits();
    if (target && units.length > 0) {
      // Left-click on an enemy with a firing selection = attack (stop the
      // march, then fire). Keeps the selection for chained orders.
      sendCommands([clearMove(units), attack(units, target.id)]);
      return;
    }
    selectAt(sx, sy, ev.shiftKey);
  } else {
    boxSelect(start, [sx, sy]);
  }
});

function boxSelect(a: [number, number], b: [number, number]): void {
  const minX = Math.min(a[0], b[0]), maxX = Math.max(a[0], b[0]);
  const minY = Math.min(a[1], b[1]), maxY = Math.max(a[1], b[1]);
  const hit: number[] = [];
  for (const e of world.ownUnits) {
    const sx = renderer.camera.screenX(e.x);
    const sy = renderer.camera.screenY(e.y);
    if (sx >= minX && sx <= maxX && sy >= minY && sy <= maxY) hit.push(e.id);
  }
  if (hit.length === 0) {
    // Dragging an empty box deselects everything (a clear way to unselect).
    clearSelection();
    return;
  }
  // A box replaces the selection rather than piling onto it.
  selection = new Set(hit);
  selectedTile = null;
  world.clearTileInspection();
  lastInspectorSig = "";
  renderTileInspector();
  lastPanelSig = "";
  renderCommandSidebar();
}

function selectAt(sx: number, sy: number, additive: boolean): void {
  const [tx, ty] = tileAt(sx, sy);
  let bestId: number | null = null;
  let bestDist = Infinity;

  // First check if clicking directly ON an own building's tile
  const b = buildingAt(tx, ty);
  if (b && b.owner === 0) {
    bestId = b.id;
  } else {
    // Check if clicking near an own unit
    for (const e of world.ownUnits) {
      const dx = e.x - (tx + 0.5);
      const dy = e.y - (ty + 0.5);
      const d = dx * dx + dy * dy;
      if (d < 0.65 && d < bestDist) {
        bestDist = d;
        bestId = e.id;
      }
    }
  }

  if (bestId != null) {
    if (!additive) selection = new Set();
    // Selecting one unit in a stacked tile pulls in the whole stack, so a
    // multi-unit tile can be commanded as a group.
    if (!additive) {
      const clicked = world.entities.get(bestId);
      if (clicked && UNIT_KINDS.has(clicked.kind)) {
        const stack = [...world.ownUnits].filter(
          (u) =>
            Math.floor(u.x) === Math.floor(clicked.x) &&
            Math.floor(u.y) === Math.floor(clicked.y),
        );
        if (stack.length > 1) {
          selection = new Set(stack.map((u) => u.id));
          selectTile([tx, ty]);
          lastPanelSig = "";
          renderCommandSidebar();
          return;
        }
      }
    }
    selection.add(bestId);
    // Inspect the selected entity's tile.
    selectTile([tx, ty]);
  } else if (!additive) {
    // A left-click on ground with nothing to pick fully deselects, so a
    // selected unit/building/tile can always be cleared back to nothing.
    clearSelection();
  }
  lastPanelSig = "";
  renderCommandSidebar();
}

/** Deselect every entity and clear the tile inspector (+ refresh UI). */
function clearSelection(): void {
  selection = new Set();
  selectedTile = null;
  world.clearTileInspection();
  lastInspectorSig = "";
  renderTileInspector();
  lastPanelSig = "";
  renderCommandSidebar();
}

/** Map an attacking entity's kind to its projectile type for attack FX. */
function combatAttackKind(kind: string): "bullet" | "shell" | "artillery" | "laser" {
  switch (kind) {
    case "Artillery":
      return "artillery";
    case "Tank":
    case "MammothTank":
      return "shell";
    case "Turret":
    case "TeslaCoil":
      return "laser";
    default:
      return "bullet";
  }
}

/** Play one queued attack: spawn the projectile + recoil + victim flash and
 *  bring the combat into view. Then pace the next one. */
function playCombatAttack(a: QueuedAttack): void {
  const attacker = world.entities.get(a.attackerId);
  const target = world.entities.get(a.targetId);
  if (attacker && target) {
    fx.spawnAttack(attacker.x, attacker.y, target.x, target.y, a.kind, a.color);
    fx.recordUnitFiring(a.attackerId, animClock());
    fx.recordHit(a.targetId, animClock());
    focusCombatCamera(attacker.x, attacker.y, target.x, target.y);
  }
  // Pace the next attack off the projectile travel time + a short beat so the
  // impact is readable and consecutive hits don't stack into one frame.
  const base = a.kind === "artillery" ? 8 : a.kind === "shell" ? 14 : a.kind === "laser" ? 30 : 18;
  const dist = attacker && target ? Math.hypot(target.x - attacker.x, target.y - attacker.y) : 6;
  const travelMs = Math.max(0.4, dist / Math.max(1, base) + 0.3) * 1000;
  nextAttackAt = performance.now() + travelMs;
}

/** Center the camera on combat, but only if the fight is off-screen — an
 *  already-visible battle shouldn't yank the view around. */
function focusCombatCamera(ax: number, ay: number, tx: number, ty: number): void {
  const mx = (ax + tx) / 2;
  const my = (ay + ty) / 2;
  const cam = renderer.camera;
  const vx0 = cam.worldX(0);
  const vy0 = cam.worldY(0);
  const vx1 = cam.worldX(canvas.width);
  const vy1 = cam.worldY(canvas.height);
  const pad = 1.5;
  if (
    mx >= vx0 - pad &&
    mx <= vx1 + pad &&
    my >= vy0 - pad &&
    my <= vy1 + pad
  ) {
    return;
  }
  cam.centerOn(mx, my, canvas.width, canvas.height);
}

canvas.addEventListener("wheel", (ev) => {
  ev.preventDefault();
  const [sx, sy] = canvasPos(ev);
  if (spectate.active) {
    spectate.renderer.camera.zoomAt(sx, sy, ev.deltaY < 0 ? 1.18 : 1 / 1.18, canvas.width, canvas.height);
  } else if (inGame) {
    renderer.camera.zoomAt(sx, sy, ev.deltaY < 0 ? 1.18 : 1 / 1.18, canvas.width, canvas.height);
  }
});

canvas.addEventListener("contextmenu", (ev) => ev.preventDefault());

// ---------------------------------------------------------------------------
// Touch support (U10): basic touch → mouse mapping for mobile play
// ---------------------------------------------------------------------------
let touchStart: { x: number; y: number; t: number } | null = null;
let lastTapTime = 0;

canvas.addEventListener("touchstart", (ev) => {
  ev.preventDefault();
  if (ev.touches.length === 0) return;
  const t = ev.touches[0];
  const r = canvas.getBoundingClientRect();
  const sx = t.clientX - r.left;
  const sy = t.clientY - r.top;
  touchStart = { x: sx, y: sy, t: performance.now() };

  if (ev.touches.length === 2) {
    // Two-finger pan
    panning = true;
    lastPan = [sx, sy];
  } else {
    dragStart = [sx, sy];
    dragCurrent = [sx, sy];
  }
}, { passive: false });

canvas.addEventListener("touchmove", (ev) => {
  ev.preventDefault();
  if (ev.touches.length === 0) return;
  const t = ev.touches[0];
  const r = canvas.getBoundingClientRect();
  const sx = t.clientX - r.left;
  const sy = t.clientY - r.top;
  if (panning && lastPan) {
    renderer.camera.pan(sx - lastPan[0], sy - lastPan[1], canvas.width, canvas.height);
    lastPan = [sx, sy];
  } else if (touchStart) {
    dragCurrent = [sx, sy];
  }
  if (placementMode && !panning) {
    placementCursor = tileAt(sx, sy);
  }
}, { passive: false });

canvas.addEventListener("touchend", (ev) => {
  ev.preventDefault();
  panning = false;
  lastPan = null;
  if (!touchStart) return;
  const dt = performance.now() - touchStart.t;
  const [sx, sy] = [touchStart.x, touchStart.y];
  touchStart = null;
  const start = dragStart;
  dragStart = null;
  dragCurrent = null;
  if (!start) return;

  const dist = Math.hypot(sx - start[0], sy - start[1]);
  if (dist < 8 && dt < 300) {
    // Quick tap: select / act
    const now = performance.now();
    const isDoubleTap = now - lastTapTime < 300;
    lastTapTime = now;
    if (isDoubleTap && inGame && world.activePlayer === 0) {
      // Double-tap = end turn (mobile shortcut)
      sendCommands([endTurn()]);
      renderTurnIndicator();
      return;
    }
    selectAt(sx, sy, false);
  } else if (dist >= 8) {
    // Drag: box select
    boxSelect(start, [sx, sy]);
  }
}, { passive: false });

// C&C-style control groups: Ctrl+1..9 assigns the current selection, 1..9
// recalls it. F1..F4 switch the command-sidebar tabs (keyboard-first play).
const controlGroups = new Map<number, Set<number>>();

window.addEventListener("keydown", (ev) => {
  keysPressed.add(ev.code);
  if (ev.key === "Escape") {
    placementMode = null;
    placementCursor = null;
    toolMode = null;
    lastPanelSig = "";
    renderCommandSidebar();
    if (researchOpen) closeResearch();
    el("shortcuts-overlay").classList.add("hidden");
    el("sell-confirm").classList.add("hidden");
    return;
  }
  // ? toggles the keyboard shortcuts overlay.
  if (ev.key === "?" || (ev.shiftKey && ev.key === "/")) {
    const overlay = el("shortcuts-overlay");
    overlay.classList.toggle("hidden");
    return;
  }
  // Space ends the turn (keyboard shortcut for the button).
  if (ev.key === " " || ev.code === "Space") {
    ev.preventDefault();
    if (inGame && world.activePlayer === 0) {
      sendCommands([endTurn()]);
      renderTurnIndicator();
    }
    return;
  }
  if (!inGame) return;
  // R opens the research tree (Civ-style keyboard-first flow).
  if (ev.key === "r" || ev.key === "R") {
    if (researchOpen) closeResearch();
    else openResearch();
    return;
  }

  const groupIdx = /^[1-9]$/.exec(ev.key) ? Number(ev.key) : null;
  if (groupIdx !== null) {
    if (ev.ctrlKey || ev.metaKey) {
      // Assign: save the current selection (units only; buildings stay put).
      controlGroups.set(groupIdx, new Set(world.ownUnits.map((u) => u.id)));
    } else if (controlGroups.has(groupIdx)) {
      // Recall: select the group (Shift adds instead of replacing).
      selection = new Set(controlGroups.get(groupIdx)!);
      lastPanelSig = "";
      renderCommandSidebar();
    }
    return;
  }

  const tabFor = (key: string): CommandTab | null => {
    if (key === "F1") return "buildings";
    if (key === "F2") return "troops";
    if (key === "F3") return "vehicles";
    if (key === "F4") return "aircraft";
    return null;
  };
  const tab = tabFor(ev.key);
  if (tab) {
    activeTab = tab;
    lastPanelSig = "";
    renderCommandSidebar();
  }
});

window.addEventListener("keyup", (ev) => {
  keysPressed.delete(ev.code);
});

// ---------------------------------------------------------------------------
// HUD & C&C Command Sidebar
// ---------------------------------------------------------------------------
// Command Matrix Tabs & Global Tool Modes
// ---------------------------------------------------------------------------

type CommandTab = "buildings" | "troops" | "vehicles" | "aircraft";
let activeTab: CommandTab = "buildings";
let toolMode: "sell" | "repair" | null = null;
let tabIconsInitialized = false;

function initToolAndTabIcons(): void {
  if (tabIconsInitialized) return;
  tabIconsInitialized = true;

  const repairImg = el("action-repair-img") as HTMLImageElement | null;
  if (repairImg) repairImg.src = getAssetUrl("ui", "repair");
  const sellImg = el("action-sell-img") as HTMLImageElement | null;
  if (sellImg) sellImg.src = getAssetUrl("ui", "sell");

  const bImg = el("tab-icon-buildings") as HTMLImageElement | null;
  if (bImg) bImg.src = getThumbnailDataUrl("tab_buildings", 0);
  const tImg = el("tab-icon-troops") as HTMLImageElement | null;
  if (tImg) tImg.src = getThumbnailDataUrl("tab_troops", 0);
  const vImg = el("tab-icon-vehicles") as HTMLImageElement | null;
  if (vImg) vImg.src = getThumbnailDataUrl("tab_vehicles", 0);
  const aImg = el("tab-icon-aircraft") as HTMLImageElement | null;
  if (aImg) aImg.src = getThumbnailDataUrl("tab_aircraft", 0);

  // End Turn button: send the free EndTurn command (only valid on own turn;
  // the button is disabled while the opponent is thinking).
  const endBtn = el("action-end-turn");
  endBtn.addEventListener("click", () => {
    if (inGame && world.activePlayer === 0) {
      sendCommands([endTurn()]);
      renderTurnIndicator();
    }
  });

  // Research button: opens the tech tree (also bound to R).
  const researchOpenBtn = el("research-open");
  if (researchOpenBtn) {
    researchOpenBtn.addEventListener("click", () => {
      if (researchOpen) closeResearch();
      else openResearch();
    });
  }
  const researchCloseBtn = el("research-close");
  if (researchCloseBtn) {
    researchCloseBtn.addEventListener("click", closeResearch);
  }

  // Tab button click listeners
  for (const tabName of ["buildings", "troops", "vehicles", "aircraft"] as CommandTab[]) {
    const btn = el(`tab-btn-${tabName}`);
    if (btn) {
      btn.addEventListener("click", () => {
        activeTab = tabName;
        lastPanelSig = "";
        renderCommandSidebar();
      });
    }
  }

  // Tool actions click listeners (Repair / Sell)
  const repairBtn = el("action-repair");
  if (repairBtn) {
    repairBtn.addEventListener("click", () => {
      const single = selectedSingle();
      if (single != null) {
        const selEntity = world.entities.get(single);
        if (
          selEntity &&
          BUILDING_KINDS.has(selEntity.kind) &&
          selEntity.owner === 0 &&
          selEntity.hp < selEntity.maxHp
        ) {
          sendCommands([repair(single)]);
        } else {
          toolMode = toolMode === "repair" ? null : "repair";
          if (toolMode) placementMode = null;
          lastPanelSig = "";
          renderCommandSidebar();
        }
      }
    });
  }

  const sellBtn = el("action-sell");
  if (sellBtn) {
    sellBtn.addEventListener("click", () => {
      const single = selectedSingle();
      if (single == null) return;
      const selEntity = world.entities.get(single);
      if (
        selEntity &&
        BUILDING_KINDS.has(selEntity.kind) &&
        selEntity.owner === 0 &&
        selEntity.kind !== "Hq"
      ) {
        // Sell confirmation dialog (U13): prevent misclicks.
        const confirm = el("sell-confirm");
        const confirmText = el("sell-confirm-text");
        confirmText.textContent = `Sell ${selEntity.kind} for ~${Math.floor((BUILD_COSTS[selEntity.kind]?.ore ?? 0) * 0.5)} ore refund?`;
        confirm.classList.remove("hidden");
        el("sell-confirm-yes").onclick = () => {
          sendCommands([sell(single)]);
          confirm.classList.add("hidden");
        };
        el("sell-confirm-no").onclick = () => {
          confirm.classList.add("hidden");
        };
      } else {
        toolMode = toolMode === "sell" ? null : "sell";
        if (toolMode) placementMode = null;
        lastPanelSig = "";
        renderCommandSidebar();
      }
    });
  }

  // Keyboard shortcuts overlay close button.
  const shortcutsClose = el("shortcuts-close");
  if (shortcutsClose) {
    shortcutsClose.addEventListener("click", () => {
      el("shortcuts-overlay").classList.add("hidden");
    });
  }
}

function buildingAt(tx: number, ty: number): Entity | null {
  for (const e of world.ownBuildings) {
    const bx = Math.floor(e.x);
    const by = Math.floor(e.y);
    if (tx === bx && ty === by) {
      return e;
    }
  }
  return null;
}

/** True if any enemy unit or building occupies (or stands within ~1 tile of) the hovered tile. */
function enemyAt(tx: number, ty: number): boolean {
  return enemyEntityAt(tx, ty) != null;
}

/**
 * The enemy entity under the hovered tile — buildings win on an exact tile
 * match, units within ~1 tile of the cursor (nearest wins). Only freshly seen
 * enemies are attackable: a faded last-seen ghost may no longer exist, and the
 * sim would reject the order.
 */
function enemyEntityAt(tx: number, ty: number): Entity | null {
  let best: Entity | null = null;
  let bestD2 = Infinity;
  for (const e of world.enemyEntities) {
    // Recently seen (stale = turns since last sighting; 6-turn memory).
    const stale = e.stale ?? 99;
    if (stale >= 6) continue;
    if (BUILDING_KINDS.has(e.kind)) {
      if (Math.floor(e.x) === tx && Math.floor(e.y) === ty) return e;
    } else if (UNIT_KINDS.has(e.kind)) {
      const dx = e.x - (tx + 0.5);
      const dy = e.y - (ty + 0.5);
      const d2 = dx * dx + dy * dy;
      if (d2 <= 1.0 && d2 < bestD2) {
        best = e;
        bestD2 = d2;
      }
    }
  }
  return best;
}

/** Whether the current selection includes anything that can fire. */
function hasAttackCapableSelection(): boolean {
  for (const id of selection) {
    const e = world.entities.get(id);
    if (!e) continue;
    if (UNIT_KINDS.has(e.kind)) return true;
  }
  return false;
}

/**
 * C&C-style contextual cursor: crosshair over enemies with attack-capable
 * units selected, a $ over sellable buildings, and a wrench over damaged ones.
 */
function updateCursor(sx: number, sy: number): void {
  if (panning) {
    canvas.style.cursor = "grabbing";
    return;
  }
  const [tx, ty] = tileAt(sx, sy);

  if (toolMode === "sell") {
    const b = buildingAt(tx, ty);
    canvas.style.cursor =
      b && b.kind !== "Hq" ? getCursorDataUrl("sell") : "default";
    return;
  }
  if (toolMode === "repair") {
    const b = buildingAt(tx, ty);
    canvas.style.cursor =
      b && b.hp < b.maxHp ? getCursorDataUrl("repair") : "default";
    return;
  }
  if (placementMode) {
    canvas.style.cursor = isBuildingPlacable(placementMode, [tx, ty], world)
      ? "crosshair"
      : "not-allowed";
    return;
  }
  if (hasAttackCapableSelection() && enemyAt(tx, ty)) {
    canvas.style.cursor = getCursorDataUrl("attack");
    return;
  }
  canvas.style.cursor = "default";
}

let lastPanelSig = "";

function cmdButton(
  key: string,
  cost: ResourceBundle,
  onClick: () => void,
  opts: {
    armed?: boolean;
    disabled?: boolean;
    disabledReason?: string;
    power?: { produces: number; consumes: number };
    label?: string;
    badge?: string;
  } = {},
): HTMLButtonElement {
  const thumbUrl = getThumbnailDataUrl(key, 0);
  const b = document.createElement("button");
  b.className = "cmd";
  const displayLabel = opts.label ?? key;
  // Build children via DOM API (never innerHTML) so unit/building names and
  // asset URLs can't inject markup.
  const thumb = document.createElement("img");
  thumb.className = "thumb";
  thumb.alt = key;
  thumb.src = thumbUrl;
  const name = document.createElement("span");
  name.className = "label";
  name.textContent = displayLabel;
  b.append(thumb, name);
  const costLabel = formatResourceCost(cost);
  if (costLabel) {
    const c = document.createElement("span");
    c.className = "cost";
    c.textContent = costLabel;
    b.appendChild(c);
  }
  const cannotAfford = !resourceBundleAffordable(world.resources, cost);
  if (cannotAfford || opts.disabled) {
    b.classList.add("disabled");
    b.title = opts.disabledReason ?? (cannotAfford ? "INSUFFICIENT RESOURCES" : "UNAVAILABLE");
  } else {
    // Unit/Building stat tooltip: show HP, damage, range, MP, build turns on hover.
    const stats = UNIT_STATS[key] ?? BUILD_STATS[key];
    if (stats) {
      const parts: string[] = [];
      if (stats.hp) parts.push(`${stats.hp} HP`);
      if (stats.damage) parts.push(`${stats.damage} DMG`);
      if (stats.range_tiles) parts.push(`R${stats.range_tiles}`);
      if (stats.mp) parts.push(`${stats.mp} MP`);
      if (stats.vision_tiles) parts.push(`${stats.vision_tiles} vis`);
      if (stats.build_time_turns) parts.push(`${stats.build_time_turns} Turn${stats.build_time_turns === 1 ? "" : "s"}`);
      if (stats.air) parts.push("AIR");
      if (stats.aa) parts.push("AA");
      b.title = parts.join(" · ");
    }
  }
  const pwr = opts.power ?? BUILDING_POWER[key];
  if (pwr) {
    const pTag = document.createElement("span");
    if (pwr.produces > 0) {
      pTag.className = "power-tag pos";
      pTag.textContent = `+${pwr.produces} PWR`;
      b.appendChild(pTag);
    } else if (pwr.consumes > 0) {
      pTag.className = "power-tag neg";
      pTag.textContent = `-${pwr.consumes} PWR`;
      b.appendChild(pTag);
    }
  }
  const turns = UNIT_STATS[key]?.build_time_turns ?? BUILD_STATS[key]?.build_time_turns;
  if (turns != null && turns > 0 && !opts.badge) {
    const tTag = document.createElement("span");
    tTag.className = "turns-tag";
    tTag.textContent = `${turns}T`;
    tTag.title = `${turns} Turn${turns === 1 ? "" : "s"} to build`;
    b.appendChild(tTag);
  }
  if (opts.armed) b.classList.add("armed");
  if (opts.badge) {
    const tag = document.createElement("span");
    tag.className = "cmd-badge";
    tag.textContent = opts.badge;
    b.appendChild(tag);
  }
  if (!b.classList.contains("disabled")) b.addEventListener("click", onClick);
  return b;
}

function renderCommandSidebar(): void {
  initToolAndTabIcons();

  const single = selectedSingle();
  const movingCount = selectedUnits().filter((id) => world.entities.get(id)?.moveTarget != null).length;
  const selEntity = single ? world.entities.get(single) : null;
  const isRepairable = selEntity && BUILDING_KINDS.has(selEntity.kind) && selEntity.owner === 0;
  const isSellable = selEntity && BUILDING_KINDS.has(selEntity.kind) && selEntity.owner === 0 && selEntity.kind !== "Hq";

  // Actions row
  const repairBtn = el("action-repair");
  const sellBtn = el("action-sell");
  if (repairBtn) {
    repairBtn.classList.toggle("disabled", !isRepairable);
    if (toolMode === "repair") repairBtn.classList.add("armed");
    else repairBtn.classList.remove("armed");
  }
  if (sellBtn) {
    sellBtn.classList.toggle("disabled", !isSellable);
    if (toolMode === "sell") sellBtn.classList.add("armed");
    else sellBtn.classList.remove("armed");
  }

  const qsig = selEntity && selEntity.queue ? `${selEntity.progress}/${selEntity.buildTime}` : "";
  const bCounts = `${world.ownBuildings.filter((b) => b.kind === "Barracks").length}-${
    world.ownBuildings.filter((b) => b.kind === "Factory").length
  }-${world.ownBuildings.filter((b) => b.kind === "TechLab").length}-${
    world.ownBuildings.filter((b) => b.kind === "Airfield").length
  }`;
  const sig =
    [...selection].sort((a, b) => a - b).join(",") +
    "|" +
    placementMode +
    "|" +
    toolMode +
    "|" +
    activeTab +
    "|" +
    qsig +
    "|" +
    JSON.stringify(world.resources) +
    "|" +
    movingCount +
    "|" +
    bCounts +
    "|" +
    world.research.researched.length +
    "|" +
    world.activePlayer;
  if (sig === lastPanelSig) return;
  lastPanelSig = sig;

  // Update tab buttons active states
  for (const tab of ["buildings", "troops", "vehicles", "aircraft"] as CommandTab[]) {
    const btn = el(`tab-btn-${tab}`);
    if (btn) {
      if (activeTab === tab) btn.classList.add("active");
      else btn.classList.remove("active");
    }
  }

  // Selection Card
  const name = el("sel-name");
  const detail = el("sel-detail");
  const hpwrap = el("sel-hpwrap");
  const hp = el("sel-hp");
  const queue = el("sel-queue");

  if (selEntity) {
    name.textContent = selEntity.kind.toUpperCase();
    const hpText = selEntity.maxHp > 0 ? `${selEntity.hp} / ${selEntity.maxHp} HP` : "";
    const pwr = BUILDING_POWER[selEntity.kind];
    const pwrText = pwr ? (pwr.produces > 0 ? ` · +${pwr.produces} PWR GEN` : ` · -${pwr.consumes} PWR DRAIN`) : "";
    const routeText = selEntity.moveTarget
      ? ` · ROUTE ${selEntity.moveTarget[0]},${selEntity.moveTarget[1]}`
      : selEntity.moved
        ? " · MOVED"
        : "";
    detail.textContent = hpText + pwrText + routeText + (selectedUnits().length > 1 ? ` · ${selection.size} UNITS` : "");
    if (selEntity.maxHp > 0) {
      hpwrap.classList.remove("hidden");
      hp.style.width = `${Math.max(0, Math.min(100, (selEntity.hp / selEntity.maxHp) * 100))}%`;
    } else {
      hpwrap.classList.add("hidden");
    }
    if (selEntity.queue && selEntity.queue.length > 0) {
      queue.classList.remove("hidden");
      const currentUnit = selEntity.queue[0];
      const prog = selEntity.progress ?? 0;
      const total = selEntity.buildTime ?? 1;
      const pct = Math.max(0, Math.min(100, Math.round((prog / total) * 100)));
      const turnsLeft = Math.max(0, total - prog);
      const nextUnits = selEntity.queue.slice(1);
      const nextStr = nextUnits.length > 0 ? ` · NEXT: ${nextUnits.join(", ").toUpperCase()}` : "";
      const thumb = getThumbnailDataUrl(currentUnit, 0);

      const card = document.createElement("div");
      card.className = "civ-prod-card";

      const thumbWrap = document.createElement("div");
      thumbWrap.className = "civ-prod-thumb-wrap";
      const img = document.createElement("img");
      img.className = "civ-prod-thumb";
      img.src = thumb;
      img.alt = currentUnit;
      const overlay = document.createElement("div");
      overlay.className = "civ-prod-grey-overlay";
      overlay.style.height = `${100 - pct}%`;
      const scanline = document.createElement("div");
      scanline.className = "civ-prod-scanline";
      overlay.appendChild(scanline);
      thumbWrap.append(img, overlay);

      const info = document.createElement("div");
      info.className = "civ-prod-info";
      const title = document.createElement("div");
      title.className = "civ-prod-title";
      title.textContent = `PRODUCING: ${currentUnit.toUpperCase()}`;
      const sub = document.createElement("div");
      sub.className = "civ-prod-sub";
      sub.textContent =
        turnsLeft > 0
          ? `TURN ${prog + 1} OF ${total} (${turnsLeft}T LEFT)`
          : `READY NEXT TURN`;
      if (nextStr) sub.textContent += nextStr;
      const bar = document.createElement("div");
      bar.className = "queue-bar";
      const barFill = document.createElement("div");
      barFill.style.width = `${pct}%`;
      bar.appendChild(barFill);
      info.append(title, sub, bar);
      card.append(thumbWrap, info);
      queue.replaceChildren(card);
    } else {
      queue.classList.add("hidden");
    }
  } else if (selection.size > 0) {
    const kinds = new Map<string, number>();
    for (const id of selection) {
      const e = world.entities.get(id);
      if (e) kinds.set(e.kind, (kinds.get(e.kind) ?? 0) + 1);
    }
    name.textContent = [...kinds.entries()].map(([k, n]) => `${n}× ${k.toUpperCase()}`).join(", ");
    detail.textContent = movingCount > 0
      ? `ROUTE ACTIVE · ${movingCount} UNIT${movingCount === 1 ? "" : "S"}`
      : "RIGHT-CLICK TO ATTACK-MOVE";
    hpwrap.classList.add("hidden");
    queue.classList.add("hidden");
  } else {
    name.textContent = "NO SELECTION";
    detail.textContent = "SELECT BUILDING OR UNITS";
    hpwrap.classList.add("hidden");
    queue.classList.add("hidden");
  }

  // Command Menu Grid
  const grid = el("cmd-grid");
  const empty = el("cmd-empty");
  grid.innerHTML = "";
  empty.classList.add("hidden");

  // Durable routes can be cancelled without changing the units' current
  // position or movement points. This remains visible after a turn boundary
  // because the destination is part of the server state.
  const movingUnits = selectedUnits().filter((id) => world.entities.get(id)?.moveTarget != null);
  if (movingUnits.length > 0) {
    grid.appendChild(
      cmdButton(
        "clear_move",
        { ore: 0, steel: 0, coal: 0, crystal: 0 },
        () => {
          sendCommands([clearMove(movingUnits)]);
          lastPanelSig = "";
          renderCommandSidebar();
        },
        { label: "Clear Route", badge: `${movingUnits.length} ACTIVE` },
      ),
    );
  }

  // Tech Lab selected: a RESEARCH button opens the tech overlay (Civ-style
  // tree, not a one-shot card). The ribbon shows live progress.
  if (selEntity && selEntity.kind === "TechLab" && selEntity.owner === 0) {
    const r = world.research;
    const researching = r.researching ? TECH_INFO[r.researching] : null;
    grid.appendChild(
      cmdButton(
        "Research",
        { ore: 0, steel: 0, coal: 0, crystal: 0 },
        () => openResearch(),
        {
          armed: false,
          badge: researching
            ? `${researching.name} · ${r.points}/${researching.researchCost} pts`
            : r.researched.length > 0
              ? `${r.researched.length} TECH`
              : "OPEN TREE",
        },
      ),
    );
  }

  if (activeTab === "buildings") {
    const hasFactory = world.ownBuildings.some((b) => b.kind === "Factory");
    const hasLab = world.ownBuildings.some((b) => b.kind === "TechLab");
    const buildings: BuildingType[] = [
      "PowerPlant", "Refinery", "CrystalRefinery", "Barracks", "Factory",
      "TechLab", "Airfield", "Radar", "TeslaCoil", "Turret", "AATurret",
    ];
    for (const b of buildings) {
      // Tech tree: TechLab & Airfield need a Factory; Radar, TeslaCoil and
      // the AATurret are the second tier and need the TechLab itself.
      const needsFactory = b === "TechLab" || b === "Airfield";
      const needsLab = b === "Radar" || b === "TeslaCoil" || b === "AATurret";
      const isLocked = (needsFactory && !hasFactory) || (needsLab && !hasLab);
      const reason = needsLab && !hasLab ? "REQUIRES TECH LAB" : needsFactory && !hasFactory ? "REQUIRES FACTORY" : undefined;
      grid.appendChild(
        cmdButton(
          b,
          BUILD_COSTS[b],
          () => {
            toolMode = null;
            placementMode = placementMode === b ? null : b;
            placementCursor = null;
            lastPanelSig = "";
            renderCommandSidebar();
          },
          {
            armed: placementMode === b,
            disabled: isLocked,
            disabledReason: reason,
          },
        ),
      );
    }
  } else if (activeTab === "troops") {
    const barracks = world.ownBuildings.find((b) => b.kind === "Barracks");
    const hasRockets = world.research.researched.includes("RocketPropulsion" as TechId);
    const troops: UnitType[] = ["Infantry", "Scout", "RocketTrooper"];
    for (const t of troops) {
      const needsTech = t === "RocketTrooper" && !hasRockets;
      const isLocked = !barracks || needsTech;
      const reason = needsTech ? "RESEARCH ROCKET PROPULSION" : !barracks ? "REQUIRES BARRACKS" : undefined;
      grid.appendChild(
        cmdButton(
          t,
          UNIT_COSTS[t],
          () => {
            if (barracks && !needsTech) {
              sendCommands([trainUnit(barracks.id, t)]);
            }
          },
          {
            disabled: isLocked,
            disabledReason: reason,
          },
        ),
      );
    }
  } else if (activeTab === "vehicles") {
    const factory = world.ownBuildings.find((b) => b.kind === "Factory");
    const hasLab = world.ownBuildings.some((b) => b.kind === "TechLab");
    const hasRockets = world.research.researched.includes("RocketPropulsion" as TechId);
    const vehicles: UnitType[] = ["Tank", "Artillery", "MammothTank", "SamLauncher"];
    for (const v of vehicles) {
      const needsLab = v === "Artillery" || v === "MammothTank";
      const needsTech = v === "SamLauncher" && !hasRockets;
      const isLocked = !factory || (needsLab && !hasLab) || needsTech;
      const reason = needsTech
        ? "RESEARCH ROCKET PROPULSION"
        : !factory
          ? "REQUIRES FACTORY"
          : needsLab && !hasLab
            ? "REQUIRES TECH LAB"
            : undefined;
      grid.appendChild(
        cmdButton(
          v,
          UNIT_COSTS[v],
          () => {
            if (factory && (!needsLab || hasLab) && !needsTech) {
              sendCommands([trainUnit(factory.id, v)]);
            }
          },
          {
            disabled: isLocked,
            disabledReason: reason,
          },
        ),
      );
    }
  } else if (activeTab === "aircraft") {
    const airfield = world.ownBuildings.find((b) => b.kind === "Airfield");
    const aircraft: UnitType[] = ["Gunship", "Interceptor"];
    for (const a of aircraft) {
      grid.appendChild(
        cmdButton(
          a,
          UNIT_COSTS[a],
          () => {
            if (airfield) {
              sendCommands([trainUnit(airfield.id, a)]);
            }
          },
          {
            disabled: !airfield,
            disabledReason: airfield ? undefined : "REQUIRES AIRFIELD",
          },
        ),
      );
    }
  }
}

// ---------------------------------------------------------------------------
// Research overlay (Civ-style tech tree)
// ---------------------------------------------------------------------------

/** Whether the research overlay is open. */
let researchOpen = false;

function openResearch(): void {
  if (!inGame) return;
  researchOpen = true;
  renderResearch();
  renderBuildTree();
  bindResearchTabs();
  el("research-overlay").classList.remove("hidden");
}

function closeResearch(): void {
  researchOpen = false;
  el("research-overlay").classList.add("hidden");
}

let researchTabsBound = false;
function bindResearchTabs(): void {
  if (researchTabsBound) return;
  researchTabsBound = true;
  const tabTech = el("research-tab-tech");
  const tabBuild = el("research-tab-build");
  const tree = el("research-tree");
  const build = el("research-build");
  const show = (tech: boolean) => {
    tabTech.classList.toggle("active", tech);
    tabBuild.classList.toggle("active", !tech);
    tree.classList.toggle("hidden", !tech);
    build.classList.toggle("hidden", tech);
  };
  tabTech.addEventListener("click", () => show(true));
  tabBuild.addEventListener("click", () => show(false));
}

/** U3: build-tree tab — which building produces which units and what gates
 *  each unit (tech researched, secondary building). Read-only dependency
 *  reference drawn from the same data the server validates against. */
function renderBuildTree(): void {
  const host = el("research-build");
  host.replaceChildren();
  const buildOrder = [
    "Barracks",
    "Factory",
    "Airfield",
    "Refinery",
    "CrystalRefinery",
    "PowerPlant",
    "TechLab",
    "Radar",
    "Turret",
    "TeslaCoil",
    "AATurret",
  ];
  const doneBuildings = new Set(world.ownBuildings.map((b) => b.kind));
  const researched = new Set(world.research.researched);

  for (const bt of buildOrder) {
    const node = document.createElement("div");
    node.className = "build-node";
    const head = document.createElement("div");
    head.className = "build-node-head";
    const thumb = document.createElement("img");
    thumb.className = "thumb";
    thumb.src = getThumbnailDataUrl(bt, 0);
    thumb.alt = bt;
    const name = document.createElement("span");
    name.className = "build-node-name";
    name.textContent = bt.toUpperCase();
    head.appendChild(thumb);
    head.appendChild(name);
    const stats = BUILD_STATS[bt];
    if (stats) {
      const s = document.createElement("span");
      s.className = "build-node-req";
      const parts: string[] = [];
      if (stats.hp) parts.push(`${stats.hp} HP`);
      const req = BUILDING_PREREQS[bt];
      if (req) parts.push(`requires ${req}`);
      s.textContent = parts.join(" · ");
      head.appendChild(s);
    }
    node.appendChild(head);

    // Units produced here, with their gates.
    const produced = Object.entries(UNIT_TREE).filter(([, v]) => v.building === bt);
    for (const [uk, v] of produced) {
      const chip = document.createElement("span");
      chip.className = "build-unit";
      const uThumb = document.createElement("img");
      uThumb.className = "thumb";
      uThumb.src = getThumbnailDataUrl(uk, 0);
      uThumb.alt = uk;
      const uName = document.createElement("span");
      uName.textContent = uk;
      chip.appendChild(uThumb);
      chip.appendChild(uName);
      const gates: string[] = [];
      if (v.buildingReq && !doneBuildings.has(v.buildingReq)) {
        gates.push(`needs ${v.buildingReq}`);
      }
      if (v.tech && !researched.has(v.tech)) {
        gates.push(`research ${v.tech}`);
      }
      if (gates.length > 0) {
        const g = document.createElement("span");
        g.className = gates.some((x) => x.startsWith("research")) ? "gate tech" : "gate";
        g.textContent = gates.join(", ");
        chip.appendChild(g);
      }
      node.appendChild(chip);
    }
    const status = document.createElement("span");
    status.className = "build-node-req";
    status.textContent = doneBuildings.has(bt) ? "✓ BUILT" : "";
    head.appendChild(status);
    host.appendChild(node);
  }
}

/** The research tree ordering, tier by tier (mirrors tech.rs). */
const TECH_ORDER: TechId[] = [
  "HighExplosive", "CompositeArmor", "TargetingOptics", "EfficientRefining",
  "RocketPropulsion", "TitaniumAlloys", "AerialSuperiority", "Superconductors",
  "CrystalNanotech", "AdvancedBallistics",
];

/** Tier index per tech: tier = depth of the deepest prereq chain. */
function techTier(id: TechId): number {
  const info = TECH_INFO[id];
  if (info.prereqs.length === 0) return 0;
  return 1 + Math.max(...info.prereqs.map((p) => techTier(p)));
}

function renderResearch(): void {
  const r = world.research;
  const hasLab = world.ownBuildings.some((b) => b.kind === "TechLab");
  const canStart =
    hasLab && r.researching == null && world.activePlayer === 0 && !world.result;

  el("research-points").textContent = `${r.points} pts`;
  el("research-crystal").textContent = `${world.crystal} crystal`;

  const tree = el("research-tree");
  tree.innerHTML = "";
  const tiers = new Map<number, TechId[]>();
  for (const id of TECH_ORDER) {
    const t = techTier(id);
    const bucket = tiers.get(t);
    if (bucket) bucket.push(id);
    else tiers.set(t, [id]);
  }
  const sortedTiers = [...tiers.entries()].sort((a, b) => a[0] - b[0]);
  for (const [tier, ids] of sortedTiers) {
    const row = document.createElement("div");
    row.className = "research-row";
    row.dataset.tier = String(tier);
    for (const id of ids) {
      row.appendChild(researchCard(id, canStart));
    }
    tree.appendChild(row);
  }
}

function researchCard(id: TechId, canStart: boolean): HTMLElement {
  const info = TECH_INFO[id];
  const r = world.research;
  const done = r.researched.includes(id);
  const active = r.researching === id;
  const prereqsMet = info.prereqs.every((p) => r.researched.includes(p));
  const crystalOk = world.crystal >= info.crystalCost;
  const locked = !prereqsMet || (info.crystalCost > 0 && !crystalOk);
  const disabled = done || active || !canStart || locked;

  const card = document.createElement("button");
  card.className = "research-card" + (done ? " done" : "") + (active ? " active" : "");
  card.type = "button";
  // Build children via DOM API (never innerHTML) so tech names/descriptions
  // can't inject markup.
  const nameEl = document.createElement("div");
  nameEl.className = "research-name";
  nameEl.textContent = info.name;
  const descEl = document.createElement("div");
  descEl.className = "research-desc";
  descEl.textContent = info.description;
  const costEl = document.createElement("div");
  costEl.className = "research-cost";
  costEl.textContent = `${info.researchCost} pts`;
  if (info.crystalCost > 0) costEl.textContent += ` · ${info.crystalCost} crystal`;
  if (active) {
    costEl.textContent += ` · ${Math.min(100, Math.round((r.points / info.researchCost) * 100))}%`;
  }
  card.append(nameEl, descEl, costEl);
  if (active) card.classList.add("progress");
  if (disabled) {
    card.disabled = true;
    if (locked && !done && !active) {
      const why = !prereqsMet ? "PREREQS NOT MET" : "NEEDS CRYSTAL";
      const note = document.createElement("div");
      note.className = "research-lock";
      note.textContent = why;
      card.appendChild(note);
    }
  }
  card.addEventListener("click", () => {
    if (!disabled) {
      sendCommands([startResearch(id)]);
      closeResearch();
      renderCommandSidebar();
    }
  });
  return card;
}

let lastRenderedLogCount = -1;

function renderLog(): void {
  const log = el("log-body");
  if (intel.entries.length === lastRenderedLogCount) {
    return;
  }
  lastRenderedLogCount = intel.entries.length;
  log.innerHTML = "";
  const GLYPHS: Record<string, string> = {
    info: "◆",
    prod: "▲",
    warn: "⚠",
    danger: "✖",
    kill: "✓",
  };
  for (const entry of intel.entries) {
    const row = document.createElement("div");
    row.className = `log-entry log-entry-${entry.level}`;

    const glyph = document.createElement("span");
    glyph.className = "log-glyph";
    glyph.textContent = GLYPHS[entry.level] ?? "•";

    const timeSpan = document.createElement("span");
    timeSpan.className = "log-time";
    timeSpan.textContent = `[T${entry.turn}]`;

    const tagSpan = document.createElement("span");
    tagSpan.className = `log-tag log-tag-${entry.level}`;
    tagSpan.textContent = entry.tag;

    const msgSpan = document.createElement("span");
    msgSpan.className = `log-msg log-msg-${entry.level}`;
    msgSpan.textContent = entry.text;

    row.appendChild(glyph);
    row.appendChild(timeSpan);
    row.appendChild(tagSpan);
    row.appendChild(msgSpan);
    log.appendChild(row);
  }
  log.scrollTop = log.scrollHeight;
}

function formatTurns(turns: number): string {
  return String(Math.max(0, turns));
}

function renderResourceReadouts(): void {
  el("ore").textContent = String(world.resources.ore);
  el("steel").textContent = String(world.resources.steel);
  el("coal").textContent = String(world.resources.coal);
  el("crystal").textContent = String(world.resources.crystal);
  const income = formatResourceCost(world.income);
  el("income").textContent = income ? `+${income}/turn` : "";

  // Action budget bar (F11): show how many actions remain this turn.
  const budgetBar = el("action-budget-bar");
  if (budgetBar) {
    if (inGame && world.activePlayer === 0) {
      budgetBar.classList.remove("hidden");
      const spent = world.budgetSpent ?? 0;
      const cap = world.budgetCap ?? 16;
      const pct = Math.max(0, Math.min(100, ((cap - spent) / cap) * 100));
      el("action-budget-fill").style.width = `${pct}%`;
      el("action-budget-fill").style.background =
        pct < 20 ? "var(--red)" : pct < 50 ? "var(--gold-bright)" : "var(--gold)";
    } else {
      budgetBar.classList.add("hidden");
    }
  }
}

function inspectorLine(parent: HTMLElement, key: string, value: string, className = ""): void {
  const row = document.createElement("div");
  row.className = "inspector-row";
  const keyNode = document.createElement("span");
  keyNode.className = "inspector-key";
  keyNode.textContent = key;
  const valueNode = document.createElement("span");
  valueNode.className = `inspector-value${className ? ` ${className}` : ""}`;
  valueNode.textContent = value;
  row.append(keyNode, valueNode);
  parent.appendChild(row);
}

function inspectorEmpty(parent: HTMLElement, text: string): void {
  const node = document.createElement("div");
  node.className = "inspector-empty";
  node.textContent = text;
  parent.appendChild(node);
}

/** Render the selected tile's progressive-disclosure report. */
function renderTileInspector(): void {
  const panel = document.getElementById("inspector");
  if (!panel) return;
  if (!inGame || !selectedTile) {
    panel.classList.add("hidden");
    lastInspectorSig = "";
    return;
  }

  const info = inspectionForTile(world, selectedTile[0], selectedTile[1], selection);
  const sig = `${selectedTile[0]},${selectedTile[1]}|${world.turn}|${JSON.stringify(info)}|${JSON.stringify([...selection].sort((a, b) => a - b))}`;
  if (sig === lastInspectorSig) return;
  lastInspectorSig = sig;
  panel.classList.remove("hidden");

  el("inspector-coords").textContent = `TILE ${info.x},${info.y}`;
  el("inspector-visibility").textContent = `${info.visibility.toUpperCase()} · ${info.authoritative ? "SERVER VERIFIED" : "LOCAL PREVIEW"}`;
  const inspectorIcon = document.getElementById("inspector-icon") as HTMLImageElement | null;
  if (inspectorIcon) {
    inspectorIcon.src = info.resource
      ? getAssetUrl("resources", info.resource.resource)
      : info.terrain
        ? getAssetUrl("terrain", info.terrain.kind)
        : getAssetUrl("ui", "inspect");
  }
  el("inspector-visibility").className = `inspector-visibility${info.authoritative ? " inspector-verified" : ""}`;

  const terrain = el("inspector-terrain");
  terrain.replaceChildren();
  if (!info.terrain) {
    inspectorEmpty(terrain, "Terrain and movement data are hidden. Scout this tile to reveal it.");
  } else {
    inspectorLine(terrain, "type", `${info.terrain.label} · ${info.terrain.tacticalTag}`);
    if (info.elevation != null || info.moisture != null) {
      const temp =
        info.temperature == null ? "" : ` · ${tempLabel(info.temperature)}`;
      inspectorLine(
        terrain,
        "landform",
        `elevation ${info.elevation ?? "—"} · moisture ${info.moisture ?? "—"}${temp}`,
      );
    }
    inspectorLine(
      terrain,
      "movement",
      info.terrain.passable ? `×${info.terrain.moveMultiplier} entry cost` : "impassable",
      info.terrain.passable ? "inspector-good" : "inspector-bad",
    );
    if (info.terrain.defenseReduction > 0) {
      inspectorLine(terrain, "defense", `-${info.terrain.defenseReduction}% incoming damage`, "inspector-good");
    }
    const movement = selection.size > 0
      ? info.movement.filter((entry) => selection.has(entry.unitId))
      : info.movement.slice(0, 6);
    if (movement.length === 0) {
      inspectorEmpty(terrain, "Select a unit to compare its movement points with this tile.");
    } else {
      for (const entry of movement) {
        inspectorLine(
          terrain,
          `#${entry.unitId} ${entry.unitKind}`,
          `${entry.movePoints} MP · ${entry.terrainCost} to enter · ${entry.canEnter ? "can enter" : "blocked"}`,
          entry.canEnter ? "inspector-good" : "inspector-bad",
        );
      }
    }
  }

  const resource = el("inspector-resource");
  resource.replaceChildren();
  if (!info.resource) {
    inspectorEmpty(resource, info.visibility === "unexplored" ? "No deposit data — this tile has not been scouted." : "No known deposit on this tile.");
  } else {
    const richness = ["", "Poor", "Standard", "Rich"][Math.max(0, Math.min(3, info.resource.richness))] || "Unknown";
    inspectorLine(resource, "deposit", `${info.resource.resource} · ${richness}`);
    const yieldText = info.resource.yieldPerTurn != null ? `+${info.resource.yieldPerTurn}/turn` : "yield on claim";
    inspectorLine(resource, "supply", `INFINITE · ${yieldText}`, "inspector-good");
    if (info.resource.refineryOwner != null) {
      inspectorLine(resource, "refinery", `Player ${info.resource.refineryOwner} · ${yieldText}`, "inspector-good");
    } else {
      inspectorLine(resource, "refinery", "Unclaimed · build directly on this tile", "inspector-warn");
    }
  }

  const occupants = el("inspector-occupants");
  occupants.replaceChildren();
  if (info.occupants.length === 0) {
    inspectorEmpty(occupants, "No known occupants.");
  } else {
    for (const occupant of info.occupants) {
      const side = occupant.owner === 0 ? "friendly" : occupant.stale ? `enemy · seen ${occupant.stale}t ago` : "enemy";
      const hp = occupant.maxHp > 0 ? ` · ${occupant.hp}/${occupant.maxHp} HP` : "";
      inspectorLine(occupants, `#${occupant.id}`, `${occupant.kind} · ${side}${hp}`, occupant.owner === 0 ? "inspector-good" : occupant.stale ? "" : "inspector-warn");
    }
  }

  const routes = el("inspector-routes");
  routes.replaceChildren();
  if (info.routeTargets.length === 0) {
    inspectorEmpty(routes, "No durable routes currently target from this tile.");
  } else {
    for (const route of info.routeTargets) {
      inspectorLine(routes, `unit #${route.id}`, `destination ${route.target[0]},${route.target[1]}`);
    }
  }

  const placement = el("inspector-placement");
  placement.replaceChildren();
  if (!info.placement.known) {
    inspectorEmpty(placement, "Placement facts are hidden until this tile is scouted.");
  } else {
    inspectorLine(placement, "terrain", info.placement.passable ? "Passable" : "Impassable", info.placement.passable ? "inspector-good" : "inspector-bad");
    if (info.placement.occupiedByBuilding) inspectorLine(placement, "occupied", "Building footprint", "inspector-bad");
    else if (info.placement.occupiedByUnit) inspectorLine(placement, "occupied", "Unit footprint", "inspector-bad");
    else inspectorLine(placement, "occupied", "Clear", "inspector-good");
    inspectorLine(placement, "base radius", info.placement.withinBaseRadius ? "Within build radius" : "Outside build radius", info.placement.withinBaseRadius ? "inspector-good" : "inspector-warn");
    inspectorLine(placement, "structure", info.placement.structureSiteAvailable ? "Site available" : "Not available", info.placement.structureSiteAvailable ? "inspector-good" : "inspector-warn");
    inspectorLine(placement, "refinery", info.placement.refinerySiteAvailable ? "Deposit claim available" : "No legal claim", info.placement.refinerySiteAvailable ? "inspector-good" : "inspector-warn");
  }
}

function initTileInspector(): void {
  document.getElementById("inspector-close")?.addEventListener("click", () => {
    selectedTile = null;
    world.clearTileInspection();
    lastInspectorSig = "";
    renderTileInspector();
  });
}

// ---------------------------------------------------------------------------
// Main Render Loop & Menu Battle Theater
// ---------------------------------------------------------------------------

function resize(): void {
  // The canvas is an inset battlefield while a match is active and a full
  // window cinematic surface in the menu. Size its backing store from the
  // actual CSS box so camera math and pointer coordinates share one viewport.
  const fallbackW = window.innerWidth || document.documentElement.clientWidth || 800;
  const fallbackH = window.innerHeight || document.documentElement.clientHeight || 600;
  // The menu canvas always covers the window; in-match it is the battlefield
  // inset between the top bar and the command rail. Fall back to window
  // metrics when the CSS box is degenerate (e.g. a stale stylesheet leaves
  // the canvas at its 300×150 intrinsic default).
  const inMatch = document.body.classList.contains("in-match");
  const rect = canvas.getBoundingClientRect();
  const w = inMatch
    ? (rect.width >= 100 ? Math.floor(rect.width) : fallbackW)
    : fallbackW;
  const h = inMatch
    ? (rect.height >= 100 ? Math.floor(rect.height) : fallbackH)
    : fallbackH;
  canvas.width = w;
  canvas.height = h;
  renderer.camera.setViewport(w, h);
  menuRenderer.camera.setViewport(w, h);
  spectate.renderer.camera.setViewport(w, h);
}
window.addEventListener("resize", resize);
// Re-size once the tab becomes visible again (a browser may launch the page
// hidden, then expand it without firing another `resize`).
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) resize();
});
resize();

/** Deterministic cosmetic terrain for the menu backdrop: low-frequency
 *  climate noise (moisture/elevation/temperature) condensed into biome
 *  patches, so the lobby shows off the real biome tiles. Cosmetic only —
 *  live matches always use the server's authoritative generator. */
function generateDemoTerrain(seed: number): { terrain: string[]; passable: boolean[] } {
  const terrain: string[] = Array.from({ length: MAP_TILES }, () => "Plains");
  const passable: boolean[] = Array.from({ length: MAP_TILES }, () => true);
  // Deterministic integer demo-terrain hash. All constants must be Number-safe
  // (< 2^53): the original 64-bit prime overflows JS Numbers and loses the
  // intended avalanche. Keep them small and exact.
  const hash = (x: number, y: number, salt: number): number => {
    let z = ((x * 2654435761 + y * 40503 + salt * 74996233) ^ (seed >>> 0)) >>> 0;
    z = (z ^ (z >> 13)) * 1274126143;
    return (z ^ (z >> 16)) >>> 0;
  };
  const noise = (x: number, y: number, salt: number): number => {
    // Bilinear-interpolated lattice noise for smooth patches.
    const cell = Math.max(4, Math.floor(MAP_SIZE / 16));
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
  // A river channel across the middle with banks.
  const riverY = (x: number): number => MAP_SIZE / 2 + Math.round(Math.sin(x / 14) * 10);
  for (let y = 0; y < MAP_SIZE; y++) {
    for (let x = 0; x < MAP_SIZE; x++) {
      const idx = y * MAP_SIZE + x;
      if (Math.abs(y - riverY(x)) <= 1) {
        terrain[idx] = "River";
        continue;
      }
      const elev = noise(x, y, 0x11);
      const moist = noise(x, y, 0x22);
      const temp = 40 + (32 - Math.abs(y - 32)) * 5 + (hash(x, y, 0x33) % 40) - 20;
      if (elev > 205) terrain[idx] = "Mountain";
      else if (elev > 175 && moist < 140) terrain[idx] = "Hills";
      else if (moist > 200 && temp > 130) terrain[idx] = "Swamp";
      else if (moist < 70 && temp > 90) terrain[idx] = "Desert";
      else if (moist > 150) terrain[idx] = "Forest";
      else terrain[idx] = "Plains";
      passable[idx] = !["Mountain"].includes(terrain[idx]);
    }
  }
  // Two lakes.
  for (let y = 16; y < 40; y++) {
    for (let x = 16; x < 40; x++) {
      if ((x - 28) ** 2 + (y - 28) ** 2 < 88) terrain[y * MAP_SIZE + x] = "Water";
    }
  }
  for (let y = 88; y < 116; y++) {
    for (let x = 92; x < 120; x++) {
      if ((x - 105) ** 2 + (y - 102) ** 2 < 104) terrain[y * MAP_SIZE + x] = "Water";
    }
  }
  return { terrain, passable };
}

function initMenuTour(): void {
  // The menu is a deterministic map tour rather than a fake combat match.
  const seed = 42;
  const demo = generateDemoTerrain(seed);
  menuWorld.setMap(seed, demo.passable, demo.terrain, [[20, 20], [108, 108]]);
  menuWorld.resourceTiles.clear();
  menuWorld.resourceTiles.set("28,20", { x: 28, y: 20, resource: "Ore", amount: 600, richness: 3, infinite: true });
  menuWorld.resourceTiles.set("99,108", { x: 99, y: 108, resource: "Steel", amount: 500, richness: 2, infinite: true });
  menuWorld.resourceTiles.set("64,64", { x: 64, y: 64, resource: "Coal", amount: 700, richness: 2, infinite: true });
  menuWorld.visible = new Set(Array.from({ length: MAP_TILES }, (_, i) => i));
  menuWorld.explored = new Set(menuWorld.visible);
  menuInit = true;
}

let lastFrame = performance.now();

function frame(ts: number): void {
  const dt = Math.min(100, Math.max(1, ts - lastFrame));
  const dtSec = dt / 1000;
  lastFrame = ts;

  // Step FX system
  fx.update(dtSec);

  // Keyboard camera panning in-game or in-spectate
  const panSpeed = (400 * dt) / 1000;
  if (spectate.active) {
    if (keysPressed.has("KeyW") || keysPressed.has("ArrowUp")) spectate.renderer.camera.pan(0, panSpeed, canvas.width, canvas.height);
    if (keysPressed.has("KeyS") || keysPressed.has("ArrowDown")) spectate.renderer.camera.pan(0, -panSpeed, canvas.width, canvas.height);
    if (keysPressed.has("KeyA") || keysPressed.has("ArrowLeft")) spectate.renderer.camera.pan(panSpeed, 0, canvas.width, canvas.height);
    if (keysPressed.has("KeyD") || keysPressed.has("ArrowRight")) spectate.renderer.camera.pan(-panSpeed, 0, canvas.width, canvas.height);
  } else if (inGame) {
    if (keysPressed.has("KeyW") || keysPressed.has("ArrowUp")) renderer.camera.pan(0, panSpeed, canvas.width, canvas.height);
    if (keysPressed.has("KeyS") || keysPressed.has("ArrowDown")) renderer.camera.pan(0, -panSpeed, canvas.width, canvas.height);
    if (keysPressed.has("KeyA") || keysPressed.has("ArrowLeft")) renderer.camera.pan(panSpeed, 0, canvas.width, canvas.height);
    if (keysPressed.has("KeyD") || keysPressed.has("ArrowRight")) renderer.camera.pan(-panSpeed, 0, canvas.width, canvas.height);

    // Replay queued combat one attack at a time (never all in one frame) and
    // focus the camera on each, so an enemy turn's attacks are never missed.
    if (combatQueue.length > 0 && performance.now() >= nextAttackAt) {
      const atk = combatQueue.shift();
      if (atk) playCombatAttack(atk);
    }
  }

  ctx.fillStyle = "#04060a";
  ctx.fillRect(0, 0, canvas.width, canvas.height);

  if (spectate.active) {
    spectate.draw(ctx, canvas.width, canvas.height);
    if (radarCtx && radarCanvas) {
      drawRadar(radarCtx, spectate.world, new Set(), spectate.renderer.camera, radarCanvas.width, radarCanvas.height);
    }
    el("opponent").textContent = "SPECTATING REPLAY";
    el("ore").textContent = `${spectate.score0} · ${spectate.score1}`;
    el("steel").textContent = "—";
    el("coal").textContent = "—";
    el("crystal").textContent = "—";
    el("income").textContent = "";
    el("turn").textContent = String(spectate.currentTurn);
    requestAnimationFrame(frame);
    return;
  }

  if (!inGame) {
    // Slow deterministic map tour: the menu shows terrain and resources
    // without a fake combat loop stealing focus from the actual game.
    // Zoom in far enough that only a bounded tile window is drawn — the
    // full 128x128 map at zoom 9 is ~16k tiles per frame and drags the
    // menu to a crawl. The sweep radius keeps the view inside the map so
    // the backdrop never shows void at the edges.
    if (!menuInit) initMenuTour();
    demoTime += dtSec;
    const sweepAngle = demoTime * 0.035;
    const zoom = Math.max(28, Math.ceil(Math.max(canvas.width / 44, canvas.height / 26)));
    const halfW = canvas.width / (2 * zoom);
    const halfH = canvas.height / (2 * zoom);
    const sweepX = 64 + Math.cos(sweepAngle) * Math.min(20, Math.max(6, halfW - 6));
    const sweepY = 64 + Math.sin(sweepAngle * 0.83) * Math.min(13, Math.max(4, halfH - 6));
    menuRenderer.camera.focusOn(sweepX, sweepY, zoom, canvas.width, canvas.height);

    menuRenderer.draw(ctx, menuWorld, new Set(), canvas.width, canvas.height);
  } else {
    // Clean up completed unit waypoints
    for (const [uid, wp] of unitWaypoints) {
      const e = world.entities.get(uid);
      if (!e || Math.hypot(e.x - (wp[0] + 0.5), e.y - (wp[1] + 0.5)) < 0.6) {
        unitWaypoints.delete(uid);
      }
    }

    renderer.draw(ctx, world, selection, canvas.width, canvas.height, {
      waypoints: unitWaypoints,
      selectedTile,
      placementMode,
      placementCursor,
      hoverTile,
    });

    // Draw live Radar Surveillance in Command Sidebar
    if (radarCtx && radarCanvas) {
      drawRadar(radarCtx, world, selection, renderer.camera, radarCanvas.width, radarCanvas.height);
    }

    // Box selection drag rectangle
    if (dragStart && dragCurrent) {
      ctx.strokeStyle = "rgba(255, 226, 122, 0.85)";
      ctx.lineWidth = 1.5;
      ctx.setLineDash([4, 4]);
      const x = Math.min(dragStart[0], dragCurrent[0]);
      const y = Math.min(dragStart[1], dragCurrent[1]);
      ctx.strokeRect(x, y, Math.abs(dragCurrent[0] - dragStart[0]), Math.abs(dragCurrent[1] - dragStart[1]));
      ctx.setLineDash([]);
    }

    renderResourceReadouts();

    const pwr = world.ownPower;
    el("power-used").textContent = String(pwr.consumed);
    el("power-total").textContent = String(pwr.produced);
    const pwrRes = document.getElementById("power-res");
    if (pwrRes) {
      if (pwr.consumed > pwr.produced) {
        pwrRes.classList.add("low-power");
      } else {
        pwrRes.classList.remove("low-power");
      }
    }
    intel.processPowerStatus(world.turn, pwr.produced, pwr.consumed);

    renderCommandSidebar();
    renderTileInspector();
    renderLog();
  }

  requestAnimationFrame(frame);
}

// ---------------------------------------------------------------------------
// Opponent Picker
// ---------------------------------------------------------------------------

interface ChampionInfo {
  genome_id: number;
  generation: number;
  reigning: boolean;
  elo: number | null;
}

async function initOpponentPicker(): Promise<void> {
  const championBtn = document.getElementById("champion-btn") as HTMLButtonElement | null;
  const museumRow = el("museum-opponents");
  try {
    const [champRes, museumRes] = await Promise.all([
      fetch("/api/champion"),
      fetch("/api/museum"),
    ]);
    const champ = (await champRes.json()) as { champion: ChampionInfo | null };
    const museum = (await museumRes.json()) as { champions: ChampionInfo[] };

    if (championBtn) {
      if (champ.champion) {
        const c = champ.champion;
        const elo = c.elo == null ? "" : ` · ELO ${Math.round(c.elo)}`;
        championBtn.textContent = `CHAMPION (GEN ${c.generation}${elo})`;
        championBtn.disabled = false;
      } else {
        championBtn.textContent = "CHAMPION (NONE CROWNED)";
        championBtn.disabled = true;
      }
    }

    museumRow.innerHTML = "";
    const nonReigning = museum.champions.filter((c) => !c.reigning);
    // Last 6 dethroned champions, most-recent first (no mutating `.reverse()`).
    const bosses: { genome_id: number; generation: number }[] = [];
    for (let i = nonReigning.length - 1; i >= 0 && bosses.length < 6; i--) {
      bosses.push(nonReigning[i]);
    }
    if (bosses.length > 0) {
      const label = document.createElement("span");
      label.className = "muted";
      label.textContent = "MUSEUM ARCHIVES:";
      museumRow.appendChild(label);
      for (const c of bosses) {
        const b = document.createElement("button");
        b.className = "btn";
        b.textContent = `#${c.genome_id} (GEN ${c.generation})`;
        b.addEventListener("click", () => startMatch(`museum:${c.genome_id}`, `CHAMPION #${c.genome_id}`));
        museumRow.appendChild(b);
      }
    }
  } catch {
    if (championBtn) {
      championBtn.textContent = "CHAMPION (OFFLINE)";
      championBtn.disabled = true;
    }
  }
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

document.querySelectorAll<HTMLButtonElement>("[data-opp]").forEach((btn) => {
  const opp = btn.dataset.opp;
  if (opp != null) {
    const label = btn.dataset.label ?? opp;
    btn.addEventListener("click", () => startMatch(opp, label));
  }
});
initDashboard();
initTileInspector();
initToolAndTabIcons();
spectate.init();
void initOpponentPicker();

el("again").addEventListener("click", () => {
  showLobby();
});

// F4: adaptive difficulty. Remember recent match results locally and pick a
// tier that keeps the challenge fair: 3 wins in the last 4 → step up,
// losing most of the last 4 → step down.
interface RecordEntry {
  tier: string;
  won: boolean;
}
function recordResult(tier: string, won: boolean): void {
  try {
    const raw = localStorage.getItem("crucible.record");
    const list: RecordEntry[] = raw ? (JSON.parse(raw) as RecordEntry[]) : [];
    list.push({ tier, won });
    while (list.length > 24) list.shift();
    localStorage.setItem("crucible.record", JSON.stringify(list));
  } catch {
    /* storage unavailable (private mode) — skip */
  }
}
function adaptiveTier(): string {
  try {
    const raw = localStorage.getItem("crucible.record");
    if (!raw) return "medium";
    const list: RecordEntry[] = JSON.parse(raw) as RecordEntry[];
    const recent = list.slice(-4);
    if (recent.length === 0) return "medium";
    const wins = recent.filter((r) => r.won).length;
    if (wins >= 3) return "hard";
    if (wins === 0 && recent.length >= 3) return "easy";
    return "medium";
  } catch {
    return "medium";
  }
}
const adaptiveBtn = el("adaptive-btn");
if (adaptiveBtn) {
  adaptiveBtn.addEventListener("click", () => {
    const tier = adaptiveTier();
    // One unified adaptive opponent: send the difficulty scalar to the server
    // (the trainer's `adaptive()` commander picks the matching archetype).
    const DIFF: Record<string, number> = { easy: 0.3, medium: 0.55, hard: 0.85 };
    const diff = (DIFF[tier] ?? 0.55).toFixed(2);
    startMatch(`adaptive:${diff}`, tier.toUpperCase());
  });
}

// F13: replay sharing via URL. The result screen offers a shareable link;
// opening `?replay=N` drops the player straight into the spectate view.
let lastReplayId: number | null = null;
const shareBtn = el("share-replay");
if (shareBtn) {
  shareBtn.addEventListener("click", async () => {
    if (lastReplayId == null) return;
    const url = `${location.origin}${location.pathname}?replay=${lastReplayId}`;
    try {
      await navigator.clipboard.writeText(url);
      shareBtn.textContent = "COPIED ✓";
      setTimeout(() => {
        shareBtn.textContent = "Copy Replay Link";
      }, 1800);
    } catch {
      window.prompt("Copy this replay link:", url);
    }
  });
}
const replayParam = new URLSearchParams(location.search).get("replay");
if (replayParam != null && /^\d+$/.test(replayParam)) {
  void spectate.loadReplay(Number(replayParam));
}

requestAnimationFrame(frame);
