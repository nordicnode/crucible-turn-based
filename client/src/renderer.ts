// Canvas 2D tactical renderer for Crucible:
// Integer-cell terrain, indexed pixel sprites, directional military hardware,
// blocky combat FX, and a compact tactical minimap.

import { fx } from "./fx";
import {
  drawBuildingSprite,
  drawDesertTile,
  drawForestTile,
  drawHealthBar,
  drawHillsTile,
  drawImpassableTile,
  drawMountainTile,
  drawResourceDeposit,
  drawPassableTile,
  drawRiverTile,
  drawSelectionReticle,
  drawSwampTile,
  drawUnitSprite,
  drawWaterTile,
} from "./sprites";
import { BUILDING_KINDS, BUILD_COSTS, MAP_SIZE, resourceBundleAffordable } from "./types";
import type { Entity } from "./world";
import type { World } from "./world";

export const MAP = MAP_SIZE;
export const ZOOM_MIN = 4;
export const ZOOM_MAX = 96;

/// Production building kinds that accept a rally point.
const PRODUCER_KINDS = new Set(["Barracks", "Factory", "Airfield"]);

export class Camera {
  cx = 32; // World coordinate at top-left of viewport
  cy = 32;
  zoom = 18; // Pixels per world tile
  viewportW = 800;
  viewportH = 600;
  /**
   * Initial match framing may need to show a HQ at the edge of the map even
   * when HUD panels cover the lower part of the viewport. While set, keep the
   * requested point centered and let the map end outside the canvas; the first
   * manual pan/zoom returns to normal map clamping.
   */
  private centeredTarget: [number, number] | null = null;

  screenX(wx: number): number {
    return (wx - this.cx) * this.zoom;
  }
  screenY(wy: number): number {
    return (wy - this.cy) * this.zoom;
  }
  worldX(sx: number): number {
    return sx / this.zoom + this.cx;
  }
  worldY(sy: number): number {
    return sy / this.zoom + this.cy;
  }

  setViewport(vw: number, vh: number): void {
    this.viewportW = vw;
    this.viewportH = vh;
    if (this.centeredTarget) {
      this.cx = this.centeredTarget[0] - vw / (2 * this.zoom);
      this.cy = this.centeredTarget[1] - vh / (2 * this.zoom);
      return;
    }
    this.clampToMap();
  }

  clampToMap(): void {
    if (this.viewportW <= 0 || this.viewportH <= 0) return;
    const viewW = this.viewportW / this.zoom;
    const viewH = this.viewportH / this.zoom;

    if (viewW >= MAP) {
      this.cx = (MAP - viewW) / 2;
    } else {
      this.cx = Math.max(0, Math.min(MAP - viewW, this.cx));
    }

    if (viewH >= MAP) {
      this.cy = (MAP - viewH) / 2;
    } else {
      this.cy = Math.max(0, Math.min(MAP - viewH, this.cy));
    }
  }

  focusOn(
    wx: number,
    wy: number,
    zoom: number,
    vw: number,
    vh: number,
    keepCentered = false,
  ): void {
    this.zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, zoom));
    this.viewportW = vw;
    this.viewportH = vh;
    this.centeredTarget = keepCentered ? [wx, wy] : null;
    this.cx = wx - vw / (2 * this.zoom);
    this.cy = wy - vh / (2 * this.zoom);
  }

  centerOn(wx: number, wy: number, vw: number, vh: number, zoom?: number): void {
    this.focusOn(wx, wy, zoom ?? this.zoom, vw, vh);
  }

  pan(dx: number, dy: number, vw?: number, vh?: number): void {
    this.centeredTarget = null;
    if (vw != null && vh != null) {
      this.viewportW = vw;
      this.viewportH = vh;
    }
    this.cx -= dx / this.zoom;
    this.cy -= dy / this.zoom;
    this.clampToMap();
  }

  zoomAt(sx: number, sy: number, factor: number, vw?: number, vh?: number): void {
    this.centeredTarget = null;
    if (vw != null && vh != null) {
      this.viewportW = vw;
      this.viewportH = vh;
    }
    const wx = this.worldX(sx);
    const wy = this.worldY(sy);
    this.zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, this.zoom * factor));
    this.cx = wx - sx / this.zoom;
    this.cy = wy - sy / this.zoom;
    this.clampToMap();
  }
}

export function cameraViewRect(
  c: Camera,
  w: number,
  h: number,
): { x: number; y: number; w: number; h: number } | null {
  const x0 = c.worldX(0);
  const y0 = c.worldY(0);
  const x1 = c.worldX(w);
  const y1 = c.worldY(h);
  const rx0 = Math.max(0, Math.min(MAP, x0));
  const ry0 = Math.max(0, Math.min(MAP, y0));
  const rx1 = Math.max(0, Math.min(MAP, x1));
  const ry1 = Math.max(0, Math.min(MAP, y1));
  if (rx1 <= rx0 || ry1 <= ry0) return null;
  return { x: rx0, y: ry0, w: rx1 - rx0, h: ry1 - ry0 };
}

const COLORS = {
  unexplored: "#080b0c",
  passable: "#263326",
  impassable: "#252b2d",
  ore: "#b88732",
  own: "#5f8996",
  enemy: "#a35a4e",
  selected: "#d7bb63",
};

export interface RenderOptions {
  waypoints?: Map<number, [number, number]>;
  /** Tile currently selected for the contextual inspector. */
  selectedTile?: [number, number] | null;
  placementMode?: string | null;
  placementCursor?: [number, number] | null;
  /** Tile under the mouse; when set and a single friendly unit is selected,
   *  a movement-cost path preview is drawn (U4). */
  hoverTile?: [number, number] | null;
}

/** Cosmetic animation clock (~20 Hz), independent of game turns. */
function animClock(): number {
  return Math.floor(performance.now() / 50);
}

/** Default facing: P0 faces bottom-right (+x, +y), P1 faces top-left. */
function defaultHeading(owner: number): number {
  return owner === 0 ? Math.PI / 4 : -3 * Math.PI / 4;
}

export class Renderer {
  camera = new Camera();

  /** Frame time at which each tile became visible (fog reveal fade, U5). */
  private revealStart = new Map<number, number>();
  /** Visible set from the previous frame, used to detect reveals. */
  private prevVisible = new Set<number>();

  draw(
    ctx: CanvasRenderingContext2D,
    world: World,
    selection: Set<number>,
    w: number,
    h: number,
    opts: RenderOptions = {},
  ): void {
    this.camera.setViewport(w, h);
    // The canvas is a display surface for integer-cell sprites, never a
    // smoothed illustration.
    ctx.imageSmoothingEnabled = false;

    // 1. Clear background to dark void
    ctx.fillStyle = COLORS.unexplored;
    ctx.fillRect(0, 0, w, h);

    const cam = this.camera;
    const x0 = Math.max(0, Math.floor(cam.worldX(0)));
    const y0 = Math.max(0, Math.floor(cam.worldY(0)));
    const x1 = Math.min(MAP - 1, Math.ceil(cam.worldX(w)));
    const y1 = Math.min(MAP - 1, Math.ceil(cam.worldY(h)));

    // 2. Terrain + Fog of War
    for (let ty = y0; ty <= y1; ty++) {
      for (let tx = x0; tx <= x1; tx++) {
        const idx = ty * MAP + tx;
        const isVis = world.visible.has(idx);
        const isExp = world.explored.has(idx);
        if (!isVis && !isExp) continue;

        const px = cam.screenX(tx);
        const py = cam.screenY(ty);
        const size = cam.zoom + 0.5;
        const terrain = world.terrain.length > 0 ? world.terrain[idx] : undefined;
        const isPassable = world.passable[idx] ?? true;

        // U5: newly visible tiles fade in over ~300 ms instead of popping,
        // so the fog of war reads as a smooth reveal rather than a blink.
        let revealAlpha = 1;
        if (isVis && !this.prevVisible.has(idx)) {
          this.revealStart.set(idx, performance.now());
        }
        if (isVis) {
          const t0 = this.revealStart.get(idx);
          if (t0 !== undefined) {
            revealAlpha = Math.min(1, (performance.now() - t0) / 300);
          }
        } else {
          // Prune fade timestamps for tiles that have left the visible set,
          // so the map doesn't accumulate an entry per tile ever revealed.
          this.revealStart.delete(idx);
        }

        // Typed terrain (biomes, rivers, lakes, mountains): passability and
        // the tile's look both come from the sim's terrain field, so what the
        // player sees is what the engine moves and defends on.
        if (revealAlpha < 1) {
          // Fading reveal: everything (base tile + topology) drawn under one
          // global alpha so the fog transition is a smooth fade, not a pop.
          ctx.save();
          ctx.globalAlpha = revealAlpha;
          this.drawTerrainTile(ctx, terrain, isPassable, tx, ty, px, py, size, false);
          drawTerrainTopology(ctx, world, tx, ty, px, py, size, terrain ?? (isPassable ? "Plains" : "Mountain"));
          ctx.restore();
        } else if (isVis) {
          this.drawTerrainTile(ctx, terrain, isPassable, tx, ty, px, py, size, false);
          drawTerrainTopology(ctx, world, tx, ty, px, py, size, terrain ?? (isPassable ? "Plains" : "Mountain"));
        } else {
          // Explored-but-not-visible: dark silhouette of the real terrain.
          this.drawTerrainTile(ctx, terrain, isPassable, tx, ty, px, py, size, true);
        }
      }
    }
    this.prevVisible = new Set(world.visible);

    // 3. Selected tile frame. The border remains visible for unexplored tiles
    // without revealing any hidden terrain or resource details.
    if (opts.selectedTile) {
      const [tx, ty] = opts.selectedTile;
      if (tx >= 0 && tx < MAP && ty >= 0 && ty < MAP) {
        const px = cam.screenX(tx);
        const py = cam.screenY(ty);
        ctx.save();
        ctx.fillStyle = "rgba(255, 226, 122, 0.08)";
        ctx.fillRect(px, py, cam.zoom, cam.zoom);
        ctx.strokeStyle = "rgba(255, 226, 122, 0.95)";
        ctx.lineWidth = 2;
        ctx.strokeRect(Math.floor(px) + 1, Math.floor(py) + 1, Math.floor(cam.zoom) - 2, Math.floor(cam.zoom) - 2);
        ctx.restore();
      }
    }

    // 4. Infinite deposits: the generic stream is always authoritative.
    for (const t of world.resourceTiles.values()) {
      const px = cam.screenX(t.x);
      const py = cam.screenY(t.y);
      const size = cam.zoom;
      if (px > w || py > h || px + size < 0 || py + size < 0) continue;
      if (t.infinite !== true && t.amount <= 0) continue;
      drawResourceDeposit(ctx, t.resource, px, py, size, t.amount, animClock(), t.richness);
    }

    // 5. Ground Layer FX: Scorch craters, tracks, wreckage
    fx.drawGroundLayer(ctx, cam, w, h);

    // 6. Waypoint destination lines (units only — rally points are gone)
    if (opts.waypoints && selection.size > 0) {
      this.drawUnitWaypoints(ctx, world, selection, opts.waypoints);
    }

    // 7b. Movement path preview (U4): show the A* route a selected unit
    // would take to the hovered tile, with a cost readout.
    if (opts.hoverTile && selection.size === 1) {
      this.drawPathPreview(ctx, world, selection, opts.hoverTile);
    }

    // 7. Entities: Buildings first, then units
    const drawList = [...world.entities.values()].sort((a, b) => {
      const aUnit = isUnit(a);
      const bUnit = isUnit(b);
      if (aUnit !== bUnit) return aUnit ? 1 : -1;
      return a.id - b.id;
    });

    for (const e of drawList) {
      this.drawEntity(ctx, world, e, selection, w, h);
    }

    // 7c. Stacking badges (U12): when several units share a tile, the top
    // entity gets a small ×N counter so the player knows the tile holds more
    // than one unit.
    this.drawStackingBadges(ctx, drawList, w, h);

    // 7d. Production rally points: a dashed line + marker from each own
    // producer to where its newly-trained units will march.
    this.drawRallyPoints(ctx, world, w, h);

    // 8. Air Layer FX: Projectiles, lasers, explosions, particles
    fx.drawAirLayer(ctx, cam, w, h);

    // 9. Building placement ghost
    if (opts.placementMode && opts.placementCursor) {
      this.drawPlacementGhost(ctx, opts.placementMode, opts.placementCursor, world);
    }
  }

  /** Draw one terrain tile at `px,py` (shared by the fog-reveal path). */
  private drawTerrainTile(
    ctx: CanvasRenderingContext2D,
    terrain: string | undefined,
    isPassable: boolean,
    tx: number,
    ty: number,
    px: number,
    py: number,
    size: number,
    silhouette: boolean,
  ): void {
    if (terrain === "Forest") drawForestTile(ctx, tx, ty, px, py, size, silhouette);
    else if (terrain === "Hills") drawHillsTile(ctx, tx, ty, px, py, size, silhouette);
    else if (terrain === "Desert") drawDesertTile(ctx, tx, ty, px, py, size, silhouette);
    else if (terrain === "Swamp") drawSwampTile(ctx, tx, ty, px, py, size, silhouette);
    else if (terrain === "Water") drawWaterTile(ctx, tx, ty, px, py, size, silhouette);
    else if (terrain === "River") drawRiverTile(ctx, tx, ty, px, py, size, silhouette, animClock());
    else if (terrain === "Mountain") drawMountainTile(ctx, tx, ty, px, py, size, silhouette);
    else if (isPassable) drawPassableTile(ctx, tx, ty, px, py, size, silhouette);
    else drawImpassableTile(ctx, tx, ty, px, py, size, silhouette);
  }

  /** Draw ×N counters on tiles that hold more than one unit (U12). */
  private drawStackingBadges(
    ctx: CanvasRenderingContext2D,
    drawList: Entity[],
    w: number,
    h: number,
  ): void {
    const cam = this.camera;
    const perTile = new Map<number, Entity[]>();
    for (const e of drawList) {
      if (!isUnit(e)) continue;
      const key = Math.floor(e.x) * MAP + Math.floor(e.y);
      const list = perTile.get(key);
      if (list) list.push(e);
      else perTile.set(key, [e]);
    }
    const z = cam.zoom;
    for (const [key, list] of perTile) {
      if (list.length < 2) continue;
      // Topmost = last drawn = the one with the highest id.
      const top = list[list.length - 1];
      const px = cam.screenX(top.x);
      const py = cam.screenY(top.y);
      if (px < -z * 2 || py < -z * 2 || px > w + z * 2 || py > h + z * 2) continue;
      const label = `×${list.length}`;
      ctx.save();
      ctx.font = `bold ${Math.max(8, Math.floor(z * 0.32))}px monospace`;
      const tw = ctx.measureText(label).width;
      const bx = px + z / 2 + 2;
      const by = py - 2;
      ctx.fillStyle = "rgba(8, 10, 12, 0.85)";
      ctx.fillRect(bx, by - 10, tw + 6, 13);
      ctx.strokeStyle = "rgba(255, 226, 122, 0.8)";
      ctx.lineWidth = 1;
      ctx.strokeRect(bx, by - 10, tw + 6, 13);
      ctx.fillStyle = "#ffe27a";
      ctx.fillText(label, bx + 3, by);
      ctx.restore();
      void key;
    }
  }

  /** Draw production rally points: a dashed line from each own producer to
   *  its rally tile, with a marker at the destination, so the player sees
   *  where newly-trained units will march. */
  private drawRallyPoints(
    ctx: CanvasRenderingContext2D,
    world: World,
    w: number,
    h: number,
  ): void {
    const cam = this.camera;
    const z = cam.zoom;
    for (const b of world.entities.values()) {
      if (b.owner !== 0 || !PRODUCER_KINDS.has(b.kind)) continue;
      const rally = b.rally;
      if (!rally) continue;
      const px = cam.screenX(b.x);
      const py = cam.screenY(b.y);
      const rx = cam.screenX(rally[0] + 0.5);
      const ry = cam.screenY(rally[1] + 0.5);
      // Cull both endpoints off-screen to skip a wasted line draw.
      if (px < -z * 4 || py < -z * 4 || px > w + z * 4 || py > h + z * 4) continue;
      ctx.save();
      ctx.strokeStyle = "rgba(231, 202, 138, 0.55)";
      ctx.lineWidth = 1.5;
      ctx.setLineDash([4, 4]);
      ctx.beginPath();
      ctx.moveTo(px, py);
      ctx.lineTo(rx, ry);
      ctx.stroke();
      ctx.setLineDash([]);
      const r = Math.max(3, Math.floor(z * 0.18));
      ctx.fillStyle = "rgba(231, 202, 138, 0.92)";
      ctx.beginPath();
      ctx.arc(rx, ry, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.strokeStyle = "rgba(8, 10, 12, 0.85)";
      ctx.lineWidth = 1;
      ctx.stroke();
      ctx.restore();
    }
  }

  private drawEntity(
    ctx: CanvasRenderingContext2D,
    world: World,
    e: Entity,
    selection: Set<number>,
    w: number,
    h: number,
  ): void {
    const cam = this.camera;
    const px = cam.screenX(e.x);
    const py = cam.screenY(e.y);
    const z = cam.zoom;
    if (px < -z * 2 || py < -z * 2 || px > w + z * 2 || py > h + z * 2) return;

    let isStale = false;
    let alpha = 1;
    if (e.owner === 1 && e.stale != null) {
      isStale = true;
      // Fade ghosts out over the 6-turn fog memory window.
      const age = Math.max(0, e.stale);
      alpha = Math.max(0.25, 1 - age / 6);
    }

    ctx.save();
    ctx.globalAlpha = alpha;

    const isSelected = e.owner === 0 && selection.has(e.id);

    // Civ-style single-tile baseplate: a faint inset square marking the exact
    // tile this entity occupies. Every entity owns exactly one tile.
    ctx.strokeStyle = e.owner === 0 ? "rgba(143, 193, 181, 0.30)" : "rgba(201, 141, 106, 0.30)";
    ctx.lineWidth = 1;
    const pad = 4;
    ctx.strokeRect(
      Math.floor(px - z / 2) + pad,
      Math.floor(py - z / 2) + pad,
      z - 2 * pad,
      z - 2 * pad,
    );

    const heading = e.kind === "Turret" || e.kind === "TeslaCoil"
      ? this.turretHeading(world, e)
      : defaultHeading(e.owner);
    const firingAge = fx.getFiringAge(e.id, animClock());

    if (isUnit(e)) {
      fx.recordVehicleMovement(e.id, e.kind, e.x, e.y, heading);
      drawUnitSprite(
        ctx,
        e.kind,
        px,
        py,
        z,
        e.owner,
        heading,
        animClock(),
        isStale,
        firingAge,
        e.moved === true,
      );
    } else {
      drawBuildingSprite(
        ctx,
        e.kind,
        px,
        py,
        z,
        e.owner,
        heading,
        animClock(),
        isStale,
        e.constructionProgress ?? 0,
        e.constructionTime ?? 0,
        firingAge,
      );
    }

    // Just-taken hit: a quick white/red flash + border so incoming fire reads
    // clearly on the victim even when the projectile is off-screen.
    const hitAge = fx.getHitAge(e.id, animClock());
    if (hitAge >= 0 && hitAge <= 4) {
      const hitFrac = 1 - hitAge / 4; // 1 -> 0
      const hitA = hitFrac * 0.5;
      const half = z * 0.95;
      ctx.fillStyle = `rgba(255, 236, 236, ${hitA})`;
      ctx.fillRect(px - half / 2, py - half / 2, half, half);
      ctx.strokeStyle = `rgba(248, 113, 113, ${Math.min(1, hitA + 0.35)})`;
      ctx.lineWidth = Math.max(1.5, z * 0.07);
      ctx.strokeRect(px - half / 2, py - half / 2, half, half);
    }

    if (isSelected) {
      const reticleSize = isUnit(e) ? z * 0.9 : z * 1.15;
      drawSelectionReticle(ctx, px, py, reticleSize, animClock());
    }

    if (e.owner === 0 && e.maxHp > 0) {
      const barSize = isUnit(e) ? z * 0.8 : z * 1.1;
      drawHealthBar(ctx, px, py, barSize, e.hp, e.maxHp);
    }

    ctx.restore();
  }

  /** Aim a turret/Tesla coil at the nearest enemy it can see (cosmetic only —
   *  the sim resolves turret fire server-side). */
  private turretHeading(world: World, e: Entity): number {
    let best: Entity | null = null;
    let bestD2 = Infinity;
    for (const other of world.enemyEntities) {
      if (other.hp === 0 && other.maxHp === 0) continue;
      const d2 = (other.x - e.x) ** 2 + (other.y - e.y) ** 2;
      if (d2 < bestD2) {
        bestD2 = d2;
        best = other;
      }
    }
    if (best) return Math.atan2(best.y - e.y, best.x - e.x);
    return defaultHeading(e.owner);
  }

  private drawUnitWaypoints(
    ctx: CanvasRenderingContext2D,
    world: World,
    selection: Set<number>,
    waypoints: Map<number, [number, number]>,
  ): void {
    const cam = this.camera;

    for (const id of selection) {
      const e = world.entities.get(id);
      if (!e || !isUnit(e)) continue;
      const wp = waypoints.get(id);
      if (!wp) continue;

      const fromX = cam.screenX(e.x);
      const fromY = cam.screenY(e.y);
      const toX = cam.screenX(wp[0] + 0.5);
      const toY = cam.screenY(wp[1] + 0.5);

      ctx.save();
      // Cyan dashed line for unit move waypoints
      ctx.strokeStyle = "rgba(6, 182, 212, 0.65)";
      ctx.lineWidth = 1.5;
      ctx.setLineDash([4, 4]);
      ctx.lineDashOffset = -(animClock() * 0.5) % 8;
      ctx.beginPath();
      ctx.moveTo(fromX, fromY);
      ctx.lineTo(toX, toY);
      ctx.stroke();

      const r = Math.max(4, Math.floor(cam.zoom * 0.25));
      ctx.strokeStyle = "#ffe27a";
      ctx.lineWidth = 1.5;
      ctx.setLineDash([]);
      ctx.strokeRect(Math.floor(toX - r), Math.floor(toY - r), r * 2, r * 2);
      ctx.fillStyle = "#06b6d4";
      ctx.fillRect(Math.floor(toX - 2), Math.floor(toY - 2), 4, 4);
      ctx.restore();
    }
  }

  /** A* over world tiles: cheapest path from `from` to `to` honoring
   *  passability, terrain move multipliers, and enemy occupancy. Returns
   *  the tile list (excluding the start) or null when unreachable. */
  static pathfind(
    world: World,
    from: [number, number],
    to: [number, number],
  ): [number, number][] | null {
    if (from[0] === to[0] && from[1] === to[1]) return [];
    const cached = Renderer.pathCache.get(
      `${world.revision}|${from[0]},${from[1]}>${to[0]},${to[1]}`,
    );
    if (cached !== undefined) return cached;
    const w = MAP;
    const h = MAP;
    const idx = (x: number, y: number) => y * w + x;
    const blocked = (x: number, y: number): boolean => {
      if (x < 0 || y < 0 || x >= w || y >= h) return true;
      const i = idx(x, y);
      const rule = world.terrainRules.get(world.terrain[i]);
      if (!rule || !rule.passable) return true;
      // Enemy-held tiles are not routable through.
      for (const e of world.entities.values()) {
        if (e.owner === 0) continue;
        if (Math.floor(e.x) === x && Math.floor(e.y) === y) return true;
      }
      return false;
    };
    const cost = (x: number, y: number): number => {
      const rule = world.terrainRules.get(world.terrain[idx(x, y)]);
      return rule ? Math.max(1, rule.moveMultiplier) : 1;
    };
    const g = new Map<number, number>();
    const came = new Map<number, number>();
    const startIdx = idx(from[0], from[1]);
    const goalIdx = idx(to[0], to[1]);
    const heuristic = (i: number): number => {
      const x = i % w;
      const y = Math.floor(i / w);
      return Math.max(Math.abs(x - to[0]), Math.abs(y - to[1]));
    };
    // Binary min-heap over (idx, f) so each pop is O(log V) instead of an
    // O(V) linear scan of an open-set map. Reachability hangs (hovering an
    // isolated island/lake) would otherwise cost ~40k * ~16k steps per frame.
    const heapIdx: number[] = [];
    const heapF: number[] = [];
    const heapPush = (i: number, f: number) => {
      heapIdx.push(i);
      heapF.push(f);
      let c = heapF.length - 1;
      while (c > 0) {
        const p = (c - 1) >> 1;
        if (heapF[p] <= heapF[c]) break;
        [heapF[p], heapF[c]] = [heapF[c], heapF[p]];
        [heapIdx[p], heapIdx[c]] = [heapIdx[c], heapIdx[p]];
        c = p;
      }
    };
    const heapPop = (): number => {
      const top = heapIdx[0];
      const lastIdx = heapIdx[heapIdx.length - 1];
      heapIdx.pop();
      heapF.pop();
      if (heapF.length > 0) {
        heapIdx[0] = lastIdx;
        let p = 0;
        for (;;) {
          const l = p * 2 + 1;
          const r = l + 1;
          let s = p;
          if (l < heapF.length && heapF[l] < heapF[s]) s = l;
          if (r < heapF.length && heapF[r] < heapF[s]) s = r;
          if (s === p) break;
          [heapF[p], heapF[s]] = [heapF[s], heapF[p]];
          [heapIdx[p], heapIdx[s]] = [heapIdx[s], heapIdx[p]];
          p = s;
        }
      }
      return top;
    };
    g.set(startIdx, 0);
    heapPush(startIdx, heuristic(startIdx));
    const closed = new Set<number>();
    let guard = 0;
    while (heapF.length > 0 && guard < 500_000) {
      guard += 1;
      const bestIdx = heapPop();
      if (bestIdx === goalIdx) break;
      if (closed.has(bestIdx)) continue;
      closed.add(bestIdx);
      const bx = bestIdx % w;
      const by = Math.floor(bestIdx / w);
      const gBest = g.get(bestIdx);
      if (gBest === undefined) continue;
      for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]] as const) {
        const nx = bx + dx;
        const ny = by + dy;
        if (blocked(nx, ny)) continue;
        const ni = idx(nx, ny);
        const step = cost(nx, ny) * (dx !== 0 && dy !== 0 ? 1.4 : 1);
        const ng = gBest + step;
        const known = g.get(ni);
        if (known !== undefined && known <= ng) continue;
        g.set(ni, ng);
        came.set(ni, bestIdx);
        heapPush(ni, ng + heuristic(ni));
      }
    }
    const result: [number, number][] | null = came.has(goalIdx)
      ? (() => {
          const path: [number, number][] = [];
          let cur = goalIdx;
          while (cur !== startIdx) {
            const prev = came.get(cur);
            if (prev === undefined) break;
            // prepend so the result is start→goal without a mutating
            // `.reverse()`
            path.unshift([cur % w, Math.floor(cur / w)]);
            cur = prev;
          }
          return path;
        })()
      : null;
    Renderer.pathCache.set(
      `${world.revision}|${from[0]},${from[1]}>${to[0]},${to[1]}`,
      result,
    );
    if (Renderer.pathCache.size > 4096) {
      // Revisions and map seeds accumulate over a session; drop the oldest
      // entries once the cache passes a modest bound.
      const keys = [...Renderer.pathCache.keys()];
      for (const k of keys.slice(0, 512)) Renderer.pathCache.delete(k);
    }
    return result;
  }
  /** Path-preview cache, invalidated by `World.revision` (bumped on every
   *  diff), so a hovered route isn't recomputed every animation frame and an
   *  unreachable hover is remembered until the world changes. Bounded. */
  private static pathCache = new Map<string, [number, number][] | null>();

  /** Draw the movement path preview for a single selected unit (U4). */
  private drawPathPreview(
    ctx: CanvasRenderingContext2D,
    world: World,
    selection: Set<number>,
    target: [number, number],
  ): void {
    const unit = [...selection]
      .map((id) => world.entities.get(id))
      .find((e) => e && e.owner === 0 && isUnit(e));
    if (!unit) return;
    const from: [number, number] = [Math.floor(unit.x), Math.floor(unit.y)];
    const path = Renderer.pathfind(world, from, target);
    if (!path) return;
    const cam = this.camera;
    ctx.save();
    ctx.strokeStyle = "rgba(255, 226, 122, 0.85)";
    ctx.lineWidth = 2;
    ctx.setLineDash([6, 5]);
    ctx.lineDashOffset = -(animClock() * 0.6) % 11;
    ctx.beginPath();
    let started = false;
    for (const [px, py] of [from, ...path]) {
      const sx = cam.screenX(px + 0.5);
      const sy = cam.screenY(py + 0.5);
      if (started) {
        ctx.lineTo(sx, sy);
      } else {
        ctx.moveTo(sx, sy);
        started = true;
      }
    }
    ctx.stroke();
    ctx.setLineDash([]);
    // Cost readout at the end of the route. Include the starting tile and
    // weight diagonal steps the same way the A* search does, so the label
    // matches the path's actual computed cost.
    const total = Renderer.pathCost(world, [from, ...path]);
    const end = path[path.length - 1] ?? from;
    const ex = cam.screenX(end[0] + 0.5);
    const ey = cam.screenY(end[1] + 0.5);
    const label = `${total} MP`;
    ctx.font = "bold 11px monospace";
    const tw = ctx.measureText(label).width;
    const bx = Math.min(cam.viewportW - tw - 6, Math.max(2, ex - tw / 2));
    const by = Math.max(12, ey - 10);
    ctx.fillStyle = "rgba(8, 10, 12, 0.82)";
    ctx.fillRect(bx - 3, by - 11, tw + 6, 14);
    ctx.fillStyle = "#ffe27a";
    ctx.fillText(label, bx, by);
    ctx.restore();
  }

  /** Movement-point cost of a path (used by the preview). Weights diagonal
   *  steps ×1.4 exactly as the A* search does, so the shown MP matches the
   *  path's computed cost instead of understating routes that step diagonally. */
  static pathCost(world: World, path: [number, number][]): number {
    let total = 0;
    let prev: [number, number] | null = null;
    for (const tile of path) {
      const rule = world.terrainRules.get(world.terrain[tile[1] * MAP + tile[0]]);
      let step = rule ? Math.max(1, rule.moveMultiplier) : 1;
      if (prev) {
        const dx = Math.abs(tile[0] - prev[0]);
        const dy = Math.abs(tile[1] - prev[1]);
        if (dx !== 0 && dy !== 0) step *= 1.4;
      }
      total += step;
      prev = tile;
    }
    return Math.round(total * 10) / 10;
  }

  private drawPlacementGhost(
    ctx: CanvasRenderingContext2D,
    btype: string,
    cursor: [number, number],
    world: World,
  ): void {
    const cam = this.camera;
    const px = cam.screenX(cursor[0] + 0.5);
    const py = cam.screenY(cursor[1] + 0.5);
    const placable = isBuildingPlacable(btype, cursor, world);

    ctx.save();
    ctx.globalAlpha = placable ? 0.75 : 0.45;
    drawBuildingSprite(ctx, btype, px, py, cam.zoom, 0, 0, animClock());

    const rx = cam.screenX(cursor[0]);
    const ry = cam.screenY(cursor[1]);

    // Placement feedback is a single-tile footprint. A warm neutral outline
    // marks a legal site; an amber-red outline and X mark a rejected site.
    const z = cam.zoom;
    ctx.globalAlpha = 0.9;
    ctx.strokeStyle = placable ? "rgba(231, 202, 138, 0.95)" : "rgba(208, 120, 104, 0.95)";
    ctx.lineWidth = 1.5;
    ctx.strokeRect(
      Math.floor(rx) + 1,
      Math.floor(ry) + 1,
      Math.max(1, Math.floor(z) - 2),
      Math.max(1, Math.floor(z) - 2),
    );
    if (!placable) {
      ctx.beginPath();
      ctx.moveTo(rx + 3, ry + 3);
      ctx.lineTo(rx + z - 3, ry + z - 3);
      ctx.moveTo(rx + z - 3, ry + 3);
      ctx.lineTo(rx + 3, ry + z - 3);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
    ctx.restore();
  }

  minimapToWorld(sx: number, sy: number, _w: number, h: number): [number, number] | null {
    const s = 3.5;
    const ox = 12;
    const oy = h - MAP * s - 12;
    if (sx >= ox && sx <= ox + MAP * s && sy >= oy && sy <= oy + MAP * s) {
      return [(sx - ox) / s, (sy - oy) / s];
    }
    return null;
  }
}

/** Render tactical Radar Surveillance panel */
export function drawRadar(
  ctx: CanvasRenderingContext2D,
  world: World,
  selection: Set<number>,
  camera: Camera,
  w: number,
  h: number,
): void {
  ctx.imageSmoothingEnabled = false;
  ctx.fillStyle = "#080b0c";
  ctx.fillRect(0, 0, w, h);

  const s = Math.min(w, h) / MAP;
  const ox = (w - MAP * s) / 2;
  const oy = (h - MAP * s) / 2;

  // 1. Terrain tiles
  for (let ty = 0; ty < MAP; ty++) {
    for (let tx = 0; tx < MAP; tx++) {
      const idx = ty * MAP + tx;
      const terrain = world.terrain[idx] ?? (world.passable[idx] ? "Plains" : "Mountain");
      if (world.visible.has(idx)) {
        ctx.fillStyle = terrainRadarColor(terrain);
      } else if (world.explored.has(idx)) {
        ctx.fillStyle = world.passable[idx] ? "#172119" : "#1b2022";
      } else {
        continue;
      }
      ctx.fillRect(ox + tx * s, oy + ty * s, Math.ceil(s), Math.ceil(s));
    }
  }

  // 2. Generic infinite deposits
  for (const t of world.resourceTiles.values()) {
    if (t.infinite !== true && t.amount <= 0) continue;
    ctx.fillStyle = resourceColor(t.resource);
    ctx.fillRect(ox + t.x * s, oy + t.y * s, Math.max(2, s), Math.max(2, s));
  }

  // 3. Entities
  for (const e of world.entities.values()) {
    const isSel = selection.has(e.id);      ctx.fillStyle = isSel ? COLORS.selected : e.owner === 0 ? COLORS.own : COLORS.enemy;
    const dotSize = isUnit(e) ? Math.max(2, s) : Math.max(3, s * 1.5);
    ctx.fillRect(ox + e.x * s - dotSize / 2, oy + e.y * s - dotSize / 2, dotSize, dotSize);
  }

  // 3b. 8×8 sector grid (the same sectors the AI's army head targets, plan
  // §8: "Minimap with sector grid") — lets players read AI target choices.
  ctx.strokeStyle = "rgba(56, 189, 248, 0.22)";
  ctx.lineWidth = 1;
  for (let i = 1; i < 8; i++) {
    const gx = ox + i * (MAP / 8) * s;
    const gy = oy + i * (MAP / 8) * s;
    ctx.beginPath();
    ctx.moveTo(gx, oy);
    ctx.lineTo(gx, oy + MAP * s);
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(ox, gy);
    ctx.lineTo(ox + MAP * s, gy);
    ctx.stroke();
  }

  // 4. Accurate Camera Viewport Box. Use the camera's own viewport (the
  // inset battlefield canvas, set on every frame) rather than the window
  // dims — the radar is sized to the battle view, not the OS window.
  const vr = cameraViewRect(camera, camera.viewportW, camera.viewportH);
  if (vr) {
    ctx.strokeStyle = "rgba(255, 226, 122, 0.9)";
    ctx.lineWidth = 1.5;
    ctx.strokeRect(ox + vr.x * s, oy + vr.y * s, Math.max(4, vr.w * s), Math.max(4, vr.h * s));
    ctx.fillStyle = "rgba(255, 226, 122, 0.12)";
    ctx.fillRect(ox + vr.x * s, oy + vr.y * s, Math.max(4, vr.w * s), Math.max(4, vr.h * s));
  }

  // 5. Radar sweep animation
  const sweepAngle = (performance.now() * 0.002) % (Math.PI * 2);
  const cx = ox + (MAP * s) / 2;
  const cy = oy + (MAP * s) / 2;
  const sweepLen = (MAP * s) * 0.7;
  ctx.strokeStyle = "rgba(6, 182, 212, 0.4)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(cx, cy);
  ctx.lineTo(cx + Math.cos(sweepAngle) * sweepLen, cy + Math.sin(sweepAngle) * sweepLen);
  ctx.stroke();

  // 6. Grid lines
  ctx.strokeStyle = "rgba(71, 85, 105, 0.35)";
  ctx.lineWidth = 1;
  ctx.strokeRect(ox, oy, MAP * s, MAP * s);
  ctx.beginPath();
  ctx.moveTo(ox, cy);
  ctx.lineTo(ox + MAP * s, cy);
  ctx.moveTo(cx, oy);
  ctx.lineTo(cx, oy + MAP * s);
  ctx.stroke();
}

/** Add small topology cues after the base tile is painted. These edges are
 * intentionally softer than a full grid: shores, ridge faces, and river
 * banks communicate connected landforms without turning the map into graph
 * paper. */
function drawTerrainTopology(
  ctx: CanvasRenderingContext2D,
  world: World,
  tx: number,
  ty: number,
  px: number,
  py: number,
  size: number,
  terrain: string,
): void {
  const neighbors = [
    { x: tx, y: ty - 1, edge: "north" },
    { x: tx + 1, y: ty, edge: "east" },
    { x: tx, y: ty + 1, edge: "south" },
    { x: tx - 1, y: ty, edge: "west" },
  ];
  const neighbor = (x: number, y: number): string | null => {
    if (x < 0 || y < 0 || x >= MAP || y >= MAP) return null;
    const index = y * MAP + x;
    if (!world.visible.has(index)) return null;
    return world.terrain[index] ?? (world.passable[index] ? "Plains" : "Mountain");
  };

  // Edge styles per terrain pair: organic sandy beaches & surf where water meets land,
  // seamless water/river junctions, rugged mountain crag outlines, and soft forest fringes.
  const w = Math.max(1, Math.min(3, size * 0.08));
  const isWaterBody = (k: string) => k === "Water" || k === "River";

  const edgeStyle = (a: string, b: string): { stroke: string; foam?: string } | null => {
    // Water meeting water: seamless flow, no dividing boundary
    if (isWaterBody(a) && isWaterBody(b)) return null;

    // Water/River meeting land: golden beach and white surf
    if (isWaterBody(a)) {
      return {
        stroke: "rgba(218, 185, 110, 0.88)", // golden beach shoreline
        foam: "rgba(220, 245, 255, 0.75)",   // foaming surf crest
      };
    }
    if (isWaterBody(b)) {
      return {
        stroke: "rgba(180, 150, 90, 0.50)", // wet coastal soil
      };
    }

    if (a === "Mountain") return { stroke: "rgba(42, 50, 60, 0.88)" }; // alpine rock precipice
    if (b === "Mountain") return { stroke: "rgba(120, 136, 152, 0.40)" }; // talus scree base
    if (a === "Forest") return { stroke: "rgba(78, 160, 86, 0.60)" };  // leafy canopy fringe
    if (a === "Swamp") return { stroke: "rgba(34, 52, 24, 0.65)" };   // dark peat wetland boundary
    if (a === "Hills" && b !== "Hills") return { stroke: "rgba(122, 105, 68, 0.55)" }; // terrace contour step
    return null;
  };

  ctx.save();
  for (const n of neighbors) {
    const other = neighbor(n.x, n.y);
    if (other == null || other === terrain) continue;
    const style = edgeStyle(terrain, other);
    if (!style) continue;

    ctx.lineWidth = w;
    ctx.strokeStyle = style.stroke;
    ctx.beginPath();
    if (n.edge === "north") {
      ctx.moveTo(px, py + w / 2);
      ctx.lineTo(px + size, py + w / 2);
    } else if (n.edge === "east") {
      ctx.moveTo(px + size - w / 2, py);
      ctx.lineTo(px + size - w / 2, py + size);
    } else if (n.edge === "south") {
      ctx.moveTo(px, py + size - w / 2);
      ctx.lineTo(px + size, py + size - w / 2);
    } else {
      ctx.moveTo(px + w / 2, py);
      ctx.lineTo(px + w / 2, py + size);
    }
    ctx.stroke();

    // Foaming surf line on water edges
    if (style.foam && size >= 10) {
      ctx.lineWidth = Math.max(1, w * 0.4);
      ctx.strokeStyle = style.foam;
      ctx.beginPath();
      const foamOffset = w + 1;
      if (n.edge === "north") {
        ctx.moveTo(px + 2, py + foamOffset);
        ctx.lineTo(px + size - 2, py + foamOffset);
      } else if (n.edge === "east") {
        ctx.moveTo(px + size - foamOffset, py + 2);
        ctx.lineTo(px + size - foamOffset, py + size - 2);
      } else if (n.edge === "south") {
        ctx.moveTo(px + 2, py + size - foamOffset);
        ctx.lineTo(px + size - 2, py + size - foamOffset);
      } else {
        ctx.moveTo(px + foamOffset, py + 2);
        ctx.lineTo(px + foamOffset, py + size - 2);
      }
      ctx.stroke();
    }
  }
  ctx.restore();
}

function terrainRadarColor(terrain: string): string {
  switch (terrain) {
    case "Forest": return "#35703a";
    case "Hills": return "#8a7a4f";
    case "Desert": return "#d6b46c";
    case "Swamp": return "#4a6b3a";
    case "Water": return "#2a6f9c";
    case "River": return "#3b93bd";
    case "Mountain": return "#828d99";
    default: return "#64884a";
  }
}

function resourceColor(resource: string): string {
  switch (resource) {
    case "Steel": return "#cbd5e1";
    case "Coal": return "#94a3b8";
    case "Crystal": return "#22d3ee";
    default: return COLORS.ore;
  }
}

function isUnit(e: Entity): boolean {
  return [
    "Infantry",
    "Scout",
    "RocketTrooper",
    "Tank",
    "Artillery",
    "MammothTank",
    "Gunship",
    "Interceptor",
    "SamLauncher",
  ].includes(e.kind);
}

export function isBuildingPlacable(
  btype: string,
  tile: [number, number],
  world: World,
): boolean {
  const [tx, ty] = tile;
  if (tx < 0 || tx >= MAP || ty < 0 || ty >= MAP) return false;
  const idx = ty * MAP + tx;
  if (world.passable.length > 0 && !world.passable[idx]) return false;

  // Buildings and friendly units both reserve their current tile. The sim
  // applies the same rule; treating a unit as an open construction site makes
  // the placement ghost lie during a crowded turn.
  for (const e of world.entities.values()) {
    if (
      Math.floor(e.x) === tx
      && Math.floor(e.y) === ty
      && (BUILDING_KINDS.has(e.kind) || isUnit(e))
    ) {
      return false;
    }
  }

  const oreTile = world.oreTiles.get(`${tx},${ty}`);
  const cryTile = world.crystalTiles.get(`${tx},${ty}`);
  const resource = world.resourceTiles.get(`${tx},${ty}`)
    ?? (oreTile
      ? { resource: "Ore" as const, amount: oreTile.amount, infinite: true }
      : cryTile
        ? { resource: "Crystal" as const, amount: cryTile.amount, infinite: true }
        : null);
  const hasResource = resource != null && (resource.infinite === true || resource.amount > 0);

  // A generic refinery occupies the live deposit tile. Every resource type is
  // valid; the server remains authoritative if the deposit was just claimed
  // by another command.
  if (btype === "Refinery" || btype === "CrystalRefinery") {
    if (!hasResource) return false;
  } else if (hasResource) {
    return false;
  }

  // Non-refinery buildings on a normal tile must sit near an own building
  // (within 5 tiles Euclidean).
  if (btype !== "Refinery" && btype !== "CrystalRefinery") {
    const PLACE_RADIUS_SQ = 25; // 5^2
    let nearOwn = false;
    for (const b of world.ownBuildings) {
      const bx = Math.floor(b.x);
      const by = Math.floor(b.y);
      const d2 = (bx - tx) * (bx - tx) + (by - ty) * (by - ty);
      if (d2 <= PLACE_RADIUS_SQ) {
        nearOwn = true;
        break;
      }
    }
    if (!nearOwn && world.ownBuildings.length > 0) {
      return false;
    }
  }

  // Tech tree gates: TechLab & Airfield need a Factory; Radar, TeslaCoil and
  // the AATurret are the second tier and need the TechLab itself.
  if (btype === "TechLab" || btype === "Airfield") {
    const hasFactory = world.ownBuildings.some((b) => b.kind === "Factory");
    if (!hasFactory) return false;
  }
  if (btype === "Radar" || btype === "TeslaCoil" || btype === "AATurret") {
    const hasLab = world.ownBuildings.some((b) => b.kind === "TechLab");
    if (!hasLab) return false;
  }

  // Check the complete four-resource price, not just the legacy ore scalar.
  const cost = BUILD_COSTS[btype] ?? { ore: 0, steel: 0, coal: 0, crystal: 0 };
  if (!resourceBundleAffordable(world.resources, cost)) return false;

  return true;
}
