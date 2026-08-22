// Command & Conquer Elite Retro Pixel-Art Custom Sprite Engine for Crucible.
// Features 2.5D isometric elevation, volumetric crystal fields, industrial details,
// multi-tier lighting, hazard striping, and dynamic mechanical animations.

export interface TeamPalette {
  primary: string;
  primaryLight: string;
  primaryDark: string;
  accent: string;
  glow: string;
}

export const TEAM_BLUE: TeamPalette = {
  primary: "#1d4ed8",
  primaryLight: "#3b82f6",
  primaryDark: "#1e3a8a",
  accent: "#93c5fd",
  glow: "rgba(59, 130, 246, 0.6)",
};

export const TEAM_RED: TeamPalette = {
  primary: "#b91c1c",
  primaryLight: "#ef4444",
  primaryDark: "#7f1d1d",
  accent: "#fca5a5",
  glow: "rgba(239, 68, 68, 0.6)",
};

export const TEAM_STALE: TeamPalette = {
  primary: "#374151",
  primaryLight: "#6b7280",
  primaryDark: "#1f2937",
  accent: "#9ca3af",
  glow: "rgba(107, 114, 128, 0.3)",
};

export function getTeamPalette(owner: number, isStale: boolean = false): TeamPalette {
  if (isStale) return TEAM_STALE;
  return owner === 0 ? TEAM_BLUE : TEAM_RED;
}

/** Stable URLs for the small in-repository art pack. Canvas terrain keeps a
 * procedural fallback, while HTML HUD/inspector surfaces can use crisp SVGs
 * without waiting for an asset loader. */
export function getAssetUrl(category: "terrain" | "resources" | "ui", key: string): string {
  const normalized = key.toLowerCase() === "water" ? "lake" : key.toLowerCase();
  return `/assets/${category}/${normalized}.svg`;
}

function tileHash(tx: number, ty: number): number {
  let h = (tx * 374761393 + ty * 668265263) ^ 0x5bf03635;
  h = (h ^ (h >> 13)) * 1274126143;
  return (h ^ (h >> 16)) >>> 0;
}

/** Deterministic micro-position within a tile for per-tile detail placement. */
function detPos(h: number, i: number, span: number): number {
  return ((h >>> (i * 7)) % Math.max(1, Math.floor(span)));
}

/** A filled circle, integer-snapped, deterministic per tile. */
function pCircle(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  r: number,
  color: string,
): void {
  ctx.beginPath();
  ctx.arc(Math.floor(cx) + 0.5, Math.floor(cy) + 0.5, Math.max(0.5, r), 0, Math.PI * 2);
  ctx.fillStyle = color;
  ctx.fill();
}

/** A filled triangle (peak/ridge facet), integer-snapped. */
function pTri(
  ctx: CanvasRenderingContext2D,
  x1: number, y1: number,
  x2: number, y2: number,
  x3: number, y3: number,
  color: string,
): void {
  ctx.beginPath();
  ctx.moveTo(Math.floor(x1) + 0.5, Math.floor(y1) + 0.5);
  ctx.lineTo(Math.floor(x2) + 0.5, Math.floor(y2) + 0.5);
  ctx.lineTo(Math.floor(x3) + 0.5, Math.floor(y3) + 0.5);
  ctx.closePath();
  ctx.fillStyle = color;
  ctx.fill();
}

/** A short grass tuft: two blades with a highlight. */
function pGrassTuft(
  ctx: CanvasRenderingContext2D,
  x: number, y: number,
  w: number, h: number,
  light: string,
  dark: string,
): void {
  const bw = Math.max(1, Math.floor(w));
  const bh = Math.max(2, Math.floor(h));
  pRect(ctx, x, y, bw, bh, dark);
  pRect(ctx, x + bw + Math.max(1, bw / 2), y, bw, Math.max(2, bh - 1), light);
}

// ---------------------------------------------------------------------------
// Pixel Art Drawing Helpers (Grid-snapped, integer alignment)
// ---------------------------------------------------------------------------

function pRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, color: string): void {
  ctx.fillStyle = color;
  ctx.fillRect(Math.floor(x), Math.floor(y), Math.max(1, Math.floor(w)), Math.max(1, Math.floor(h)));
}

function pStroke(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, color: string): void {
  ctx.strokeStyle = color;
  ctx.lineWidth = 1;
  ctx.strokeRect(Math.floor(x) + 0.5, Math.floor(y) + 0.5, Math.floor(w) - 1, Math.floor(h) - 1);
}

function pDither(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, c1: string, c2: string): void {
  pRect(ctx, x, y, w, h, c1);
  ctx.fillStyle = c2;
  const ix0 = Math.floor(x);
  const iy0 = Math.floor(y);
  const ix1 = ix0 + Math.floor(w);
  const iy1 = iy0 + Math.floor(h);
  for (let py = iy0; py < iy1; py += 2) {
    for (let px = ix0 + (py % 4 === 0 ? 0 : 1); px < ix1; px += 2) {
      ctx.fillRect(px, py, 1, 1);
    }
  }
}

function pHazard(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number): void {
  pRect(ctx, x, y, w, h, "#ca8a04");
  ctx.fillStyle = "#09090b";
  const ix0 = Math.floor(x);
  const iy0 = Math.floor(y);
  const iw = Math.floor(w);
  const ih = Math.floor(h);
  for (let px = ix0; px < ix0 + iw; px += 4) {
    ctx.fillRect(px, iy0, 2, ih);
  }
}

// ---------------------------------------------------------------------------
// Terrain Sprites (Civ-style readable biomes: organic shapes, consistent
// north-west light, distinct per-tile texture)
// ---------------------------------------------------------------------------

export function drawPassableTile(
  ctx: CanvasRenderingContext2D,
  tx: number,
  ty: number,
  px: number,
  py: number,
  size: number,
  isExploredOnly: boolean,
): void {
  const h = tileHash(tx, ty);

  if (isExploredOnly) {
    pRect(ctx, px, py, size, size, "#09100c");
    pStroke(ctx, px, py, size, size, "#040705");
    return;
  }

  // Multi-tone lush grassland with rich micro-texture
  const bases = ["#4e7a36", "#56843c", "#477030", "#5c8c42"];
  pRect(ctx, px, py, size, size, bases[h % 4]);
  // Sunlit north-west corner, shadowed south-east corner for gentle elevation
  pRect(ctx, px, py, size, Math.max(1, size * 0.12), "rgba(255,255,255,0.12)");
  pRect(ctx, px, py, Math.max(1, size * 0.12), size, "rgba(255,255,255,0.08)");
  pRect(ctx, px, py + size - Math.max(1, size * 0.12), size, Math.max(1, size * 0.12), "rgba(0,0,0,0.12)");
  pRect(ctx, px + size - Math.max(1, size * 0.12), py, Math.max(1, size * 0.12), size, "rgba(0,0,0,0.08)");
  pStroke(ctx, px, py, size, size, "rgba(24,46,18,0.30)");

  if (size >= 7) {
    // Multi-blade grass tufts scattered deterministically
    const tuftCount = 2 + (h % 3);
    for (let i = 0; i < tuftCount; i++) {
      const gx = px + detPos(h, i * 2, size - 8);
      const gy = py + detPos(h, i * 2 + 3, size - 6);
      pGrassTuft(ctx, gx, gy, Math.max(1, size * 0.05), Math.max(2, size * 0.14), "#8ec460", "#385223");
    }

    // Deterministic colorful wildflowers & clover
    const flowerType = h % 4;
    const fx1 = px + detPos(h, 5, size - 4);
    const fy1 = py + detPos(h, 6, size - 4);
    const fx2 = px + detPos(h, 7, size - 4);
    const fy2 = py + detPos(h, 8, size - 4);

    if (flowerType === 0) {
      // Golden buttercups
      pRect(ctx, fx1, fy1, 2, 2, "#facc15");
      pRect(ctx, fx1, fy1 + 1, 1, 1, "#ca8a04");
      pRect(ctx, fx2, fy2, 1, 1, "#fef08a");
    } else if (flowerType === 1) {
      // Crimson field poppies
      pRect(ctx, fx1, fy1, 2, 2, "#ef4444");
      pRect(ctx, fx1 + 1, fy1, 1, 1, "#fca5a5");
      pRect(ctx, fx2, fy2, 1, 1, "#dc2626");
    } else if (flowerType === 2) {
      // Soft lavender clover blossoms
      pRect(ctx, fx1, fy1, 2, 2, "#c084fc");
      pRect(ctx, fx1, fy1, 1, 1, "#e9d5ff");
      pRect(ctx, fx2, fy2, 1, 1, "#a855f7");
    } else {
      // White meadow daisies with yellow eye
      pRect(ctx, fx1, fy1, 2, 2, "#f8fafc");
      pRect(ctx, fx1 + 1, fy1, 1, 1, "#eab308");
      pRect(ctx, fx2, fy2, 1, 1, "#f1f5f9");
    }

    // Small earth pebble
    if (size >= 12 && h % 3 === 0) {
      const rx = px + size * 0.65;
      const ry = py + size * 0.75;
      pRect(ctx, rx, ry, 2, 2, "#8b7e66");
      pRect(ctx, rx, ry + 1, 2, 1, "#544c3c");
    }
  }
}

/** Forest: volumetric layered canopies, visible tree trunks, deep shadowed floor. */
export function drawForestTile(
  ctx: CanvasRenderingContext2D,
  tx: number,
  ty: number,
  px: number,
  py: number,
  size: number,
  isExploredOnly: boolean,
): void {
  const h = tileHash(tx, ty);

  if (isExploredOnly) {
    pRect(ctx, px, py, size, size, "#07130a");
    pStroke(ctx, px, py, size, size, "#030804");
    return;
  }

  // Forest floor base: rich loamy understory
  pRect(ctx, px, py, size, size, h % 2 === 0 ? "#223518" : "#283c1c");
  pStroke(ctx, px, py, size, size, "rgba(12,30,14,0.45)");

  if (size >= 7) {
    // Deep ground shadows beneath trees
    pRect(ctx, px + size * 0.15, py + size * 0.62, size * 0.35, size * 0.15, "rgba(10, 20, 10, 0.55)");
    pRect(ctx, px + size * 0.52, py + size * 0.68, size * 0.38, size * 0.16, "rgba(10, 20, 10, 0.55)");

    // Tree trunks with root flares
    pRect(ctx, px + size * 0.28, py + size * 0.48, Math.max(2, size * 0.06), size * 0.22, "#4a3016");
    pRect(ctx, px + size * 0.26, py + size * 0.66, Math.max(3, size * 0.10), Math.max(1, size * 0.04), "#3a2510");
    pRect(ctx, px + size * 0.66, py + size * 0.52, Math.max(2, size * 0.06), size * 0.24, "#54381a");
    pRect(ctx, px + size * 0.64, py + size * 0.72, Math.max(3, size * 0.10), Math.max(1, size * 0.04), "#3a2510");

    // 5 Layered tree crowns with rich 3D shading
    const trees = [
      [0.26, 0.24, 0.24],
      [0.68, 0.20, 0.26],
      [0.44, 0.44, 0.22],
      [0.20, 0.52, 0.25],
      [0.72, 0.54, 0.28],
    ];

    for (let i = 0; i < trees.length; i++) {
      const [fx, fy, fr] = trees[(h + i) % trees.length];
      const cxp = px + size * (fx + detPos(h, i + 1, 6) / 100);
      const cyp = py + size * (fy + detPos(h, i + 4, 6) / 100);
      const r = size * fr;

      // Dark understory shadow
      pCircle(ctx, cxp, cyp + 1, r, "#153818");
      // Mid-tone foliage body
      pCircle(ctx, cxp, cyp, r * 0.9, i % 2 === 0 ? "#245c2a" : "#2f6e35");
      // Sunlit NW canopy crown highlight
      pCircle(ctx, cxp - r * 0.28, cyp - r * 0.32, r * 0.58, "#4fa456");
      // Bright specular foliage glint
      pCircle(ctx, cxp - r * 0.36, cyp - r * 0.40, r * 0.28, "#6ec876");
    }

    // Occasional forest blossom / pinecone fleck
    if (size >= 12 && h % 3 === 0) {
      pRect(ctx, px + size * 0.55, py + size * 0.32, 1, 1, "#fde047");
    }
  }
}

/** Hills: rich rolling contours with sunlit rocky ridges, shale shelves, and scree. */
export function drawHillsTile(
  ctx: CanvasRenderingContext2D,
  tx: number,
  ty: number,
  px: number,
  py: number,
  size: number,
  isExploredOnly: boolean,
): void {
  const h = tileHash(tx, ty);

  if (isExploredOnly) {
    pRect(ctx, px, py, size, size, "#120e08");
    pStroke(ctx, px, py, size, size, "#070503");
    return;
  }

  // Warm ochre earth base
  pRect(ctx, px, py, size, size, h % 2 === 0 ? "#7b6b42" : "#847448");
  pStroke(ctx, px, py, size, size, "rgba(56,46,24,0.4)");

  if (size >= 7) {
    // 1. Faceted 3D hill contour slopes
    // Lit NW slope face (warm golden light)
    pTri(ctx, px, py + size, px + size * 0.36, py + size * 0.22, px + size * 0.74, py + size, "#a8975e");
    pTri(ctx, px, py, px + size * 0.36, py + size * 0.22, px + size * 0.74, py, "#9e8d56");
    // Deep shaded SE slope hollows
    pTri(ctx, px + size * 0.36, py + size * 0.22, px + size, py + size * 0.64, px + size * 0.74, py + size, "#504222");
    pTri(ctx, px + size * 0.36, py + size * 0.22, px + size, py + size * 0.64, px + size, py, "#5b4c27");

    // 2. Crest ridge highlight line
    ctx.strokeStyle = "#c8b77c";
    ctx.lineWidth = Math.max(1, size * 0.05);
    ctx.beginPath();
    ctx.moveTo(px + size * 0.10, py + size * 0.54);
    ctx.quadraticCurveTo(px + size * 0.36, py + size * 0.28, px + size * 0.62, py + size * 0.58);
    ctx.stroke();

    // 3. Secondary contour terrace
    ctx.strokeStyle = "#493b1e";
    ctx.lineWidth = Math.max(1, size * 0.04);
    ctx.beginPath();
    ctx.moveTo(px + size * 0.50, py + size * 0.78);
    ctx.quadraticCurveTo(px + size * 0.74, py + size * 0.62, px + size * 0.94, py + size * 0.76);
    ctx.stroke();

    // 4. Rocky shale shelves and scree stones
    pRect(ctx, px + size * 0.48, py + size * 0.16, Math.max(2, size * 0.06), Math.max(2, size * 0.06), "#d6c796");
    pRect(ctx, px + size * 0.50, py + size * 0.20, Math.max(2, size * 0.06), 1, "#493b1e");
    pRect(ctx, px + size * 0.70, py + size * 0.48, Math.max(2, size * 0.06), Math.max(2, size * 0.06), "#3f3218");
    pRect(ctx, px + size * 0.20, py + size * 0.76, Math.max(2, size * 0.08), 2, "#b8a674");

    // Highland alpine grass tuft
    pGrassTuft(ctx, px + size * 0.12, py + size * 0.32, Math.max(1, size * 0.04), Math.max(2, size * 0.10), "#728c46", "#3f4e24");
  }
}

/** Desert: sweeping golden barchan dunes with sun-bleached crests and wind ripples. */
export function drawDesertTile(
  ctx: CanvasRenderingContext2D,
  tx: number,
  ty: number,
  px: number,
  py: number,
  size: number,
  isExploredOnly: boolean,
): void {
  const h = tileHash(tx, ty);

  if (isExploredOnly) {
    pRect(ctx, px, py, size, size, "#161108");
    pStroke(ctx, px, py, size, size, "#0a0703");
    return;
  }

  // Warm golden sand base
  pRect(ctx, px, py, size, size, h % 2 === 0 ? "#d6ae5c" : "#deb664");
  pRect(ctx, px, py, size, Math.max(1, size * 0.12), "rgba(255,255,255,0.18)");
  pStroke(ctx, px, py, size, size, "rgba(138,104,48,0.40)");

  if (size >= 7) {
    // 1. Sweeping wind-carved dune crest (sun-bleached gold)
    ctx.strokeStyle = "#f6e19c";
    ctx.lineWidth = Math.max(1.5, size * 0.06);
    ctx.beginPath();
    ctx.moveTo(px + size * 0.04, py + size * (0.28 + detPos(h, 0, 6) / 100));
    ctx.quadraticCurveTo(
      px + size * 0.44,
      py + size * (0.12 + detPos(h, 1, 6) / 100),
      px + size * 0.96,
      py + size * (0.30 + detPos(h, 2, 6) / 100),
    );
    ctx.stroke();

    // 2. Shaded dune slipface trough below
    ctx.strokeStyle = "#9c772e";
    ctx.lineWidth = Math.max(1, size * 0.045);
    ctx.beginPath();
    ctx.moveTo(px + size * 0.04, py + size * 0.64);
    ctx.quadraticCurveTo(px + size * 0.50, py + size * 0.78, px + size * 0.96, py + size * 0.64);
    ctx.stroke();

    // 3. Parallel wind ripple striations
    const ry = py + size * (0.42 + detPos(h, 3, 10) / 100);
    pRect(ctx, px + size * 0.12, ry, size * 0.20, 1, "#c79f4c");
    pRect(ctx, px + size * 0.40, ry + Math.max(2, size * 0.06), size * 0.18, 1, "#c79f4c");
    pRect(ctx, px + size * 0.66, ry + Math.max(1, size * 0.03), size * 0.24, 1, "#c79f4c");

    // 4. Weathered desert stone nodule with hard shadow
    const rx = px + size * 0.74;
    const stoneY = py + size * 0.26;
    pRect(ctx, rx, stoneY, Math.max(2, size * 0.08), Math.max(2, size * 0.08), "#fdf2cf");
    pRect(ctx, rx, stoneY + Math.max(2, size * 0.07), Math.max(2, size * 0.08), 1, "#846220");
    pRect(ctx, rx + Math.max(2, size * 0.08), stoneY + 1, Math.max(1, size * 0.04), Math.max(1, size * 0.06), "#523c14");
  }
}

/** Swamp: murky peat wetland with iridescent brackish pools, water lilies, and cattails. */
export function drawSwampTile(
  ctx: CanvasRenderingContext2D,
  tx: number,
  ty: number,
  px: number,
  py: number,
  size: number,
  isExploredOnly: boolean,
): void {
  const h = tileHash(tx, ty);

  if (isExploredOnly) {
    pRect(ctx, px, py, size, size, "#081008");
    pStroke(ctx, px, py, size, size, "#040804");
    return;
  }

  // Dark mossy peat base
  pRect(ctx, px, py, size, size, h % 2 === 0 ? "#3b4f2c" : "#435a32");
  pStroke(ctx, px, py, size, size, "rgba(22,40,18,0.45)");

  if (size >= 7) {
    // 1. Iridescent brackish water pools
    // Pool 1 (North-West)
    pCircle(ctx, px + size * 0.32, py + size * 0.32, size * 0.18, "#1e4e46");
    pCircle(ctx, px + size * 0.34, py + size * 0.30, size * 0.10, "#2f786d");
    pCircle(ctx, px + size * 0.36, py + size * 0.28, size * 0.04, "#52b8a6");

    // Pool 2 (South-East)
    pCircle(ctx, px + size * 0.68, py + size * 0.64, size * 0.22, "#1a4640");
    pCircle(ctx, px + size * 0.70, py + size * 0.60, size * 0.12, "#2b6e64");
    pCircle(ctx, px + size * 0.72, py + size * 0.58, size * 0.05, "#48a898");

    // 2. Floating water lily pads
    pCircle(ctx, px + size * 0.28, py + size * 0.34, Math.max(1.5, size * 0.05), "#4f7832");
    pCircle(ctx, px + size * 0.74, py + size * 0.66, Math.max(1.5, size * 0.05), "#4f7832");
    pRect(ctx, px + size * 0.28, py + size * 0.33, 1, 1, "#f472b6"); // pink lotus flower

    // 3. Dense reed and cattail cluster
    const rx = px + size * (0.14 + detPos(h, 0, 10) / 100);
    const ry = py + size * 0.52;
    const rw = Math.max(1, size * 0.045);

    // Stalks
    pRect(ctx, rx, ry, rw, size * 0.34, "#88903c");
    pRect(ctx, rx + rw * 3, ry - size * 0.08, rw, size * 0.42, "#9ea644");
    pRect(ctx, rx + rw * 6, ry + size * 0.04, rw, size * 0.30, "#767c32");

    // Fuzzy dark brown cattail heads
    pRect(ctx, rx - 0.5, ry + 1, rw + 1, size * 0.12, "#523e16");
    pRect(ctx, rx + rw * 3 - 0.5, ry - size * 0.07, rw + 1, size * 0.14, "#59421b");
    pRect(ctx, rx + rw * 6 - 0.5, ry + size * 0.05, rw + 1, size * 0.11, "#483612");
    // Fuzzy head tips
    pRect(ctx, rx, ry + 1, rw, 1, "#7a5d28");
    pRect(ctx, rx + rw * 3, ry - size * 0.07, rw, 1, "#7a5d28");

    // Wet mud patch
    pRect(ctx, px + size * 0.50, py + size * 0.16, size * 0.26, Math.max(1, size * 0.05), "#28381e");
  }
}

/** Water: deep rich ocean/lake water filling the FULL tile with dynamic waves and sun glints. */
export function drawWaterTile(
  ctx: CanvasRenderingContext2D,
  tx: number,
  ty: number,
  px: number,
  py: number,
  size: number,
  isExploredOnly: boolean,
  tick: number = 0,
): void {
  const h = tileHash(tx, ty);

  if (isExploredOnly) {
    pRect(ctx, px, py, size, size, "#06121b");
    pStroke(ctx, px, py, size, size, "#03090e");
    return;
  }

  // Full-tile water base: rich layered ocean/lake blue depth
  pRect(ctx, px, py, size, size, h % 2 === 0 ? "#205d8a" : "#246494");
  // Deep water trench / subsurface depth at lower half
  pRect(ctx, px, py + size * 0.50, size, size * 0.50, "#164468");
  pRect(ctx, px, py + size * 0.78, size, size * 0.22, "#103350");
  // Sunlit surface bevel at top/left
  pRect(ctx, px, py, size, Math.max(1, size * 0.08), "rgba(142, 226, 255, 0.22)");
  pStroke(ctx, px, py, size, size, "rgba(12,38,60,0.40)");

  if (size >= 7) {
    // 1. Organic wave crests
    ctx.strokeStyle = "rgba(110, 196, 240, 0.85)";
    ctx.lineWidth = Math.max(1, size * 0.045);
    ctx.beginPath();
    ctx.arc(px + size * 0.28, py + size * 0.32, size * 0.18, Math.PI * 0.15, Math.PI * 0.85);
    ctx.stroke();

    ctx.strokeStyle = "rgba(70, 150, 198, 0.75)";
    ctx.beginPath();
    ctx.arc(px + size * 0.72, py + size * 0.56, size * 0.22, Math.PI * 1.15, Math.PI * 1.85);
    ctx.stroke();

    // 2. Animated horizontal drift ripples
    const drift = ((Math.floor(tick * 0.8) + h) % Math.max(4, Math.floor(size * 0.8))) as number;
    pRect(ctx, px + size * 0.36 + (drift % 6) - 3, py + size * 0.22, size * 0.22, 1, "#5cb8e8");
    pRect(ctx, px + size * 0.20 + ((drift + 4) % 6) - 3, py + size * 0.68, size * 0.26, 1, "#4090c4");

    // 3. Specular sunlight sparkle glints
    const glintPhase = Math.sin(tick * 0.18 + h) > 0.1;
    if (glintPhase) {
      pRect(ctx, px + size * 0.46, py + size * 0.38, 2, 2, "#ffffff");
      pRect(ctx, px + size * 0.45, py + size * 0.39, 4, 1, "#b8edff");
      pRect(ctx, px + size * 0.18, py + size * 0.58, 2, 2, "#dcf5ff");
    } else {
      pRect(ctx, px + size * 0.68, py + size * 0.28, 2, 2, "#ffffff");
      pRect(ctx, px + size * 0.32, py + size * 0.76, 2, 2, "#dcf5ff");
    }
  }
}

/** River: dynamic flowing river filling the FULL tile with rushing currents and white-water drift. */
export function drawRiverTile(
  ctx: CanvasRenderingContext2D,
  tx: number,
  ty: number,
  px: number,
  py: number,
  size: number,
  isExploredOnly: boolean,
  tick: number = 0,
): void {
  const h = tileHash(tx, ty);

  if (isExploredOnly) {
    pRect(ctx, px, py, size, size, "#06151f");
    pStroke(ctx, px, py, size, size, "#030a0f");
    return;
  }

  // Full-tile river water: rushing vibrant cerulean water
  pRect(ctx, px, py, size, size, h % 2 === 0 ? "#25749c" : "#2b7ea6");
  // Deep center current channel
  pRect(ctx, px, py + size * 0.22, size, size * 0.56, "#1d6286");
  // Fast flow mid-stream
  pRect(ctx, px, py + size * 0.34, size, size * 0.32, "#2f8dbd");
  // Sunlit surface bevel
  pRect(ctx, px, py, size, Math.max(1, size * 0.08), "rgba(164, 235, 255, 0.28)");
  pStroke(ctx, px, py, size, size, "rgba(14,48,70,0.40)");

  if (size >= 7) {
    // 1. Dynamic white-water current flow ribbons drifting with tick
    const flow1 = ((Math.floor(tick * 1.5) + (h % 17)) % Math.max(4, Math.floor(size))) as number;
    const flow2 = ((Math.floor(tick * 1.2) + ((h >> 4) % 23)) % Math.max(4, Math.floor(size))) as number;

    // Fast center stream ribbon
    pRect(ctx, px + flow1, py + size * 0.36, Math.max(4, size * 0.30), Math.max(1, size * 0.05), "#dcf5ff");
    pRect(ctx, px + ((flow1 + size * 0.45) % size), py + size * 0.44, Math.max(3, size * 0.22), Math.max(1, size * 0.04), "#a4e5ff");
    // Secondary flow stream
    pRect(ctx, px + flow2, py + size * 0.58, Math.max(3, size * 0.24), Math.max(1, size * 0.04), "#6acbf7");
    pRect(ctx, px + ((flow2 + size * 0.55) % size), py + size * 0.24, Math.max(3, size * 0.20), Math.max(1, size * 0.04), "#50bceb");

    // 2. Submerged riverbed stones peeking through shallow edges
    const stoneX = px + size * 0.16;
    const stoneY = py + size * 0.72;
    pRect(ctx, stoneX, stoneY, Math.max(2, size * 0.08), Math.max(2, size * 0.06), "#2a4232");
    pRect(ctx, stoneX, stoneY, Math.max(2, size * 0.08), 1, "#4e6e58");

    // 3. Specular sunlight sparkle
    if (Math.sin(tick * 0.25 + h * 0.5) > 0.2) {
      pRect(ctx, px + size * 0.62, py + size * 0.30, 2, 2, "#ffffff");
      pRect(ctx, px + size * 0.28, py + size * 0.50, 2, 2, "#ffffff");
    }
  }
}

/** Mountain: dramatic 3D alpine peaks with sharp ridges, lit rock faces, snow caps, and scree. */
export function drawMountainTile(
  ctx: CanvasRenderingContext2D,
  tx: number,
  ty: number,
  px: number,
  py: number,
  size: number,
  isExploredOnly: boolean,
): void {
  const h = tileHash(tx, ty);

  if (isExploredOnly) {
    pRect(ctx, px, py, size, size, "#080d12");
    pStroke(ctx, px, py, size, size, "#040608");
    return;
  }

  // Base mountain bedrock
  pRect(ctx, px, py, size, size, h % 2 === 0 ? "#525c68" : "#5a6673");
  pStroke(ctx, px, py, size, size, "rgba(28,36,44,0.5)");

  if (size >= 7) {
    // 1. Primary Peak: Apex coordinates
    const apexX = px + size * (0.48 + detPos(h, 0, 8) / 100);
    const apexY = py + size * 0.08;

    // Lit NW granite face (bright mountain light)
    pTri(ctx, px + size * 0.12, py + size * 0.94, apexX, apexY, px + size * 0.52, py + size * 0.94, "#7e8c9c");
    // Deep SE precipice shadow face
    pTri(ctx, px + size * 0.52, py + size * 0.94, apexX, apexY, px + size * 0.88, py + size * 0.94, "#323a44");

    // Ridge crest highlight line
    ctx.strokeStyle = "#9db0c4";
    ctx.lineWidth = Math.max(1, size * 0.04);
    ctx.beginPath();
    ctx.moveTo(apexX, apexY);
    ctx.lineTo(px + size * 0.52, py + size * 0.94);
    ctx.stroke();

    // 2. Pure snow and ice cap on primary peak
    pTri(
      ctx,
      apexX - size * 0.12,
      apexY + size * 0.20,
      apexX,
      apexY,
      apexX + size * 0.12,
      apexY + size * 0.20,
      "#edf5fb",
    );
    // Glacial shadow facet on snow
    pTri(
      ctx,
      apexX,
      apexY,
      apexX,
      apexY + size * 0.20,
      apexX + size * 0.12,
      apexY + size * 0.20,
      "#b4d4e8",
    );

    // 3. Secondary companion peak
    const secApexX = px + size * (0.84 + detPos(h, 2, 6) / 100);
    const secApexY = py + size * 0.34;
    pTri(ctx, px + size * 0.70, py + size * 0.94, secApexX, secApexY, px + size * 0.98, py + size * 0.94, "#687480");
    pTri(ctx, secApexX - size * 0.06, secApexY + size * 0.12, secApexX, secApexY, secApexX + size * 0.06, secApexY + size * 0.12, "#edf5fb");

    // 4. Shadowed base footing & talus scree field
    pRect(ctx, px, py + size * 0.88, size, size * 0.12, "#252c34");
    pRect(ctx, px + size * 0.22, py + size * 0.74, 2, 2, "#9ab0c4");
    pRect(ctx, px + size * 0.64, py + size * 0.82, 2, 2, "#36404c");
    pRect(ctx, px + size * 0.40, py + size * 0.86, 2, 2, "#8898a8");
  }
}

export function drawImpassableTile(
  ctx: CanvasRenderingContext2D,
  tx: number,
  ty: number,
  px: number,
  py: number,
  size: number,
  isExploredOnly: boolean,
): void {
  const h = tileHash(tx, ty);

  if (isExploredOnly) {
    pRect(ctx, px, py, size, size, "#0f1318");
    pStroke(ctx, px, py, size, size, "#080a0d");
    return;
  }

  // 2.5D Basaltic rock formation
  pRect(ctx, px, py, size, size, "#1c222b");

  // Top & Left 2px bright highlight bevel (light from North-West)
  pRect(ctx, px, py, size, 2, "#384556");
  pRect(ctx, px, py, 2, size, "#384556");
  pRect(ctx, px + 1, py + 1, size - 2, 1, "#4f5f75");

  // Bottom & Right 2px deep shadow bevel
  pRect(ctx, px, py + size - 2, size, 2, "#0b0e12");
  pRect(ctx, px + size - 2, py, 2, size, "#0b0e12");

  // Crag fissure crevasse
  const fv = h % 3;
  if (size >= 10) {
    if (fv === 0) {
      pRect(ctx, px + size * 0.35, py + 3, 2, size * 0.45, "#0b0e12");
      pRect(ctx, px + size * 0.45, py + size * 0.45, size * 0.35, 2, "#0b0e12");
      pRect(ctx, px + size * 0.35 - 1, py + 4, 1, size * 0.25, "#252d38");
    } else if (fv === 1) {
      pRect(ctx, px + 3, py + size * 0.4, size * 0.55, 2, "#0b0e12");
      pRect(ctx, px + size * 0.55, py + size * 0.4, 2, size * 0.4, "#0b0e12");
    } else {
      pDither(ctx, px + 3, py + 3, size - 6, size - 6, "#252d38", "#1c222b");
    }
  }
}

// ---------------------------------------------------------------------------
// Ore Deposit: Volumetric Command & Conquer Tiberite Crystal Field
// ---------------------------------------------------------------------------

export function drawOreDeposit(
  ctx: CanvasRenderingContext2D,
  px: number,
  py: number,
  size: number,
  amount: number,
  tick: number,
): void {
  const cx = px + size * 0.5;
  const cy = py + size * 0.5;
  const scale = Math.min(1.0, Math.max(0.5, amount / 1500));
  const s = size * 0.22 * scale;
  const seed = (Math.floor(px * 23 + py * 41)) % 100;

  // 1. Subtle golden ambient ground shimmer
  const glowR = Math.floor(s * 1.5);
  ctx.fillStyle = "rgba(234, 179, 8, 0.18)";
  ctx.fillRect(Math.floor(cx - glowR), Math.floor(cy - glowR * 0.6), glowR * 2, glowR * 1.2);
  ctx.fillStyle = "rgba(250, 204, 21, 0.12)";
  ctx.fillRect(Math.floor(cx - glowR * 0.5), Math.floor(cy - glowR * 0.3), glowR, glowR * 0.6);

  // 2. Compact bedrock nodule with molten gold veins
  pRect(ctx, cx - s * 0.85, cy + s * 0.05, s * 1.7, s * 0.45, "#18181b");
  pRect(ctx, cx - s * 0.7, cy, s * 1.4, s * 0.25, "#27272a");

  // Molten golden fissure veins
  pRect(ctx, cx - s * 0.5, cy + s * 0.2, s * 0.4, 1.5, "#eab308");
  pRect(ctx, cx + s * 0.1, cy + s * 0.25, s * 0.45, 1.5, "#facc15");

  // 3. Compact Faceted Crystal Mineral Nuggets: [dx, dy, w, h, lean, phase]
  const crystals = [
    { dx: -s * 0.55, dy: s * 0.1, w: Math.max(2.5, s * 0.28), h: s * 0.6, lean: -0.5, phase: 1.0 },
    { dx: s * 0.5, dy: s * 0.12, w: Math.max(2.5, s * 0.26), h: s * 0.55, lean: 0.5, phase: 2.7 },
    { dx: -s * 0.25, dy: -s * 0.05, w: Math.max(3, s * 0.34), h: s * 0.85, lean: -0.3, phase: 0.4 },
    { dx: s * 0.25, dy: 0, w: Math.max(3, s * 0.35), h: s * 0.8, lean: 0.3, phase: 3.1 },
    { dx: 0, dy: -s * 0.1, w: Math.max(3.5, s * 0.4), h: s * 0.95, lean: 0, phase: 4.5 },
    { dx: s * 0.12, dy: s * 0.18, w: Math.max(2.5, s * 0.24), h: s * 0.45, lean: 0.5, phase: 5.2 },
  ];

  for (let i = 0; i < crystals.length; i++) {
    const c = crystals[i];
    const x = Math.floor(cx + c.dx);
    const y = Math.floor(cy + c.dy);
    const w = Math.floor(c.w);
    const h = Math.floor(c.h);
    const halfW = Math.floor(w / 2);

    const shimmer = Math.sin(tick * 0.25 + c.phase + seed) > 0.4;

    // Hard dark amber outline
    pRect(ctx, x - halfW - 1, y - h - 1, w + 2, h + 2, "#451a03");

    // Right dark amber shadow facet (East face)
    pRect(ctx, x, y - h, halfW, h, "#92400e");
    pRect(ctx, x + 0.5, y - h + 1, Math.max(1, halfW - 1), h - 1, "#b45309");

    // Left bright sunlit gold facet (West face)
    pRect(ctx, x - halfW, y - h, halfW, h, "#eab308");
    pRect(ctx, x - halfW + 0.5, y - h + 0.5, Math.max(1, halfW - 0.5), h - 1, "#facc15");

    // Pointed crystal tip
    pRect(ctx, x - 0.5 + c.lean, y - h - 1.5, 1.5, 1.5, "#fef08a");

    // Dynamic specular sparkle
    if (shimmer) {
      pRect(ctx, x - 1 + c.lean, y - h - 2, 2, 2, "#ffffff");
      pRect(ctx, x - 2 + c.lean, y - h - 1, 4, 1, "#ffffff");
    }
  }

  // 4. Subtle floating mineral glint
  const sporePhase = (tick * 0.1 + seed) % 10;
  if (sporePhase < 4) {
    const sporeY = cy - s * 0.5 - sporePhase * 1.5;
    const sporeX = cx + Math.sin(tick * 0.2 + seed) * (s * 0.5);
    pRect(ctx, sporeX, sporeY, 1.5, 1.5, "#fef08a");
  }
}

// ---------------------------------------------------------------------------
// Crystal Deposit: Cyan strategic crystal field (ore is gold, crystal is cyan)
// ---------------------------------------------------------------------------

export function drawCrystalDeposit(
  ctx: CanvasRenderingContext2D,
  px: number,
  py: number,
  size: number,
  amount: number,
  tick: number,
): void {
  const cx = px + size * 0.5;
  const cy = py + size * 0.5;
  const scale = Math.min(1.0, Math.max(0.5, amount / 1500));
  const s = size * 0.22 * scale;
  const seed = (Math.floor(px * 23 + py * 41)) % 100;

  // 1. Cool cyan ambient ground shimmer
  const glowR = Math.floor(s * 1.5);
  ctx.fillStyle = "rgba(8, 145, 178, 0.18)";
  ctx.fillRect(Math.floor(cx - glowR), Math.floor(cy - glowR * 0.6), glowR * 2, glowR * 1.2);
  ctx.fillStyle = "rgba(34, 211, 238, 0.12)";
  ctx.fillRect(Math.floor(cx - glowR * 0.5), Math.floor(cy - glowR * 0.3), glowR, glowR * 0.6);

  // 2. Compact dark bedrock nodule with cyan veins
  pRect(ctx, cx - s * 0.85, cy + s * 0.05, s * 1.7, s * 0.45, "#18181b");
  pRect(ctx, cx - s * 0.7, cy, s * 1.4, s * 0.25, "#27272a");
  pRect(ctx, cx - s * 0.5, cy + s * 0.2, s * 0.4, 1.5, "#0891b2");
  pRect(ctx, cx + s * 0.1, cy + s * 0.25, s * 0.45, 1.5, "#22d3ee");

  // 3. Faceted cyan crystal shards
  const crystals = [
    { dx: -s * 0.55, dy: s * 0.1, w: Math.max(2.5, s * 0.28), h: s * 0.6, lean: -0.5, phase: 1.0 },
    { dx: s * 0.5, dy: s * 0.12, w: Math.max(2.5, s * 0.26), h: s * 0.55, lean: 0.5, phase: 2.7 },
    { dx: -s * 0.25, dy: -s * 0.05, w: Math.max(3, s * 0.34), h: s * 0.85, lean: -0.3, phase: 0.4 },
    { dx: s * 0.25, dy: 0, w: Math.max(3, s * 0.35), h: s * 0.8, lean: 0.3, phase: 3.1 },
    { dx: 0, dy: -s * 0.1, w: Math.max(3.5, s * 0.4), h: s * 0.95, lean: 0, phase: 4.5 },
    { dx: s * 0.12, dy: s * 0.18, w: Math.max(2.5, s * 0.24), h: s * 0.45, lean: 0.5, phase: 5.2 },
  ];

  for (let i = 0; i < crystals.length; i++) {
    const c = crystals[i];
    const x = Math.floor(cx + c.dx);
    const y = Math.floor(cy + c.dy);
    const w = Math.floor(c.w);
    const h = Math.floor(c.h);
    const halfW = Math.floor(w / 2);
    const shimmer = Math.sin(tick * 0.25 + c.phase + seed) > 0.4;

    // Hard dark cyan outline
    pRect(ctx, x - halfW - 1, y - h - 1, w + 2, h + 2, "#083344");
    // Right darker cyan facet (East face)
    pRect(ctx, x, y - h, halfW, h, "#0e7490");
    pRect(ctx, x + 0.5, y - h + 1, Math.max(1, halfW - 1), h - 1, "#0891b2");
    // Left bright sunlit cyan facet (West face)
    pRect(ctx, x - halfW, y - h, halfW, h, "#06b6d4");
    pRect(ctx, x - halfW + 0.5, y - h + 0.5, Math.max(1, halfW - 0.5), h - 1, "#22d3ee");
    // Pointed crystal tip
    pRect(ctx, x - 0.5 + c.lean, y - h - 1.5, 1.5, 1.5, "#a5f3fc");
    if (shimmer) {
      pRect(ctx, x - 1 + c.lean, y - h - 2, 2, 2, "#ffffff");
      pRect(ctx, x - 2 + c.lean, y - h - 1, 4, 1, "#ffffff");
    }
  }

  // 4. Floating cyan glint
  const sporePhase = (tick * 0.1 + seed) % 10;
  if (sporePhase < 4) {
    const sporeY = cy - s * 0.5 - sporePhase * 1.5;
    const sporeX = cx + Math.sin(tick * 0.2 + seed) * (s * 0.5);
    pRect(ctx, sporeX, sporeY, 1.5, 1.5, "#a5f3fc");
  }
}

/** Draw any of the four inexhaustible map deposits. Ore and crystal keep
 * their established silhouettes; Steel and Coal use compact industrial nodules
 * so every resource is visually distinct on the tactical map. Richness changes
 * the presentation scale, never the existence of the field. */
export function drawResourceDeposit(
  ctx: CanvasRenderingContext2D,
  resource: string,
  px: number,
  py: number,
  size: number,
  amount: number,
  tick: number,
  richness: number = 1,
): void {
  // The amount argument is a legacy marker. Richness is the authoritative
  // visual tier for an infinite deposit, with a safe legacy fallback.
  const visualRichness = richness > 0 ? richness : amount > 0 ? 1 : 1;
  const visualAmount = Math.max(1, Math.min(3, visualRichness)) * 500;
  if (resource === "Ore") {
    drawOreDeposit(ctx, px, py, size, visualAmount, tick);
    return;
  }
  if (resource === "Crystal") {
    drawCrystalDeposit(ctx, px, py, size, visualAmount, tick);
    return;
  }

  const cx = px + size * 0.5;
  const cy = py + size * 0.56;
  const scale = 0.45 + Math.max(0, Math.min(2, richness - 1)) * 0.18;
  const s = size * 0.2 * scale;
  const steel = resource === "Steel";
  const glow = steel ? "rgba(148, 163, 184, 0.20)" : "rgba(30, 41, 59, 0.32)";
  const edge = steel ? "#334155" : "#111827";
  const light = steel ? "#cbd5e1" : "#64748b";
  const mid = steel ? "#64748b" : "#374151";
  const dark = steel ? "#1e293b" : "#030712";

  ctx.fillStyle = glow;
  ctx.fillRect(Math.floor(cx - s * 2), Math.floor(cy - s), Math.floor(s * 4), Math.floor(s * 2));
  pRect(ctx, cx - s * 1.15, cy, s * 2.3, s * 0.55, dark);
  pRect(ctx, cx - s * 0.9, cy - s * 0.12, s * 1.8, s * 0.28, edge);
  const chunks = [
    { dx: -0.62, dy: 0.02, w: 0.42, h: 0.58 },
    { dx: -0.15, dy: -0.2, w: 0.52, h: 0.82 },
    { dx: 0.38, dy: -0.02, w: 0.45, h: 0.64 },
  ];
  for (const chunk of chunks) {
    const x = Math.floor(cx + s * chunk.dx);
    const y = Math.floor(cy + s * chunk.dy);
    const w = Math.max(2, Math.floor(s * chunk.w));
    const h = Math.max(3, Math.floor(s * chunk.h));
    pRect(ctx, x - 1, y - h - 1, w + 2, h + 2, dark);
    pRect(ctx, x, y - h, w, h, mid);
    pRect(ctx, x, y - h, Math.max(1, Math.floor(w * 0.45)), h, light);
    pRect(ctx, x, y - h, w, 1, steel ? "#f1f5f9" : "#94a3b8");
  }
  pRect(ctx, cx - s * 0.95, cy + s * 0.18, s * 0.35, 1.5, steel ? "#e2e8f0" : "#94a3b8");
  pRect(ctx, cx + s * 0.3, cy + s * 0.2, s * 0.45, 1.5, light);
  if (Math.sin(tick * 0.2 + px + py) > 0.5) {
    pRect(ctx, cx + s * 0.65, cy - s * 0.7, 2, 2, steel ? "#f8fafc" : "#cbd5e1");
  }
}

// ---------------------------------------------------------------------------
// Unit Sprites (Command & Conquer Chunky Pixel-Art Military Hardware)
// ---------------------------------------------------------------------------

/**
 * Renders the Civilization-style construction progression overlay:
 * - The unbuilt portion (from top down to the horizon) is greyed out with
 *   a desaturated blueprint wash and scaffolding lattice.
 * - The built portion (from the horizon down to the bottom) is in full vivid color.
 * - A glowing golden/amber laser construction beam scans across the horizon line
 *   with animated spark particles.
 * - A turn countdown progress bar with discrete turn markers is drawn at the base.
 */
export function drawCivUnGreyOverlay(
  ctx: CanvasRenderingContext2D,
  cx: number,
  cy: number,
  w: number,
  h: number,
  progress: number,
  buildTime: number,
  tick: number = 0,
): void {
  if (buildTime <= 0) return;
  const frac = Math.max(0, Math.min(1, progress / buildTime));
  const halfW = w / 2;
  const halfH = h / 2;

  if (frac < 1) {
    const horizonY = cy + halfH - h * frac;

    // 1. Greyed-out / Blueprint wash over the unbuilt region
    ctx.save();
    ctx.fillStyle = "rgba(18, 24, 34, 0.76)";
    const unbuiltH = horizonY - (cy - halfH);
    if (unbuiltH > 0) {
      ctx.fillRect(Math.floor(cx - halfW), Math.floor(cy - halfH), Math.floor(w), Math.floor(unbuiltH));

      // Blueprint lattice / scaffolding lines
      ctx.strokeStyle = "rgba(148, 163, 184, 0.30)";
      ctx.lineWidth = 1;
      const step = Math.max(4, Math.floor(w * 0.15));
      for (let y = cy - halfH; y < horizonY; y += step) {
        ctx.beginPath();
        ctx.moveTo(Math.floor(cx - halfW), Math.floor(y) + 0.5);
        ctx.lineTo(Math.floor(cx + halfW), Math.floor(y) + 0.5);
        ctx.stroke();
      }
      for (let x = cx - halfW; x < cx + halfW; x += step) {
        ctx.beginPath();
        ctx.moveTo(Math.floor(x) + 0.5, Math.floor(cy - halfH));
        ctx.lineTo(Math.floor(x) + 0.5, Math.floor(horizonY));
        ctx.stroke();
      }
    }

    // 2. Glowing golden construction laser line at the horizon
    pRect(ctx, cx - halfW, horizonY - 1, w, 2, "#f59e0b");
    pRect(ctx, cx - halfW + 1, horizonY, w - 2, 1, "#fef08a");

    // Dynamic laser sparks drifting along the horizon
    const sparkOffset1 = Math.floor(((tick * 2.5) % Math.max(1, w - 4)));
    const sparkOffset2 = Math.floor((((tick * 1.8) + w * 0.5) % Math.max(1, w - 4)));
    pRect(ctx, cx - halfW + sparkOffset1, horizonY - 2, 3, 3, "#ffffff");
    pRect(ctx, cx - halfW + sparkOffset2, horizonY - 1, 2, 2, "#fde047");

    ctx.restore();
  }

  // 3. Turn progress meter with discrete turn notch dividers
  const barW = Math.floor(w * 0.90);
  const barH = 5;
  const barY = Math.floor(cy + halfH + 2);
  pRect(ctx, cx - barW / 2 - 1, barY - 1, barW + 2, barH + 2, "#09090b");
  pRect(ctx, cx - barW / 2, barY, barW, barH, "#18181b");
  const fillW = Math.max(1, Math.floor(barW * frac));
  pRect(ctx, cx - barW / 2, barY, fillW, barH, "#f59e0b");
  pRect(ctx, cx - barW / 2, barY, fillW, 1, "#fde047");

  // Discrete turn notches
  if (buildTime > 1) {
    for (let step = 1; step < buildTime; step++) {
      const stepX = Math.floor(cx - barW / 2 + (barW * step) / buildTime);
      pRect(ctx, stepX, barY, 1, barH, "#09090b");
    }
  }
}

export function drawUnitSprite(
  ctx: CanvasRenderingContext2D,
  kind: string,
  px: number,
  py: number,
  zoom: number,
  owner: number,
  heading: number,
  tick: number,
  isStale: boolean = false,
  firingAge: number = -1,
  isMoving: boolean = false,
  progress: number = 0,
  buildTime: number = 0,
): void {
  const pal = getTeamPalette(owner, isStale);

  ctx.save();
  ctx.translate(Math.floor(px), Math.floor(py));
  ctx.rotate(heading);

  switch (kind) {
    case "Infantry":
      drawInfantry(ctx, zoom, pal, tick, firingAge, isMoving);
      break;
    case "Tank":
      drawTank(ctx, zoom, pal, tick, firingAge);
      break;
    case "Artillery":
      drawArtillery(ctx, zoom, pal, tick, firingAge);
      break;
    case "MammothTank":
      drawMammothTank(ctx, zoom, pal, firingAge);
      break;
    case "Gunship":
      drawGunship(ctx, zoom, pal, tick, firingAge);
      break;
    case "Interceptor":
      drawInterceptor(ctx, zoom, pal, tick, firingAge);
      break;
    case "Scout":
      drawScout(ctx, zoom, pal, tick, firingAge);
      break;
    case "RocketTrooper":
      drawRocketTrooper(ctx, zoom, pal, firingAge);
      break;
    case "SamLauncher":
      drawSamLauncher(ctx, zoom, pal, firingAge);
      break;
    default:
      pRect(ctx, -4, -4, 8, 8, pal.primary);
  }

  // Civilization-style un-greying if unit is under construction / training
  if (buildTime > 0) {
    drawCivUnGreyOverlay(ctx, 0, 0, zoom * 0.85, zoom * 0.85, progress, buildTime, tick);
  }

  ctx.restore();
}

/** Gunship: Rotary attack helicopter (+X is forward) */
function drawGunship(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
  firingAge: number = -1,
): void {
  const s = Math.max(3, Math.floor(z * 0.40));

  // Drop shadow
  pRect(ctx, -s * 0.6, -s * 0.7 + 2, s * 1.2, s * 1.4, "rgba(0, 0, 0, 0.55)");

  // Spinning top rotor disc (blurred line alternating each frame)
  const rotorFrame = Math.floor(tick * 0.9) % 2;
  pRect(ctx, -s * 0.6, rotorFrame === 0 ? -s * 0.72 : -s * 0.58, s * 1.2, 2, "#94a3b8");
  pRect(ctx, -s * 0.15, -s * 0.85, 2, s * 0.5, "#475569");
  pRect(ctx, -s * 0.2, -s * 0.9, 4, 2, "#facc15");

  // Tail boom extending rearward (-X)
  pRect(ctx, -s * 0.9, -s * 0.22, s * 0.45, s * 0.44, "#334155");
  pRect(ctx, -s * 1.0, -s * 0.35, s * 0.1, s * 0.7, "#eab308"); // tail rotor

  // Armored fuselage (+X forward)
  pRect(ctx, -s * 0.55, -s * 0.45, s * 1.0, s * 0.9, pal.primaryDark);
  pRect(ctx, -s * 0.5, -s * 0.4, s * 0.9, s * 0.8, pal.primary);

  // Cockpit canopy with tactical HUD glass
  pRect(ctx, s * 0.15, -s * 0.25, s * 0.3, s * 0.5, "#15803d");
  pRect(ctx, s * 0.2, -s * 0.2, s * 0.2, s * 0.4, "#22c55e");
  pRect(ctx, s * 0.24, -s * 0.14, s * 0.1, s * 0.28, "#86efac");

  // Chin-mounted rotary minigun
  pRect(ctx, s * 0.2, s * 0.3, s * 0.55, s * 0.2, "#09090b");
  pRect(ctx, s * 0.3, s * 0.5, s * 0.4, s * 0.1, "#64748b");

  // Side weapon pylons with rocket pods
  pRect(ctx, -s * 0.4, -s * 0.55, s * 0.4, s * 0.12, "#1e293b");
  pRect(ctx, -s * 0.4, s * 0.43, s * 0.4, s * 0.12, "#1e293b");
  pRect(ctx, -s * 0.45, -s * 0.62, s * 0.12, s * 0.3, "#ea580c");
  pRect(ctx, -s * 0.45, s * 0.32, s * 0.12, s * 0.3, "#ea580c");

  // Muzzle flash
  if (firingAge === 0 || firingAge === 1) {
    pRect(ctx, s * 0.75, s * 0.35, 4, 4, "#fef08a");
    pRect(ctx, s * 0.8, s * 0.37, 2, 2, "#ffffff");
  }
}

/** Interceptor: Supersonic jet fighter (+X is forward) */
function drawInterceptor(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
  firingAge: number = -1,
): void {
  const s = Math.max(3, Math.floor(z * 0.40));

  // Drop shadow
  pRect(ctx, -s * 0.65, -s * 0.75 + 2, s * 1.3, s * 1.5, "rgba(0, 0, 0, 0.55)");

  // Swept delta wings (spanning Y, swept back toward -X)
  pRect(ctx, -s * 0.5, -s * 0.7, s * 1.1, s * 0.25, pal.primaryDark);
  pRect(ctx, -s * 0.4, s * 0.45, s * 1.0, s * 0.25, pal.primaryDark);

  // Wingtip missile pods
  pRect(ctx, -s * 0.35, -s * 0.88, s * 0.14, s * 0.42, "#38bdf8");
  pRect(ctx, -s * 0.35, s * 0.46, s * 0.14, s * 0.42, "#38bdf8");

  // Twin canted vertical stabilizers
  pRect(ctx, s * 0.15, -s * 0.55, s * 0.1, s * 0.32, "#60a5fa");
  pRect(ctx, s * 0.15, s * 0.23, s * 0.1, s * 0.32, "#60a5fa");

  // Needle fuselage with radar nose
  pRect(ctx, -s * 0.55, -s * 0.22, s * 1.35, s * 0.44, "#475569");
  pRect(ctx, -s * 0.5, -s * 0.17, s * 1.25, s * 0.34, "#94a3b8");
  pRect(ctx, -s * 0.75, -s * 0.12, s * 0.2, s * 0.24, "#cbd5e1");

  // Holographic cyan cockpit canopy
  pRect(ctx, s * 0.3, -s * 0.12, s * 0.22, s * 0.24, "#0284c7");
  pRect(ctx, s * 0.34, -s * 0.09, s * 0.12, s * 0.18, "#ffffff");

  // Flickering plasma afterburner
  const flicker = Math.floor(tick % 3);
  pRect(ctx, -s * 0.55 - flicker * 1.5, -s * 0.13, s * 0.12 + flicker, s * 0.26, "#ea580c");
  pRect(ctx, -s * 0.6 - flicker, -s * 0.1, 3, s * 0.2, "#facc15");

  // Muzzle flash
  if (firingAge === 0 || firingAge === 1) {
    pRect(ctx, s * 0.8, -s * 0.08, 4, 4, "#fef08a");
    pRect(ctx, s * 0.85, -s * 0.06, 2, 2, "#ffffff");
  }
}

/** Infantry: Chunky C&C pixel rifleman trooper (+X is forward) */
function drawInfantry(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
  firingAge: number = -1,
  isMoving: boolean = false,
): void {
  const s = Math.max(3, Math.floor(z * 0.36));

  // Drop shadow
  pRect(ctx, -s * 0.4, -s * 0.25 + 2, s * 0.8, s * 0.5, "rgba(0, 0, 0, 0.55)");

  // Walking leg stride animation ONLY when actively moving (+X is forward)
  let leg1Off = 0;
  let leg2Off = 0;
  if (isMoving) {
    const walkPhase = Math.sin(tick * 0.5) > 0;
    leg1Off = walkPhase ? -s * 0.2 : s * 0.2;
    leg2Off = -leg1Off;
  }

  // Combat boots (planted in idle ready stance, alternating when marching)
  pRect(ctx, -s * 0.45 + leg1Off, -s * 0.35, s * 0.3, s * 0.2, "#09090b");
  pRect(ctx, -s * 0.45 + leg2Off, s * 0.15, s * 0.3, s * 0.2, "#09090b");

  // Camo fatigue body / torso
  pRect(ctx, -s * 0.35, -s * 0.3, s * 0.65, s * 0.6, "#27272a");
  pRect(ctx, -s * 0.25, -s * 0.25, s * 0.45, s * 0.5, pal.primary);

  // Assault rifle barrel extending forward
  const kick = firingAge === 0 ? -2 : 0;
  pRect(ctx, s * 0.15 + kick, s * 0.12, s * 0.75, Math.max(2, s * 0.18), "#09090b");
  pRect(ctx, s * 0.65 + kick, s * 0.08, s * 0.25, Math.max(2, s * 0.12), "#52525b");

  // Muzzle flash for infantry
  if (firingAge === 0 || firingAge === 1) {
    const tipX = s * 0.9 + kick;
    const tipY = s * 0.12;
    pRect(ctx, tipX, tipY - 2, 4, 4, "#fef08a");
    pRect(ctx, tipX + 1, tipY - 1, 2, 2, "#ffffff");
  }

  // Helmet / head
  const headW = Math.max(6, Math.floor(s * 0.5));
  pRect(ctx, -s * 0.25, -headW / 2, headW, headW, "#18181b");
  pRect(ctx, -s * 0.15, -headW / 2 + 1, headW - 2, headW - 2, pal.primaryDark);

  // Team helmet band & tactical visor (+X is forward)
  pRect(ctx, s * 0.05, -headW * 0.3, 2, headW * 0.6, pal.accent);
}

/** Tank: Heavy C&C battle tank with dual treads & rotating turret (+X is forward) */
function drawTank(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  _tick: number,
  firingAge: number = -1,
): void {
  const s = Math.max(3, Math.floor(z * 0.42));

  // Recoil calculation: 0 = peak fire, 1 = heavy recoil, 2 = recovery, 3 = settling
  let recoil = 0;
  let hullKick = 0;
  if (firingAge >= 0 && firingAge <= 4) {
    if (firingAge === 0) { recoil = s * 0.34; hullKick = -s * 0.08; }
    else if (firingAge === 1) { recoil = s * 0.26; hullKick = -s * 0.05; }
    else if (firingAge === 2) { recoil = s * 0.16; hullKick = -s * 0.02; }
    else if (firingAge === 3) { recoil = s * 0.07; hullKick = 0; }
  }

  // Drop shadow
  pRect(ctx, -s * 0.85 + hullKick, -s * 0.65 + 2, s * 1.7, s * 1.3, "rgba(0, 0, 0, 0.55)");

  // Dual caterpillar treads (+X is forward)
  const tw = Math.floor(s * 1.6);
  const th = Math.max(3, Math.floor(s * 0.32));
  const ty = Math.floor(s * 0.48);

  // Left & Right treads (hard pixel outline + links)
  pRect(ctx, -tw / 2 + hullKick, -ty - th / 2, tw, th, "#09090b");
  pRect(ctx, -tw / 2 + hullKick, ty - th / 2, tw, th, "#09090b");
  for (let x = -tw / 2 + 2; x < tw / 2 - 1; x += 3) {
    pRect(ctx, x + hullKick, -ty - th / 2 + 1, 1.5, th - 2, "#3f3f46");
    pRect(ctx, x + hullKick, ty - th / 2 + 1, 1.5, th - 2, "#3f3f46");
  }

  // Armored chassis hull
  const hw = Math.floor(s * 1.3);
  const hh = Math.floor(s * 0.75);
  pRect(ctx, -hw / 2 + hullKick, -hh / 2, hw, hh, "#18181b");
  pRect(ctx, -hw / 2 + 1 + hullKick, -hh / 2 + 1, hw - 2, hh - 2, "#27272a");

  // Team identification deck plate
  pRect(ctx, -hw * 0.35 + hullKick, -hh * 0.35, hw * 0.7, hh * 0.7, pal.primary);
  pRect(ctx, -hw * 0.3 + hullKick, -hh * 0.3, hw * 0.6, hh * 0.6, pal.primaryDark);

  // Rear exhaust grilles (-X)
  pRect(ctx, -hw / 2 + 2 + hullKick, -hh * 0.3, 2, hh * 0.6, "#09090b");

  // Main cannon barrel extending forward (+X) with dynamic recoil offset
  const blen = Math.floor(s * 1.25);
  const bw = Math.max(2, Math.floor(s * 0.2));
  const barrelStart = -recoil;
  const barrelEnd = blen - recoil;
  pRect(ctx, barrelStart, -bw / 2, blen, bw, "#09090b");
  pRect(ctx, barrelStart + 1, -bw / 2 + 0.5, blen - 2, bw - 1, "#3f3f46");

  // Muzzle brake on cannon tip
  const brakeColor = firingAge >= 0 && firingAge <= 2 ? "#d97706" : "#18181b";
  pRect(ctx, barrelEnd - 3, -bw / 2 - 1, 3, bw + 2, brakeColor);

  // Armored box turret
  const tr = Math.floor(s * 0.45);
  pRect(ctx, -tr + hullKick * 0.5, -tr, tr * 2, tr * 2, "#09090b");
  pRect(ctx, -tr + 1 + hullKick * 0.5, -tr + 1, tr * 2 - 2, tr * 2 - 2, pal.primary);

  // Commander cupola hatch
  pRect(ctx, -tr * 0.3 + hullKick * 0.5, -tr * 0.3, tr * 0.7, tr * 0.7, "#18181b");

  // Muzzle blast explosive firing animation
  if (firingAge >= 0 && firingAge <= 2) {
    const tipX = barrelEnd;
    if (firingAge <= 1) {
      // Big explosive muzzle flash burst (star)
      const flashSize = Math.floor(s * 0.65);
      pRect(ctx, tipX, -flashSize * 0.4, flashSize * 1.1, flashSize * 0.8, "#f97316"); // Outer orange fire
      pRect(ctx, tipX + 1, -flashSize * 0.25, flashSize * 0.8, flashSize * 0.5, "#fef08a"); // Bright yellow core
      pRect(ctx, tipX + 2, -flashSize * 0.12, flashSize * 0.5, flashSize * 0.24, "#ffffff"); // Pure white center

      // Side muzzle vent flame jets
      pRect(ctx, tipX - 2, -bw / 2 - flashSize * 0.35, 3, flashSize * 0.35, "#fbbf24");
      pRect(ctx, tipX - 2, bw / 2 + 1, 3, flashSize * 0.35, "#fbbf24");

      // Muzzle smoke cloud
      pRect(ctx, tipX + flashSize * 0.9, -flashSize * 0.3, flashSize * 0.5, flashSize * 0.6, "#71717a");
    } else if (firingAge === 2) {
      // Dissipating smoke puff
      const smokeSize = Math.floor(s * 0.45);
      pRect(ctx, tipX + 4, -smokeSize * 0.4, smokeSize, smokeSize * 0.8, "#52525b");
      pRect(ctx, tipX + 8, -smokeSize * 0.2, smokeSize * 0.6, smokeSize * 0.4, "#71717a");
    }
  }
}

/** Artillery: Heavy self-propelled siege howitzer crawler (+X is forward) */
function drawArtillery(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  _tick: number,
  firingAge: number = -1,
): void {
  const s = Math.max(3, Math.floor(z * 0.42));

  // Recoil calculation
  let recoil = 0;
  if (firingAge >= 0 && firingAge <= 5) {
    if (firingAge === 0) recoil = s * 0.45;
    else if (firingAge === 1) recoil = s * 0.38;
    else if (firingAge === 2) recoil = s * 0.26;
    else if (firingAge === 3) recoil = s * 0.15;
    else if (firingAge === 4) recoil = s * 0.07;
  }

  // Drop shadow
  pRect(ctx, -s * 0.85, -s * 0.75 + 2, s * 1.7, s * 1.5, "rgba(0, 0, 0, 0.55)");

  // Outrigger tracked treads
  const tw = Math.floor(s * 1.55);
  const th = Math.max(3.5, Math.floor(s * 0.35));
  const ty = Math.floor(s * 0.58);

  pRect(ctx, -tw / 2, -ty - th / 2, tw, th, "#09090b");
  pRect(ctx, -tw / 2, ty - th / 2, tw, th, "#09090b");
  for (let x = -tw / 2 + 2; x < tw / 2 - 1; x += 3.5) {
    pRect(ctx, x, -ty - th / 2 + 1, 1.5, th - 2, "#3f3f46");
    pRect(ctx, x, ty - th / 2 + 1, 1.5, th - 2, "#3f3f46");
  }

  // Stabilizer arm crossbeams
  pRect(ctx, -s * 0.45, -ty + th / 2, s * 0.9, ty * 2 - th, "#27272a");

  // Heavy armored chassis
  const bw = Math.floor(s * 1.15);
  const bh = Math.floor(s * 0.8);
  pRect(ctx, -bw / 2, -bh / 2, bw, bh, "#18181b");
  pRect(ctx, -bw / 2 + 1, -bh / 2 + 1, bw - 2, bh - 2, pal.primaryDark);
  pRect(ctx, -bw * 0.45, -bh * 0.35, bw * 0.4, bh * 0.7, pal.primary);

  // Massive elevated siege railgun barrel (+X is forward)
  const blen = Math.floor(s * 1.75);
  const barrelW = Math.max(3, Math.floor(s * 0.28));

  // Recoil housing
  pRect(ctx, -s * 0.2, -barrelW * 0.8, s * 0.7, barrelW * 1.6, "#09090b");
  pRect(ctx, -s * 0.15, -barrelW * 0.6, s * 0.6, barrelW * 1.2, "#27272a");

  // Long gun barrel with reinforced sleeve & recoil offset
  const bStart = -recoil;
  const bEnd = blen - recoil;
  pRect(ctx, bStart, -barrelW / 2, blen, barrelW, "#09090b");
  pRect(ctx, bStart + 1, -barrelW / 2 + 0.5, blen - 2, barrelW - 1, "#3f3f46");
  pRect(ctx, bStart + blen * 0.4, -barrelW / 2 - 1, s * 0.35, barrelW + 2, "#18181b");

  // Massive siege muzzle blast
  if (firingAge >= 0 && firingAge <= 2) {
    const tipX = bEnd;
    const blastSize = Math.floor(s * 0.85);
    pRect(ctx, tipX, -blastSize * 0.5, blastSize * 1.3, blastSize, "#f97316");
    pRect(ctx, tipX + 2, -blastSize * 0.3, blastSize * 0.9, blastSize * 0.6, "#fef08a");
    pRect(ctx, tipX + 4, -blastSize * 0.15, blastSize * 0.5, blastSize * 0.3, "#ffffff");
  }
}

// ---------------------------------------------------------------------------
// 2.5D Isometric Building Sprites (Command & Conquer Tier Industrial Architecture)
// ---------------------------------------------------------------------------

export function drawBuildingSprite(
  ctx: CanvasRenderingContext2D,
  kind: string,
  px: number,
  py: number,
  zoom: number,
  owner: number,
  heading: number = 0,
  tick: number = 0,
  isStale: boolean = false,
  progress: number = 0,
  buildTime: number = 0,
  firingAge: number = -1,
): void {
  const pal = getTeamPalette(owner, isStale);

  ctx.save();
  ctx.translate(Math.floor(px), Math.floor(py));

  switch (kind) {
    case "Hq":
      drawHq(ctx, zoom, pal, tick);
      break;
    case "PowerPlant":
      drawPowerPlant(ctx, zoom, pal, tick);
      break;
    case "Refinery":
      drawRefinery(ctx, zoom, pal, tick);
      break;
    case "Barracks":
      drawBarracks(ctx, zoom, pal, tick);
      break;
    case "Factory":
      drawFactory(ctx, zoom, pal, tick);
      break;
    case "TechLab":
      drawTechLab(ctx, zoom, pal, tick);
      break;
    case "Airfield":
      drawAirfield(ctx, zoom, pal, tick);
      break;
    case "Radar":
      drawRadar(ctx, zoom, pal, tick);
      break;
    case "TeslaCoil":
      drawTeslaCoil(ctx, zoom, pal, heading, tick, firingAge);
      break;
    case "Turret":
      drawTurret(ctx, zoom, pal, heading, tick, firingAge);
      break;
    case "CrystalRefinery":
      drawCrystalRefinery(ctx, zoom, pal, tick);
      break;
    case "AATurret":
      drawAATurret(ctx, zoom, pal, heading, tick, firingAge);
      break;
    default:
      pRect(ctx, -zoom * 0.4, -zoom * 0.4, zoom * 0.8, zoom * 0.8, pal.primary);
  }

  // Civilization-style un-greying construction & production progression
  if (buildTime > 0) {
    drawCivUnGreyOverlay(ctx, 0, 0, zoom * 0.92, zoom * 0.92, progress, buildTime, tick);
  }

  ctx.restore();
}

/** HQ / Construction Yard: Fortified 2.5D concrete command fortress with rotating radar array */
function drawHq(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
): void {
  const r = Math.max(3, Math.floor(z * 0.47));

  // 1. Soft directional ground shadow (South-East projection)
  pRect(ctx, -r + 5, -r + 7, r * 2 + 5, r * 2 + 3, "rgba(0, 0, 0, 0.6)");

  // 2. Fortified hexagonal concrete foundation slab with 3D bevels
  pRect(ctx, -r, -r, r * 2, r * 2, "#0a0f1d");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#1e293b");

  // Top/Left 3D concrete highlight (North-West light)
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, 2, "#475569");
  pRect(ctx, -r + 1, -r + 1, 2, r * 2 - 2, "#475569");
  pRect(ctx, -r + 2, -r + 2, r * 2 - 4, 1, "#64748b");
  // Bottom/Right 3D concrete shadow
  pRect(ctx, -r + 1, r - 3, r * 2 - 2, 2, "#090d16");
  pRect(ctx, r - 3, -r + 1, 2, r * 2 - 2, "#090d16");

  // 3. Four reinforced corner blast defense turrets / pillars
  const pillarW = Math.floor(r * 0.38);
  const corners = [
    [-r + 2, -r + 2],
    [r - pillarW - 2, -r + 2],
    [-r + 2, r - pillarW - 2],
    [r - pillarW - 2, r - pillarW - 2],
  ];
  for (const [cx, cy] of corners) {
    pRect(ctx, cx, cy, pillarW, pillarW, "#090d16");
    pRect(ctx, cx + 1, cy + 1, pillarW - 2, pillarW - 2, "#334155");
    pRect(ctx, cx + 1, cy + 1, pillarW - 2, 1, "#64748b");
    pRect(ctx, cx + 1, cy + 1, 1, pillarW - 2, "#64748b");
    // Gun slit
    pRect(ctx, cx + pillarW / 2 - 1, cy + pillarW / 2 - 1, 2, 2, "#09090b");
  }

  // 4. Left Module: Power & Generator Bank
  const genW = Math.floor(r * 0.55);
  const genH = Math.floor(r * 0.7);
  pRect(ctx, -r * 0.9, -r * 0.35, genW, genH, "#090d16");
  pRect(ctx, -r * 0.9 + 1, -r * 0.35 + 1, genW - 2, genH - 2, "#273549");
  // Dual ventilation fans
  pRect(ctx, -r * 0.8, -r * 0.25, genW - 4, 3, "#090d16");
  pRect(ctx, -r * 0.8, -r * 0.05, genW - 4, 3, "#090d16");

  // 5. Right Module: Telemetry & Comms Array
  const comW = Math.floor(r * 0.55);
  const comH = Math.floor(r * 0.7);
  pRect(ctx, r * 0.35, -r * 0.35, comW, comH, "#090d16");
  pRect(ctx, r * 0.35 + 1, -r * 0.35 + 1, comW - 2, comH - 2, pal.primaryDark);
  // Status telemetry lights
  pRect(ctx, r * 0.45, -r * 0.25, 2, 2, "#22c55e");
  pRect(ctx, r * 0.45, -r * 0.1, 2, 2, "#38bdf8");
  pRect(ctx, r * 0.45, r * 0.05, 2, 2, "#facc15");

  // 6. Central Elevated Command Citadel Fortress (2.5D Raised Roof)
  const citW = Math.floor(r * 1.25);
  const citH = Math.floor(r * 1.15);
  pRect(ctx, -citW / 2, -citH / 2 - 2, citW, citH, "#090d16");
  pRect(ctx, -citW / 2 + 1, -citH / 2 - 1, citW - 2, citH - 2, pal.primaryDark);
  pRect(ctx, -citW / 2 + 2, -citH / 2, citW - 4, citH - 4, pal.primary);

  // Roof armor plate seams & top highlight
  pRect(ctx, -citW / 2 + 2, -citH / 2, citW - 4, 1, pal.primaryLight);
  pRect(ctx, -citW / 2 + 2, -citH / 2, 1, citH - 4, pal.primaryLight);

  // 7. Tactical Command Observation Dome (Cyan glass vision slit)
  const domeW = Math.floor(citW * 0.65);
  pRect(ctx, -domeW / 2, -citH * 0.35, domeW, 5, "#090d16");
  pRect(ctx, -domeW / 2 + 1, -citH * 0.35 + 1, domeW - 2, 3, "#38bdf8");
  pRect(ctx, -domeW / 2 + 2, -citH * 0.35 + 1, domeW - 4, 1, "#e0f2fe"); // Specular glint

  // 8. Rotating Parabolic Radar Dish Array
  const radarPhase = Math.floor((tick * 0.12) % 8);
  const radarAngles = [
    [-5, -r * 0.65, 5, -r * 0.65],
    [-4, -r * 0.7, 4, -r * 0.6],
    [-2, -r * 0.75, 2, -r * 0.55],
    [2, -r * 0.75, -2, -r * 0.55],
    [5, -r * 0.65, -5, -r * 0.65],
    [4, -r * 0.6, -4, -r * 0.7],
    [2, -r * 0.55, -2, -r * 0.75],
    [-2, -r * 0.55, 2, -r * 0.75],
  ];
  const [rx1, ry1, rx2, ry2] = radarAngles[radarPhase];

  // Steel lattice pedestal
  pRect(ctx, -3, -r * 0.7, 6, 7, "#090d16");
  pRect(ctx, -2, -r * 0.7 + 1, 4, 5, "#475569");
  // Radar dish line
  ctx.strokeStyle = "#fef08a";
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.moveTo(rx1, ry1);
  ctx.lineTo(rx2, ry2);
  ctx.stroke();

  // Radar transmitter feedhorn spark
  pRect(ctx, (rx1 + rx2) / 2 - 1, (ry1 + ry2) / 2 - 1, 2, 2, "#ffffff");

  // Telemetry spire & flashing red beacon
  pRect(ctx, r * 0.7, -r * 1.15, 2, r * 0.45, "#64748b");
  const beaconOn = Math.sin(tick * 0.3) > 0;
  pRect(ctx, r * 0.7 - 1, -r * 1.2, 4, 3, beaconOn ? "#ef4444" : "#7f1d1d");

  // 9. Reinforced Underground Blast Hangar Gate with hazard sill
  const gateW = Math.floor(r * 0.9);
  const gateH = Math.floor(r * 0.45);
  pRect(ctx, -gateW / 2, r * 0.35, gateW, gateH, "#090d16");
  pRect(ctx, -gateW / 2 + 1, r * 0.35 + 1, gateW - 2, gateH - 2, "#1e293b");

  // Vertical gate rib seams
  for (let gx = -gateW / 2 + 3; gx < gateW / 2 - 2; gx += 4) {
    pRect(ctx, gx, r * 0.35 + 1, 1.5, gateH - 2, "#090d16");
  }
  pHazard(ctx, -gateW / 2 + 1, r * 0.74, gateW - 2, 3);
}

/** Power Plant / Generator: Heavy industrial fusion dynamo complex with twin cooling towers & plasma reactor core */
function drawPowerPlant(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
): void {
  const r = Math.max(3, Math.floor(z * 0.45));

  // 1. Soft directional ground shadow (South-East projection)
  pRect(ctx, -r + 4, -r + 6, r * 2 + 4, r * 2 + 2, "rgba(0, 0, 0, 0.6)");

  // 2. Heavy industrial concrete foundation slab with steel reinforcing
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#181f2a");

  // Hazard striped warning border on foundation edges
  pHazard(ctx, -r + 2, r - 5, r * 2 - 4, 3);

  // 3. Twin Heavy Cooling Turbines / Hyperbolic Towers (West and East)
  const towerR = Math.floor(r * 0.38);
  const towerH = Math.floor(r * 0.95);
  const towerY = -r * 0.85;

  // Left Cooling Tower
  pRect(ctx, -r * 0.85, towerY, towerR * 2, towerH, "#0f172a");
  pRect(ctx, -r * 0.85 + 1, towerY + 1, towerR * 2 - 2, towerH - 2, "#334155");
  pRect(ctx, -r * 0.85 + 2, towerY + 2, towerR * 2 - 4, 3, "#090d16"); // Chimney lip
  pRect(ctx, -r * 0.85 + 3, towerY + 3, towerR * 2 - 6, 2, "#0284c7"); // Internal glow

  // Right Cooling Tower
  pRect(ctx, r * 0.85 - towerR * 2, towerY, towerR * 2, towerH, "#0f172a");
  pRect(ctx, r * 0.85 - towerR * 2 + 1, towerY + 1, towerR * 2 - 2, towerH - 2, "#334155");
  pRect(ctx, r * 0.85 - towerR * 2 + 2, towerY + 2, towerR * 2 - 4, 3, "#090d16");
  pRect(ctx, r * 0.85 - towerR * 2 + 3, towerY + 3, towerR * 2 - 6, 2, "#0284c7");

  // Animated steam vapor puffs from cooling chimneys
  const puff1 = Math.floor((tick * 0.2) % 3);
  const puff2 = Math.floor((tick * 0.2 + 1.5) % 3);
  pRect(ctx, -r * 0.85 + 2 + puff1, towerY - puff1 * 2 - 3, towerR * 2 - 4, 3 + puff1, "#94a3b8");
  pRect(ctx, r * 0.85 - towerR * 2 + 2 + puff2, towerY - puff2 * 2 - 3, towerR * 2 - 4, 3 + puff2, "#94a3b8");

  // 4. Central High-Energy Plasma Fusion Reactor Core
  const coreW = Math.floor(r * 0.85);
  const coreH = Math.floor(r * 0.75);
  const coreY = -r * 0.25;

  pRect(ctx, -coreW / 2, coreY, coreW, coreH, "#090d16");
  pRect(ctx, -coreW / 2 + 1, coreY + 1, coreW - 2, coreH - 2, pal.primaryDark);
  pRect(ctx, -coreW / 2 + 2, coreY + 2, coreW - 4, coreH - 4, pal.primary);

  // Glowing energetic plasma core window (pulsing brightness)
  const pulse = Math.sin(tick * 0.35) * 0.5 + 0.5;
  const plasmaCoreColor = pulse > 0.5 ? "#38bdf8" : "#0284c7";
  const plasmaHighlight = pulse > 0.5 ? "#ffffff" : "#bae6fd";
  pRect(ctx, -coreW * 0.28, coreY + coreH * 0.2, coreW * 0.56, coreH * 0.6, "#082f49");
  pRect(ctx, -coreW * 0.22, coreY + coreH * 0.28, coreW * 0.44, coreH * 0.44, plasmaCoreColor);
  pRect(ctx, -coreW * 0.12, coreY + coreH * 0.36, coreW * 0.24, coreH * 0.28, plasmaHighlight);

  // High-voltage lightning bolt emblem on generator mantle
  pRect(ctx, -1, coreY - 4, 2, 4, "#facc15");
  pRect(ctx, -3, coreY - 2, 6, 2, "#fde047");

  // Heavy high-voltage power conduit busbars linking towers to core
  pRect(ctx, -r * 0.7, coreY + 3, r * 1.4, 2, "#eab308");
  pRect(ctx, -r * 0.7, coreY + 4, r * 1.4, 1, "#fde047");
}

/** Refinery: Heavy industrial Tiberium smelting complex with twin smokestacks & molten vat */
function drawRefinery(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
): void {
  const r = Math.max(3, Math.floor(z * 0.46));

  // 1. Ground shadow
  pRect(ctx, -r + 5, -r + 7, r * 2 + 5, r * 2 + 3, "rgba(0, 0, 0, 0.6)");

  // 2. Reinforced industrial concrete foundation
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#181f2a");

  // 3. Dual massive industrial exhaust smokestacks (North-West)
  const stackW = Math.max(6, Math.floor(r * 0.34));
  const stackH = Math.floor(r * 1.05);
  const stackY = -r * 1.05;

  // Stack 1 (Left)
  pRect(ctx, -r * 0.88, stackY, stackW, stackH, "#090d16");
  pRect(ctx, -r * 0.88 + 1, stackY + 1, stackW - 2, stackH - 2, "#334155");
  pRect(ctx, -r * 0.88 + 1, stackY + 1, 1.5, stackH - 2, "#64748b"); // Highlight
  pRect(ctx, -r * 0.88, stackY + stackH * 0.35, stackW, 2, "#090d16"); // Reinforcing band
  pRect(ctx, -r * 0.88, stackY + stackH * 0.7, stackW, 2, "#090d16");

  // Stack 2 (Right)
  pRect(ctx, -r * 0.42, stackY, stackW, stackH, "#090d16");
  pRect(ctx, -r * 0.42 + 1, stackY + 1, stackW - 2, stackH - 2, "#334155");
  pRect(ctx, -r * 0.42 + 1, stackY + 1, 1.5, stackH - 2, "#64748b");
  pRect(ctx, -r * 0.42, stackY + stackH * 0.35, stackW, 2, "#090d16");
  pRect(ctx, -r * 0.42, stackY + stackH * 0.7, stackW, 2, "#090d16");

  // Volumetric pixelated smoke clouds from chimneys
  const puff1 = Math.floor((tick * 0.25) % 4);
  const puff2 = Math.floor((tick * 0.25 + 2) % 4);
  pRect(ctx, -r * 0.88 + puff1 - 1, stackY - puff1 * 3 - 4, 5 + puff1, 4 + puff1, "#71717a");
  pRect(ctx, -r * 0.42 + puff2 - 1, stackY - puff2 * 3 - 4, 5 + puff2, 4 + puff2, "#71717a");

  // 4. Pressurized Silo Tank (North-East)
  const siloW = Math.floor(r * 0.7);
  const siloH = Math.floor(r * 0.9);
  pRect(ctx, r * 0.15, -r * 0.9, siloW, siloH, "#090d16");
  pRect(ctx, r * 0.15 + 1, -r * 0.9 + 1, siloW - 2, siloH - 2, pal.primary);
  pRect(ctx, r * 0.15 + 2, -r * 0.9 + 1, 1.5, siloH - 2, pal.primaryLight);
  // Silo capacity level indicator lights
  pRect(ctx, r * 0.15 + siloW - 4, -r * 0.9 + 3, 2, 2, "#22c55e");
  pRect(ctx, r * 0.15 + siloW - 4, -r * 0.9 + 7, 2, 2, "#22c55e");
  pRect(ctx, r * 0.15 + siloW - 4, -r * 0.9 + 11, 2, 2, "#eab308");

  // 5. Central Molten Smelting Vat
  const vatW = Math.floor(r * 1.2);
  const vatH = Math.floor(r * 0.52);
  pRect(ctx, -vatW / 2, -r * 0.1, vatW, vatH, "#090d16");
  pRect(ctx, -vatW / 2 + 1, -r * 0.1 + 1, vatW - 2, vatH - 2, "#78350f");
  pRect(ctx, -vatW / 2 + 2, -r * 0.1 + 2, vatW - 4, vatH - 4, "#d97706");
  pRect(ctx, -vatW / 2 + 4, -r * 0.1 + 4, vatW - 8, vatH - 8, "#fef08a");

  // Vat boiling bubble pixel
  const bubble = Math.sin(tick * 0.3) > 0 ? 1 : -1;
  pRect(ctx, -2 + bubble * 5, -r * 0.1 + 4, 3, 2, "#ffffff");

  // 6. Resource intake hopper with hazard ramp (the deposit feeds in here)
  const dockW = Math.floor(r * 1.35);
  const dockH = Math.floor(r * 0.6);
  pRect(ctx, -dockW / 2, r * 0.35, dockW, dockH, "#090d16");
  pRect(ctx, -dockW / 2 + 1, r * 0.35 + 1, dockW - 2, dockH - 2, pal.primaryDark);

  // Intake conveyor belt & hazard stripes
  pRect(ctx, -dockW * 0.35, r * 0.38, dockW * 0.7, dockH * 0.55, "#1c1917");
  pHazard(ctx, -dockW / 2 + 2, r * 0.72, dockW - 4, 3);
}

/** Barracks: Hardened military infantry garrison fortress with chevron roof & searchlight */
function drawBarracks(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
): void {
  const r = Math.max(3, Math.floor(z * 0.45));

  // 1. Ground shadow
  pRect(ctx, -r + 4, -r + 6, r * 2 + 4, r * 2 + 2, "rgba(0, 0, 0, 0.6)");

  // 2. Hardened steel-reinforced bunker hull
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#1e293b");

  // 3. Multi-tier chevron blast armor roof
  const roofW = Math.floor(r * 1.65);
  const roofH = Math.floor(r * 0.9);
  pRect(ctx, -roofW / 2, -r * 0.9, roofW, roofH, "#090d16");
  pRect(ctx, -roofW / 2 + 1, -r * 0.9 + 1, roofW - 2, roofH - 2, pal.primaryDark);
  pRect(ctx, -roofW / 2 + 2, -r * 0.9 + 2, roofW - 4, roofH - 4, pal.primary);

  // Chevron tactical rank insignia on roof
  pRect(ctx, -5, -r * 0.7, 10, 2, "#fef08a");
  pRect(ctx, -3, -r * 0.58, 6, 2, "#fef08a");
  pRect(ctx, -1, -r * 0.46, 2, 2, "#fef08a");

  // Roof observation cupola / watchtower with searchlight
  pRect(ctx, r * 0.35, -r * 1.05, r * 0.4, r * 0.35, "#090d16");
  pRect(ctx, r * 0.35 + 1, -r * 1.05 + 1, r * 0.4 - 2, r * 0.35 - 2, "#334155");
  // Searchlight beam glint
  const searchPhase = Math.sin(tick * 0.15) > 0;
  pRect(ctx, r * 0.35 + 2, -r * 1.05 + 2, 3, 2, searchPhase ? "#fef08a" : "#ca8a04");

  // Communications antenna mast with pulsing red beacon
  pRect(ctx, -r * 0.75, -r * 1.25, 2, r * 0.45, "#64748b");
  pRect(ctx, -r * 0.75 - 1, -r * 1.3, 4, 3, (tick % 10 < 5) ? "#ef4444" : "#7f1d1d");

  // 4. Double hydraulic sliding blast entrance
  const doorW = Math.floor(r * 1.05);
  const doorH = Math.floor(r * 0.7);
  pRect(ctx, -doorW / 2, r * 0.18, doorW, doorH, "#090d16");

  // Left & Right armored door leaves
  const halfDoor = Math.floor(doorW / 2 - 2);
  pRect(ctx, -doorW / 2 + 1, r * 0.18 + 1, halfDoor, doorH - 2, "#334155");
  pRect(ctx, 1, r * 0.18 + 1, halfDoor, doorH - 2, "#334155");

  // Hydraulic pistons & green ready lights
  pRect(ctx, -doorW / 2 + 2, r * 0.26, 2, 2, "#22c55e");
  pRect(ctx, doorW / 2 - 4, r * 0.26, 2, 2, "#22c55e");

  // Door caution perimeter
  pRect(ctx, -doorW / 2, r * 0.8, doorW, 2, "#ca8a04");
}

/** Factory / War Factory: Industrial armor foundry with crane gantry & roll-up bay door */
function drawFactory(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
): void {
  const r = Math.max(3, Math.floor(z * 0.46));

  // 1. Ground shadow
  pRect(ctx, -r + 5, -r + 7, r * 2 + 5, r * 2 + 3, "rgba(0, 0, 0, 0.6)");

  // 2. Heavy steel fabrication hall
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#181f2a");

  // 3. Overhead structural crane gantry track across roof
  const gantryW = Math.floor(r * 1.85);
  const gantryH = Math.floor(r * 0.42);
  pRect(ctx, -gantryW / 2, -r * 0.88, gantryW, gantryH, "#090d16");
  pRect(ctx, -gantryW / 2 + 1, -r * 0.88 + 1, gantryW - 2, gantryH - 2, "#334155");

  // Animated motorized gantry crane trolley
  const trolleyPos = Math.floor(Math.sin(tick * 0.15) * (gantryW * 0.32));
  pRect(ctx, trolleyPos - 4, -r * 0.88 + 1, 8, gantryH - 2, "#facc15");
  pRect(ctx, trolleyPos - 1, -r * 0.88 + gantryH, 2, 4, "#ca8a04"); // Crane hoist hook

  // 4. Team banner identification stripe
  pRect(ctx, -r * 0.88, -r * 0.35, r * 1.76, 4, pal.primary);
  pRect(ctx, -r * 0.88, -r * 0.35, r * 1.76, 1, pal.primaryLight);

  // 5. Heavy segmented roll-up vehicle assembly bay door
  const bayW = Math.floor(r * 1.5);
  const bayH = Math.floor(r * 0.95);
  pRect(ctx, -bayW / 2, r * 0.05, bayW, bayH, "#090d16");
  pRect(ctx, -bayW / 2 + 1, r * 0.05 + 1, bayW - 2, bayH - 2, "#1e293b");

  // Horizontal roll-up steel slats
  for (let by = r * 0.15; by < r * 0.85; by += 4) {
    pRect(ctx, -bayW / 2 + 2, by, bayW - 4, 1.5, "#090d16");
  }

  // Industrial yellow/black caution stripes at threshold
  pHazard(ctx, -bayW / 2 + 2, r * 0.82, bayW - 4, 3);

  // 6. External electrical sub-station transformer with electrical arc glint
  pRect(ctx, r * 0.58, -r * 0.78, r * 0.38, r * 0.38, "#090d16");
  pRect(ctx, r * 0.58 + 1, -r * 0.78 + 1, r * 0.38 - 2, r * 0.38 - 2, "#475569");
  const spark = (tick % 14) === 0;
  if (spark) {
    pRect(ctx, r * 0.68, -r * 0.88, 3, 3, "#38bdf8");
  }
}

/** TechLab: High-tech communications & radar dome complex */
function drawTechLab(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
): void {
  const r = Math.max(3, Math.floor(z * 0.44));

  // 1. Ground shadow
  pRect(ctx, -r + 4, -r + 6, r * 2 + 4, r * 2 + 2, "rgba(0, 0, 0, 0.6)");

  // 2. Octagonal fortified bunker platform
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#181f2a");

  // 3. Geodesic sensor dome with faceted hexagonal panels
  const domeR = Math.floor(r * 0.82);
  pRect(ctx, -domeR, -domeR, domeR * 2, domeR * 2, "#090d16");
  pRect(ctx, -domeR + 1, -domeR + 1, domeR * 2 - 2, domeR * 2 - 2, pal.primaryDark);
  pRect(ctx, -domeR + 2, -domeR + 2, domeR * 2 - 4, domeR * 2 - 4, pal.primary);

  // Faceted geodesic seams
  pRect(ctx, -domeR + 2, 0, domeR * 2 - 4, 1, pal.primaryLight);
  pRect(ctx, 0, -domeR + 2, 1, domeR * 2 - 4, pal.primaryLight);
  pRect(ctx, -domeR * 0.6, -domeR * 0.6, domeR * 1.2, 1, pal.primaryLight);
  pRect(ctx, -domeR * 0.6, domeR * 0.6, domeR * 1.2, 1, pal.primaryLight);

  // 4. Central communications transmitter mast & status beacon
  pRect(ctx, -1.5, -r * 1.05, 3, r * 1.05, "#64748b");
  pRect(ctx, -5, -r * 1.05, 10, 2, "#38bdf8");

  // Pulsing red danger beacon on spire tip
  const beaconBlink = Math.sin(tick * 0.35) > 0;
  pRect(ctx, -1.5, -r * 1.15, 3, 3, beaconBlink ? "#ef4444" : "#7f1d1d");

  // 5. Glowing energy conduit circuit busbars
  pRect(ctx, -r * 0.7, r * 0.45, r * 1.4, 2, pal.accent);
  pRect(ctx, -r * 0.7, r * 0.45, 2, r * 0.35, pal.accent);
  pRect(ctx, r * 0.7 - 2, r * 0.45, 2, r * 0.35, pal.accent);
}

/** Airfield: Reinforced tarmac base with twin helipads, control tower & rotating radar */
function drawAirfield(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
): void {
  const r = Math.max(3, Math.floor(z * 0.46));

  // 1. Ground shadow
  pRect(ctx, -r + 5, -r + 7, r * 2 + 5, r * 2 + 3, "rgba(0, 0, 0, 0.6)");

  // 2. Heavy reinforced concrete tarmac foundation
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#181f2a");
  pRect(ctx, -r + 2, -r + 2, r * 2 - 4, r * 2 - 4, "#242d3a");

  // 3. Perimeter yellow/black hazard markings
  pHazard(ctx, -r + 3, -r + 3, r * 2 - 6, 2.5);
  pHazard(ctx, -r + 3, r - 5.5, r * 2 - 6, 2.5);

  // 4. Dual illuminated landing pads (Left Pad & Right Pad)
  const padW = Math.floor(r * 0.72);
  const padH = Math.floor(r * 0.72);

  // Left Helipad / Landing Box
  const pad1X = -r * 0.52;
  const pad1Y = 0;
  pRect(ctx, pad1X - padW / 2, pad1Y - padH / 2, padW, padH, "#0f172a");
  pRect(ctx, pad1X - padW / 2 + 1, pad1Y - padH / 2 + 1, padW - 2, padH - 2, "#1e293b");
  // Yellow [ H ] stencil
  pRect(ctx, pad1X - 5, pad1Y - 6, 2, 12, "#facc15");
  pRect(ctx, pad1X + 3, pad1Y - 6, 2, 12, "#facc15");
  pRect(ctx, pad1X - 5, pad1Y - 1, 10, 2, "#facc15");

  // Right Helipad / Landing Box
  const pad2X = r * 0.42;
  const pad2Y = r * 0.15;
  pRect(ctx, pad2X - padW / 2, pad2Y - padH / 2, padW, padH, "#0f172a");
  pRect(ctx, pad2X - padW / 2 + 1, pad2Y - padH / 2 + 1, padW - 2, padH - 2, "#1e293b");
  // Yellow [ H ] stencil
  pRect(ctx, pad2X - 5, pad2Y - 6, 2, 12, "#facc15");
  pRect(ctx, pad2X + 3, pad2Y - 6, 2, 12, "#facc15");
  pRect(ctx, pad2X - 5, pad2Y - 1, 10, 2, "#facc15");

  // 5. Team banner stripe
  pRect(ctx, -r * 0.85, -r * 0.55, r * 0.9, 3, pal.primary);

  // 6. Air Traffic Control Tower & Rotating Radar Scanner at top-right
  const towerX = r * 0.5;
  const towerY = -r * 0.55;
  pRect(ctx, towerX - 6, towerY - 6, 12, 12, "#090d16");
  pRect(ctx, towerX - 5, towerY - 5, 10, 10, "#334155");
  pRect(ctx, towerX - 3, towerY - 3, 6, 6, "#38bdf8"); // Cyan observation deck glass

  // Rotating Radar Scanner Dish
  const radarAngle = (tick * 0.1) % (Math.PI * 2);
  const dishDx = Math.cos(radarAngle) * 5;
  const dishDy = Math.sin(radarAngle) * 2;
  pRect(ctx, towerX - 1, towerY - 12, 2, 6, "#64748b");
  pRect(ctx, towerX + dishDx - 3, towerY - 14 + dishDy, 6, 2, "#cbd5e1");
  pRect(ctx, towerX - 1, towerY - 15, 2, 2, "#ef4444"); // Red beacon

  // 7. Pulsing green/amber runway approach lights along pad edges
  const lightPulse = (tick % 8) < 4;
  pRect(ctx, -r + 4, -r * 0.1, 2, 2, lightPulse ? "#22c55e" : "#14532d");
  pRect(ctx, -r + 4, r * 0.5, 2, 2, lightPulse ? "#22c55e" : "#14532d");
  pRect(ctx, r * 0.1, -r * 0.1, 2, 2, lightPulse ? "#f59e0b" : "#78350f");
  pRect(ctx, r * 0.1, r * 0.5, 2, 2, lightPulse ? "#f59e0b" : "#78350f");
}

/** Radar: long-range early-warning dish with a rotating sweep (reveals the map) */
function drawRadar(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
): void {
  const r = Math.max(3, Math.floor(z * 0.44));

  // 1. Ground shadow
  pRect(ctx, -r + 4, -r + 6, r * 2 + 4, r * 2 + 2, "rgba(0, 0, 0, 0.6)");

  // 2. Armored base platform
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#1e293b");
  pRect(ctx, -r + 2, -r + 2, r * 2 - 4, r * 2 - 4, "#273549");

  // 3. Circular display / array deck
  const deck = Math.floor(r * 0.72);
  pRect(ctx, -deck, -deck, deck * 2, deck * 2, "#0f172a");
  pRect(ctx, -deck + 2, -deck + 2, deck * 2 - 4, deck * 2 - 4, "#0b1c2e");

  // 4. Rotating sweep: a bright radar blip line sweeping around the dish
  const angle = (tick * 0.08) % (Math.PI * 2);
  const sx = Math.cos(angle);
  const sy = Math.sin(angle);
  // Sweep wedge (trailing fade) drawn as a few short segments
  for (let k = 0; k < 5; k++) {
    const a = angle - k * 0.22;
    const len = deck - 4 - k * 2;
    pRect(
      ctx,
      Math.cos(a) * len - 1,
      Math.sin(a) * len - 1,
      2,
      2,
      k === 0 ? "#22d3ee" : "#0e7490",
    );
  }
  // Pivot hub
  pRect(ctx, -2, -2, 4, 4, "#cbd5e1");
  pRect(ctx, -1, -1, 2, 2, "#ffffff");

  // 5. Central feed mast + blip dot at the sweep tip
  pRect(ctx, sx * (deck - 6) - 1, sy * (deck - 6) - 1, 2, 2, "#fbbf24");
  // Team stripe
  pRect(ctx, -r * 0.55, r * 0.62, r * 1.1, 3, pal.primary);
}

/** TeslaCoil: high-voltage arc turret that zaps enemies at long range */
function drawTeslaCoil(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  heading: number = 0,
  tick: number = 0,
  firingAge: number = -1,
): void {
  const r = Math.max(3, Math.floor(z * 0.44));

  // 1. Ground shadow
  pRect(ctx, -r + 4, -r + 5, r * 2 + 3, r * 2 + 2, "rgba(0, 0, 0, 0.6)");

  // 2. Concrete barbette (same footprint as a turret)
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#1e293b");

  // 3. Copper/orange coil rings (stacked, rotated slightly for depth)
  const coilH = Math.floor(r * 1.7);
  const coilW = Math.floor(r * 0.9);
  const ringColor = tick % 4 < 2 ? "#d97706" : "#f59e0b";
  for (let i = 0; i < 5; i++) {
    const y = -coilH / 2 + i * Math.floor(coilH / 5);
    pRect(ctx, -coilW / 2 - 1, y, coilW + 2, 4, "#78350f");
    pRect(ctx, -coilW / 2, y + 1, coilW, 2, i % 2 === 0 ? "#d97706" : ringColor);
  }

  // 4. Central emitter orb with idle corona
  pRect(ctx, -5, -coilH / 2 - 5, 10, 10, "#090d16");
  pRect(ctx, -4, -coilH / 2 - 4, 8, 8, "#1e3a8a");
  const idlePulse = tick % 5 < 2;
  pRect(ctx, -3, -coilH / 2 - 3, 6, 6, idlePulse ? "#38bdf8" : "#0284c7");
  pRect(ctx, -1, -coilH / 2 - 1, 2, 2, "#e0f2fe");

  // 5. Firing arcs: jagged lightning bolts toward the aim heading
  if (firingAge >= 0 && firingAge <= 3) {
    const boltLen = Math.floor(r * 1.6);
    const bx = Math.cos(heading) * boltLen;
    const by = Math.sin(heading) * boltLen;
    const j = (tick % 3) - 1; // jag offset alternates for a lively arc
    ctx.strokeStyle = "#7dd3fc";
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(-2, -coilH / 2);
    ctx.lineTo(bx * 0.4 + j * 2, by * 0.4 - j * 3);
    ctx.lineTo(bx * 0.7 - j * 3, by * 0.7 + j * 2);
    ctx.lineTo(bx, by);
    ctx.stroke();
    pRect(ctx, bx - 2, by - 2, 4, 4, "#f0f9ff");
  }

  // Team stripe
  pRect(ctx, -r * 0.5, r * 0.55, r, 3, pal.primary);
}

/** MammothTank: massive twin-barrel siege armor (the tech-tier heavy) */
function drawMammothTank(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  firingAge: number = -1,
): void {
  const s = Math.max(3, Math.floor(z * 0.46));

  // 1. Drop shadow
  pRect(ctx, -s * 0.7, -s * 0.7 + 2, s * 1.4, s * 1.4, "rgba(0, 0, 0, 0.55)");

  // 2. Wide reinforced hull (twin tracks)
  pRect(ctx, -s, -s * 0.55, s * 2, s * 1.1, "#0f172a");
  pRect(ctx, -s + 1, -s * 0.55 + 1, s * 2 - 2, s * 1.1 - 2, "#1e293b");

  // Tracks with tread segments
  for (let i = -4; i <= 4; i++) {
    const tx = i * Math.floor(s * 0.28);
    pRect(ctx, tx, -s * 0.62, Math.floor(s * 0.18), 4, "#334155");
    pRect(ctx, tx, s * 0.58, Math.floor(s * 0.18), 4, "#334155");
  }

  // 3. Sloped glacis armor
  pRect(ctx, -s * 0.85, -s * 0.45, s * 1.7, s * 0.9, pal.primaryDark);
  pRect(ctx, -s * 0.8, -s * 0.4, s * 1.6, s * 0.8, pal.primary);
  pRect(ctx, -s * 0.75, -s * 0.35, s * 1.5, s * 0.7, pal.primaryLight);

  // 4. Twin rotating turret cupolas
  pRect(ctx, -s * 0.35, -s * 0.3, s * 0.7, s * 0.6, "#1e293b");
  pRect(ctx, -s * 0.3, -s * 0.25, s * 0.6, s * 0.5, "#334155");

  // 5. Twin cannons (+X forward) with recoil on fire
  let recoil = 0;
  if (firingAge >= 0 && firingAge <= 2) {
    recoil = (2 - firingAge) * 2;
  }
  const barrelW = Math.max(3, Math.floor(s * 0.16));
  pRect(ctx, -recoil, -s * 0.5 - barrelW / 2, s * 1.2, barrelW, "#09090b");
  pRect(ctx, -recoil + 1, -s * 0.5 - barrelW / 2 + 0.5, s * 1.2 - 2, barrelW - 1, "#64748b");
  pRect(ctx, -recoil, s * 0.5 - barrelW / 2, s * 1.2, barrelW, "#09090b");
  pRect(ctx, -recoil + 1, s * 0.5 - barrelW / 2 + 0.5, s * 1.2 - 2, barrelW - 1, "#64748b");
  // Muzzle brakes
  pRect(ctx, s * 1.2 - recoil - 3, -s * 0.5 - barrelW / 2 - 1, 3, barrelW + 2, "#181f2a");
  pRect(ctx, s * 1.2 - recoil - 3, s * 0.5 - barrelW / 2 - 1, 3, barrelW + 2, "#181f2a");

  // 6. Twin turret hatches & antenna
  pRect(ctx, -s * 0.22, -s * 0.08, 6, 6, "#090d16");
  pRect(ctx, s * 0.16, -s * 0.08, 6, 6, "#090d16");
  pRect(ctx, -s * 0.05, -s * 0.38, 2, 6, "#94a3b8");
  pRect(ctx, -s * 0.05, -s * 0.44, 4, 2, "#ef4444");
}

/** Turret: Circular reinforced gun emplacement with rotating twin heavy auto-cannons */
function drawTurret(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  heading: number = 0,
  tick: number = 0,
  firingAge: number = -1,
): void {
  const r = Math.max(3, Math.floor(z * 0.44));

  // 1. Ground shadow
  pRect(ctx, -r + 4, -r + 5, r * 2 + 3, r * 2 + 2, "rgba(0, 0, 0, 0.6)");

  // 2. Stationary circular reinforced concrete barbette
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#1e293b");

  // 8 Heavy steel foundation anchor bolts
  const boltR = r - 3;
  const boltPositions = [
    [-boltR, -boltR], [0, -boltR - 1], [boltR, -boltR],
    [-boltR - 1, 0], [boltR + 1, 0],
    [-boltR, boltR], [0, boltR + 1], [boltR, boltR],
  ];
  for (const [bx, by] of boltPositions) {
    pRect(ctx, bx, by, 2, 2, "#64748b");
  }

  // 3. Rotating 3D armored turret cupola & twin cannons (rotated by aim heading)
  ctx.save();
  ctx.rotate(heading);

  const barrelLen = Math.floor(r * 1.5);
  const barrelW = Math.max(2.5, Math.floor(r * 0.24));
  const spacing = Math.floor(r * 0.38);

  // Recoil for twin barrels
  let recoilLeft = 0;
  let recoilRight = 0;
  if (firingAge >= 0 && firingAge <= 3) {
    const rAmt = (3 - firingAge) * 1.5;
    if (tick % 2 === 0) {
      recoilLeft = rAmt;
    } else {
      recoilRight = rAmt;
    }
  }

  // Left barrel (+X is forward)
  pRect(ctx, -recoilLeft, -spacing - barrelW / 2, barrelLen, barrelW, "#090d16");
  pRect(ctx, -recoilLeft + 1, -spacing - barrelW / 2 + 0.5, barrelLen - 2, barrelW - 1, "#475569");
  pRect(ctx, barrelLen - recoilLeft - 3, -spacing - barrelW / 2 - 1, 3, barrelW + 2, "#181f2a"); // Muzzle brake

  // Right barrel (+X is forward)
  pRect(ctx, -recoilRight, spacing - barrelW / 2, barrelLen, barrelW, "#090d16");
  pRect(ctx, -recoilRight + 1, spacing - barrelW / 2 + 0.5, barrelLen - 2, barrelW - 1, "#475569");
  pRect(ctx, barrelLen - recoilRight - 3, spacing - barrelW / 2 - 1, 3, barrelW + 2, "#181f2a"); // Muzzle brake

  // Muzzle flashes on firing barrel
  if (firingAge >= 0 && firingAge <= 1) {
    const flashSize = Math.floor(r * 0.45);
    if (recoilLeft > 0) {
      const tipL = barrelLen - recoilLeft;
      pRect(ctx, tipL, -spacing - flashSize / 2, flashSize * 1.2, flashSize, "#fef08a");
      pRect(ctx, tipL + 1, -spacing - flashSize / 4, flashSize * 0.7, flashSize / 2, "#ffffff");
    }
    if (recoilRight > 0) {
      const tipR = barrelLen - recoilRight;
      pRect(ctx, tipR, spacing - flashSize / 2, flashSize * 1.2, flashSize, "#fef08a");
      pRect(ctx, tipR + 1, spacing - flashSize / 4, flashSize * 0.7, flashSize / 2, "#ffffff");
    }
  }

  // Beveled rotating armored cupola box
  const cupW = Math.floor(r * 0.95);
  pRect(ctx, -cupW / 2, -cupW / 2, cupW, cupW, "#090d16");
  pRect(ctx, -cupW / 2 + 1, -cupW / 2 + 1, cupW - 2, cupW - 2, pal.primaryDark);
  pRect(ctx, -cupW / 2 + 2, -cupW / 2 + 2, cupW - 4, cupW - 4, pal.primary);

  // Red targeting optic vision block slit (+X)
  pRect(ctx, cupW / 2 - 3, -cupW * 0.25, 2, cupW * 0.5, "#ef4444");

  ctx.restore();
}

// ---------------------------------------------------------------------------
// Tactical Icons, Thumbnails & Reticles (Retro C&C Command Sidebar Styling)
// ---------------------------------------------------------------------------

export function drawTacticalIcon(
  ctx: CanvasRenderingContext2D,
  kind: string,
  cx: number,
  cy: number,
  _size: number = 46,
  color: string = "#f59e0b",
  frame: boolean = true,
): void {
  ctx.save();
  ctx.translate(Math.floor(cx), Math.floor(cy));

  if (frame) {
    // Outer beveled frame & high-tech dark carbon backplate
    pRect(ctx, -23, -23, 46, 46, "#020617"); // Deep tactical border
    pRect(ctx, -22, -22, 44, 44, "#0f172a"); // Outer chamfer
    pRect(ctx, -21, -21, 42, 42, "#1e293b"); // Beveled body
    pRect(ctx, -20, -20, 40, 40, "#090d16"); // High-contrast dark interior screen

    // Tactical cyan corner brackets
    pRect(ctx, -20, -20, 4, 1, "#38bdf8");
    pRect(ctx, -20, -20, 1, 4, "#38bdf8");
    pRect(ctx, 16, -20, 4, 1, "#38bdf8");
    pRect(ctx, 19, -20, 1, 4, "#38bdf8");
    pRect(ctx, -20, 19, 4, 1, "#38bdf8");
    pRect(ctx, -20, 16, 1, 4, "#38bdf8");
    pRect(ctx, 16, 19, 4, 1, "#38bdf8");
    pRect(ctx, 19, 16, 1, 4, "#38bdf8");
  }

  const norm = kind.toLowerCase();

  if (norm === "infantry") {
    // Elite commando soldier facing forward
    pRect(ctx, -9, -1, 18, 16, "#1e3a8a");
    pRect(ctx, -8, 0, 16, 14, "#1d4ed8");
    pRect(ctx, -6, 2, 12, 10, "#2563eb");
    pRect(ctx, -6, 6, 4, 5, "#334155");
    pRect(ctx, 2, 6, 4, 5, "#334155");
    pRect(ctx, -2, 6, 4, 5, "#475569");

    // Combat helmet & antenna
    pRect(ctx, -9, -14, 18, 12, "#1e293b");
    pRect(ctx, -8, -13, 16, 10, "#1d4ed8");
    pRect(ctx, -6, -12, 12, 3, "#60a5fa");
    pRect(ctx, -10, -18, 2, 8, "#94a3b8");
    pRect(ctx, -11, -19, 4, 2, "#ef4444");

    // Glowing Cyan Night-Vision Visor
    pRect(ctx, -7, -6, 14, 5, "#0284c7");
    pRect(ctx, -6, -5, 12, 3, "#38bdf8");
    pRect(ctx, -4, -5, 4, 2, "#ffffff");

    // Assault rifle
    pRect(ctx, 4, 0, 12, 4, "#334155");
    pRect(ctx, 6, -1, 9, 2, "#64748b");
    pRect(ctx, 14, -2, 2, 4, "#09090b");
  } else if (norm === "tank") {
    // Heavy battle tank
    pRect(ctx, -16, -14, 32, 6, "#1e293b");
    pRect(ctx, -15, -13, 30, 4, "#334155");
    pRect(ctx, -14, -12, 28, 2, "#64748b");
    pRect(ctx, -16, 8, 32, 6, "#1e293b");
    pRect(ctx, -15, 9, 30, 4, "#334155");
    pRect(ctx, -14, 10, 28, 2, "#64748b");

    pRect(ctx, -14, -8, 28, 16, "#1d4ed8");
    pRect(ctx, -12, -7, 24, 14, "#2563eb");
    pRect(ctx, -10, -6, 20, 12, "#3b82f6");

    pRect(ctx, -7, -6, 14, 12, "#1e3a8a");
    pRect(ctx, -6, -5, 12, 10, "#1d4ed8");
    pRect(ctx, -4, -4, 8, 8, "#60a5fa");

    pRect(ctx, 5, -2, 13, 4, "#334155");
    pRect(ctx, 6, -1, 11, 2, "#94a3b8");
    pRect(ctx, 16, -3, 3, 6, "#09090b");
    pRect(ctx, 17, -2, 1, 4, "#cbd5e1");
  } else if (norm === "artillery") {
    // Long-range siege howitzer
    pRect(ctx, -16, -13, 32, 5, "#334155");
    pRect(ctx, -15, -12, 30, 3, "#64748b");
    pRect(ctx, -16, 8, 32, 5, "#334155");
    pRect(ctx, -15, 9, 30, 3, "#64748b");

    pRect(ctx, -14, -8, 28, 16, "#1d4ed8");
    pRect(ctx, -12, -7, 24, 14, "#2563eb");

    pRect(ctx, -17, -4, 4, 8, "#09090b");
    pRect(ctx, -16, -3, 2, 6, "#475569");

    pRect(ctx, -4, -4, 8, 8, "#1e3a8a");
    pRect(ctx, 0, -8, 18, 5, "#334155");
    pRect(ctx, 2, -7, 15, 3, "#94a3b8");
    pRect(ctx, 4, -6, 12, 1, "#cbd5e1");
    pRect(ctx, 16, -10, 3, 9, "#09090b");
    pRect(ctx, 17, -9, 1, 7, "#facc15");
  } else if (norm === "harvester") {
    // Legacy mining harvester (unused; no harvester unit exists in the roster)
    pRect(ctx, -16, -14, 32, 6, "#1e293b");
    pRect(ctx, -15, -13, 30, 4, "#334155");
    pRect(ctx, -14, -12, 28, 2, "#64748b");
    pRect(ctx, -16, 8, 32, 6, "#1e293b");
    pRect(ctx, -15, 9, 30, 4, "#334155");
    pRect(ctx, -14, 10, 28, 2, "#64748b");

    pRect(ctx, -14, -8, 28, 16, "#1e40af");
    pRect(ctx, -12, -7, 10, 14, "#2563eb");
    pRect(ctx, -10, -5, 6, 10, "#38bdf8");
    pRect(ctx, -8, -4, 2, 8, "#ffffff");

    pRect(ctx, 10, -10, 7, 20, "#475569");
    pRect(ctx, 12, -9, 4, 18, "#94a3b8");
    pRect(ctx, 14, -8, 2, 16, "#cbd5e1");
    pRect(ctx, 15, -6, 2, 4, "#facc15");
    pRect(ctx, 15, 2, 2, 4, "#facc15");

    pRect(ctx, -1, -5, 10, 10, "#78350f");
    pRect(ctx, 0, -4, 8, 8, "#d97706");
    pRect(ctx, 1, -3, 6, 6, "#f59e0b");
    pRect(ctx, 2, -2, 4, 4, "#fde047");
    pRect(ctx, 3, -1, 2, 2, "#ffffff");
  } else if (norm === "powerplant") {
    // Power Plant cooling towers & glowing fusion core
    pRect(ctx, -17, 12, 34, 4, "#eab308");
    pRect(ctx, -15, 12, 4, 4, "#09090b");
    pRect(ctx, -7, 12, 4, 4, "#09090b");
    pRect(ctx, 1, 12, 4, 4, "#09090b");
    pRect(ctx, 9, 12, 4, 4, "#09090b");

    // Left tower
    pRect(ctx, -16, -10, 11, 22, "#334155");
    pRect(ctx, -15, -9, 9, 20, "#475569");
    pRect(ctx, -14, -8, 7, 18, "#64748b");
    pRect(ctx, -13, -7, 3, 16, "#94a3b8");
    pRect(ctx, -17, -12, 13, 2, "#1e293b");
    pRect(ctx, -16, -11, 11, 1, "#334155");
    pRect(ctx, -14, -16, 7, 4, "#cbd5e1");
    pRect(ctx, -12, -18, 5, 2, "#ffffff");

    // Right tower
    pRect(ctx, 5, -10, 11, 22, "#334155");
    pRect(ctx, 6, -9, 9, 20, "#475569");
    pRect(ctx, 7, -8, 7, 18, "#64748b");
    pRect(ctx, 8, -7, 3, 16, "#94a3b8");
    pRect(ctx, 4, -12, 13, 2, "#1e293b");
    pRect(ctx, 5, -11, 11, 1, "#334155");
    pRect(ctx, 7, -16, 7, 4, "#cbd5e1");
    pRect(ctx, 9, -18, 5, 2, "#ffffff");

    // Center Fusion Core & Lightning Bolt
    pRect(ctx, -5, -4, 10, 16, "#0284c7");
    pRect(ctx, -4, -3, 8, 14, "#38bdf8");
    pRect(ctx, -3, -2, 6, 12, "#7dd3fc");
    pRect(ctx, 0, -5, 2, 7, "#fde047");
    pRect(ctx, -2, -1, 6, 2, "#facc15");
    pRect(ctx, -1, 1, 2, 7, "#fde047");
    pRect(ctx, 0, 0, 1, 1, "#ffffff");
  } else if (norm === "refinery") {
    // Refinery Silos & Ore Dumping Bay
    pRect(ctx, -17, 12, 34, 4, "#eab308");
    pRect(ctx, -15, 12, 4, 4, "#09090b");
    pRect(ctx, -7, 12, 4, 4, "#09090b");
    pRect(ctx, 1, 12, 4, 4, "#09090b");
    pRect(ctx, 9, 12, 4, 4, "#09090b");

    // Left tall metallic silo
    pRect(ctx, -16, -12, 12, 24, "#334155");
    pRect(ctx, -15, -11, 10, 22, "#475569");
    pRect(ctx, -14, -10, 8, 20, "#94a3b8");
    pRect(ctx, -13, -9, 3, 18, "#cbd5e1");
    pRect(ctx, -16, -14, 12, 2, "#1e293b");
    pRect(ctx, -14, -15, 8, 1, "#64748b");

    // Right furnace building
    pRect(ctx, -2, -6, 18, 18, "#1e293b");
    pRect(ctx, -1, -5, 16, 16, "#2563eb");
    pRect(ctx, 0, -4, 14, 14, "#1d4ed8");

    // Smokestack & flame
    pRect(ctx, 8, -14, 5, 8, "#334155");
    pRect(ctx, 9, -13, 3, 7, "#64748b");
    pRect(ctx, 8, -18, 5, 4, "#f97316");
    pRect(ctx, 9, -17, 3, 2, "#fde047");
    pRect(ctx, 10, -16, 1, 1, "#ffffff");

    // Golden Ore Nuggets
    pRect(ctx, 1, 4, 12, 7, "#09090b");
    pRect(ctx, 2, 5, 10, 5, "#b45309");
    pRect(ctx, 3, 5, 8, 4, "#f59e0b");
    pRect(ctx, 4, 6, 6, 2, "#fde047");
    pRect(ctx, 5, 6, 2, 2, "#ffffff");
  } else if (norm === "barracks") {
    // Hardened infantry fortress
    pRect(ctx, -17, 12, 34, 4, "#334155");
    pRect(ctx, -16, -10, 32, 22, "#1e3a8a");
    pRect(ctx, -15, -9, 30, 20, "#1d4ed8");
    pRect(ctx, -14, -8, 28, 4, "#3b82f6");

    pRect(ctx, -14, -18, 2, 8, "#94a3b8");
    pRect(ctx, -15, -19, 4, 2, "#ef4444");

    pRect(ctx, -6, 2, 12, 10, "#09090b");
    pRect(ctx, -5, 3, 10, 9, "#f59e0b");
    pRect(ctx, -4, 4, 8, 8, "#fef08a");

    pRect(ctx, -8, -3, 16, 3, "#facc15");
    pRect(ctx, -2, -6, 4, 9, "#facc15");
    pRect(ctx, -1, -5, 2, 7, "#ffffff");
  } else if (norm === "factory") {
    // Heavy dual-bay vehicle factory
    pRect(ctx, -17, 12, 34, 4, "#334155");
    pRect(ctx, -17, -11, 34, 23, "#1e293b");
    pRect(ctx, -16, -10, 32, 21, "#1e40af");
    pRect(ctx, -15, -9, 30, 4, "#3b82f6");

    pRect(ctx, -14, 10, 28, 2, "#eab308");
    pRect(ctx, -12, 10, 4, 2, "#09090b");
    pRect(ctx, -4, 10, 4, 2, "#09090b");
    pRect(ctx, 4, 10, 4, 2, "#09090b");
    pRect(ctx, 12, 10, 4, 2, "#09090b");

    pRect(ctx, -12, -2, 24, 12, "#09090b");
    pRect(ctx, -9, 3, 18, 6, "#2563eb");
    pRect(ctx, -7, 1, 14, 4, "#60a5fa");
    pRect(ctx, -2, -1, 4, 3, "#93c5fd");
    pRect(ctx, 2, 0, 7, 2, "#cbd5e1");

    pRect(ctx, -6, -5, 2, 6, "#eab308");
    pRect(ctx, -6, -1, 4, 2, "#38bdf8");
    pRect(ctx, -5, 0, 2, 2, "#ffffff");
  } else if (norm === "techlab") {
    // Quantum Research Facility & Radar Dome
    pRect(ctx, -17, 12, 34, 4, "#334155");
    pRect(ctx, -15, 0, 30, 12, "#1e293b");
    pRect(ctx, -14, 1, 28, 10, "#1e40af");

    pRect(ctx, -12, -12, 24, 13, "#0284c7");
    pRect(ctx, -11, -11, 22, 11, "#06b6d4");
    pRect(ctx, -10, -10, 20, 9, "#38bdf8");
    pRect(ctx, -8, -8, 16, 6, "#7dd3fc");
    pRect(ctx, -6, -7, 6, 4, "#ffffff");

    pRect(ctx, 4, -18, 3, 7, "#64748b");
    pRect(ctx, 2, -19, 8, 3, "#cbd5e1");
    pRect(ctx, 3, -18, 6, 1, "#ffffff");
    pRect(ctx, 5, -20, 2, 2, "#38bdf8");
  } else if (norm === "turret") {
    // Defense turret with laser diode
    pRect(ctx, -17, 11, 34, 5, "#eab308");
    pRect(ctx, -15, 11, 4, 5, "#09090b");
    pRect(ctx, -7, 11, 4, 5, "#09090b");
    pRect(ctx, 1, 11, 4, 5, "#09090b");
    pRect(ctx, 9, 11, 4, 5, "#09090b");

    pRect(ctx, -15, 0, 30, 11, "#1e293b");
    pRect(ctx, -14, 1, 28, 9, "#334155");
    pRect(ctx, -13, 2, 26, 7, "#475569");

    pRect(ctx, -11, -8, 22, 10, "#1e3a8a");
    pRect(ctx, -10, -7, 20, 8, "#2563eb");
    pRect(ctx, -8, -6, 16, 6, "#60a5fa");

    pRect(ctx, -6, -17, 3, 10, "#334155");
    pRect(ctx, -5, -16, 1, 9, "#94a3b8");
    pRect(ctx, -7, -18, 5, 2, "#09090b");

    pRect(ctx, 3, -17, 3, 10, "#334155");
    pRect(ctx, 4, -16, 1, 9, "#94a3b8");
    pRect(ctx, 2, -18, 5, 2, "#09090b");

    pRect(ctx, -1, -5, 3, 3, "#ef4444");
    pRect(ctx, 0, -4, 1, 1, "#ffffff");
  } else if (norm === "damage") {
    // Combat targeting crosshairs & explosive core
    pRect(ctx, -16, -1, 32, 2, "#ef4444");
    pRect(ctx, -1, -16, 2, 32, "#ef4444");
    pRect(ctx, -10, -10, 20, 20, "#991b1b");
    pRect(ctx, -8, -8, 16, 16, "#dc2626");
    pRect(ctx, -6, -6, 12, 12, "#ef4444");
    pRect(ctx, -3, -3, 6, 6, "#f87171");
    pRect(ctx, -1, -1, 2, 2, "#ffffff");
  } else if (norm === "hp") {
    // Medical shield & cross
    pRect(ctx, -14, -14, 28, 28, "#14532d");
    pRect(ctx, -12, -12, 24, 24, "#16a34a");
    pRect(ctx, -10, -10, 20, 20, "#22c55e");
    pRect(ctx, -4, -9, 8, 18, "#ffffff");
    pRect(ctx, -9, -4, 18, 8, "#ffffff");
    pRect(ctx, -2, -2, 4, 4, "#86efac");
  } else if (norm === "sell") {
    // Gleaming gold credit medallion
    pRect(ctx, -16, -16, 32, 32, "#78350f");
    pRect(ctx, -14, -14, 28, 28, "#b45309");
    pRect(ctx, -12, -12, 24, 24, "#d97706");
    pRect(ctx, -10, -10, 20, 20, "#f59e0b");

    // Bold Gold Dollar Sign
    pRect(ctx, -2, -9, 4, 18, "#fef08a");
    pRect(ctx, -7, -8, 13, 4, "#facc15");
    pRect(ctx, -7, -8, 4, 7, "#facc15");
    pRect(ctx, -7, -2, 14, 4, "#facc15");
    pRect(ctx, 2, 0, 4, 7, "#facc15");
    pRect(ctx, -7, 5, 14, 4, "#facc15");

    pRect(ctx, -4, -6, 2, 2, "#ffffff");
    pRect(ctx, 4, 3, 2, 2, "#ffffff");
    pRect(ctx, -11, -11, 3, 3, "#ffffff");
  } else if (norm === "repair") {
    // Polished heavy chrome spanner with welding plasma arcs
    pRect(ctx, -14, 8, 6, 6, "#1e293b");
    pRect(ctx, -13, 9, 4, 4, "#090d16");

    pRect(ctx, -11, 5, 6, 6, "#334155");
    pRect(ctx, -8, 2, 6, 6, "#475569");
    pRect(ctx, -5, -1, 6, 6, "#64748b");
    pRect(ctx, -2, -4, 6, 6, "#94a3b8");
    pRect(ctx, 1, -7, 6, 6, "#cbd5e1");
    pRect(ctx, 4, -10, 6, 6, "#ffffff");

    pRect(ctx, 4, -16, 12, 12, "#334155");
    pRect(ctx, 5, -15, 10, 10, "#cbd5e1");
    pRect(ctx, 8, -12, 6, 6, "#090d16");

    pRect(ctx, 11, -17, 4, 4, "#0284c7");
    pRect(ctx, 12, -18, 3, 3, "#38bdf8");
    pRect(ctx, 13, -19, 2, 2, "#ffffff");
    pRect(ctx, 8, -18, 2, 2, "#facc15");
    pRect(ctx, 15, -14, 2, 2, "#facc15");
  } else if (norm === "tab_buildings" || norm === "buildings" || norm === "structures") {
    // Fortified citadel tab icon
    pRect(ctx, -16, 10, 32, 4, "#eab308");
    pRect(ctx, -14, 10, 4, 4, "#09090b");
    pRect(ctx, -6, 10, 4, 4, "#09090b");
    pRect(ctx, 2, 10, 4, 4, "#09090b");
    pRect(ctx, 10, 10, 4, 4, "#09090b");

    pRect(ctx, -15, -8, 8, 18, "#334155");
    pRect(ctx, -14, -7, 6, 16, "#64748b");
    pRect(ctx, 7, -8, 8, 18, "#334155");
    pRect(ctx, 8, -7, 6, 16, "#64748b");

    pRect(ctx, -7, -12, 14, 22, "#1e293b");
    pRect(ctx, -6, -11, 12, 20, "#1d4ed8");
    pRect(ctx, -4, -9, 8, 14, "#38bdf8");
    pRect(ctx, -2, -7, 4, 4, "#ffffff");
    pRect(ctx, -2, -15, 4, 3, "#ef4444");
  } else if (norm === "tab_troops" || norm === "troops" || norm === "infantry_tab") {
    // Elite commando helmet tab icon
    pRect(ctx, -14, -12, 28, 22, "#1e293b");
    pRect(ctx, -13, -11, 26, 20, "#1d4ed8");
    pRect(ctx, -10, -10, 20, 5, "#60a5fa");

    pRect(ctx, -14, -18, 2, 8, "#94a3b8");
    pRect(ctx, -15, -19, 4, 2, "#ef4444");

    pRect(ctx, -11, -4, 22, 7, "#0284c7");
    pRect(ctx, -10, -3, 20, 5, "#38bdf8");
    pRect(ctx, -7, -3, 6, 3, "#ffffff");

    pRect(ctx, -8, 5, 16, 5, "#334155");
    pRect(ctx, -6, 6, 12, 3, "#090d16");
  } else if (norm === "tab_vehicles" || norm === "vehicles" || norm === "armor_tab") {
    // Heavy battle tank tab icon
    pRect(ctx, -16, -13, 32, 5, "#334155");
    pRect(ctx, -15, -12, 30, 3, "#64748b");
    pRect(ctx, -16, 8, 32, 5, "#334155");
    pRect(ctx, -15, 9, 30, 3, "#64748b");

    pRect(ctx, -14, -8, 28, 16, "#1d4ed8");
    pRect(ctx, -12, -7, 24, 14, "#2563eb");
    pRect(ctx, -10, -6, 20, 12, "#3b82f6");

    pRect(ctx, -6, -5, 12, 10, "#1e3a8a");
    pRect(ctx, -4, -4, 8, 8, "#60a5fa");
    pRect(ctx, 4, -2, 13, 4, "#334155");
    pRect(ctx, 5, -1, 11, 2, "#cbd5e1");
    pRect(ctx, 15, -3, 3, 6, "#09090b");
  } else if (norm === "airfield") {
    // Airfield landing pad & control tower
    pRect(ctx, -17, 12, 34, 4, "#eab308");
    pRect(ctx, -15, 12, 4, 4, "#09090b");
    pRect(ctx, -7, 12, 4, 4, "#09090b");
    pRect(ctx, 1, 12, 4, 4, "#09090b");
    pRect(ctx, 9, 12, 4, 4, "#09090b");

    // Tarmac landing slab
    pRect(ctx, -17, -10, 34, 22, "#1e293b");
    pRect(ctx, -16, -9, 32, 20, "#242d3a");

    // Helipad Landing Circle & [ H ]
    pRect(ctx, -13, -6, 16, 14, "#0f172a");
    pRect(ctx, -12, -5, 14, 12, "#1e293b");
    pRect(ctx, -10, -3, 2, 8, "#facc15");
    pRect(ctx, -4, -3, 2, 8, "#facc15");
    pRect(ctx, -10, 0, 8, 2, "#facc15");

    // Air traffic control radar tower on right
    pRect(ctx, 5, -8, 10, 16, "#1e293b");
    pRect(ctx, 6, -7, 8, 14, "#334155");
    pRect(ctx, 7, -6, 6, 5, "#38bdf8");
    pRect(ctx, 9, -15, 2, 7, "#64748b");
    pRect(ctx, 6, -17, 8, 3, "#cbd5e1");
    pRect(ctx, 9, -19, 2, 2, "#ef4444");

    // Runway approach lights
    pRect(ctx, -15, -8, 2, 2, "#22c55e");
    pRect(ctx, -15, 6, 2, 2, "#22c55e");
  } else if (norm === "radar") {
    // Long-range dish with a bright sweep
    pRect(ctx, -14, -14, 28, 28, "#1e293b");
    pRect(ctx, -12, -12, 24, 24, "#0f172a");
    pRect(ctx, -10, -10, 20, 20, "#0b1c2e");
    // Sweep wedge (fixed diagonal for the icon)
    pRect(ctx, -8, -8, 16, 1, "#22d3ee");
    pRect(ctx, -7, -7, 1, 15, "#22d3ee");
    pRect(ctx, 2, -2, 6, 1, "#38bdf8");
    // Blip
    pRect(ctx, 8, -3, 3, 3, "#fbbf24");
    // Pivot + mast
    pRect(ctx, -2, -2, 4, 4, "#cbd5e1");
    pRect(ctx, -1, -1, 2, 2, "#ffffff");
    pRect(ctx, -1, -13, 2, 5, "#64748b");
  } else if (norm === "teslacoil") {
    // High-voltage coil with arcs
    pRect(ctx, -12, -18, 24, 34, "#78350f");
    pRect(ctx, -10, -16, 20, 30, "#d97706");
    pRect(ctx, -8, -14, 16, 4, "#f59e0b");
    pRect(ctx, -8, -6, 16, 4, "#f59e0b");
    pRect(ctx, -8, 2, 16, 4, "#f59e0b");
    pRect(ctx, -8, 10, 16, 4, "#f59e0b");
    pRect(ctx, -5, -20, 10, 8, "#1e3a8a");
    pRect(ctx, -3, -19, 6, 6, "#38bdf8");
    pRect(ctx, -1, -17, 2, 2, "#ffffff");
    // Arcs
    pRect(ctx, 10, -16, 8, 2, "#7dd3fc");
    pRect(ctx, 14, -14, 2, 6, "#7dd3fc");
    pRect(ctx, -16, 2, 6, 2, "#7dd3fc");
    pRect(ctx, -14, 4, 2, 5, "#7dd3fc");
  } else if (norm === "mammothtank") {
    // Twin-barrel heavy siege tank
    pRect(ctx, -16, -12, 32, 24, "#1e293b");
    pRect(ctx, -15, -11, 30, 22, "#1d4ed8");
    pRect(ctx, -13, -9, 26, 18, "#2563eb");
    // Twin barrels
    pRect(ctx, -2, -12, 20, 4, "#334155");
    pRect(ctx, -1, -11, 18, 2, "#94a3b8");
    pRect(ctx, -2, 8, 20, 4, "#334155");
    pRect(ctx, -1, 9, 18, 2, "#94a3b8");
    pRect(ctx, 17, -13, 3, 6, "#09090b");
    pRect(ctx, 17, 7, 3, 6, "#09090b");
    // Turret hatches
    pRect(ctx, -4, -4, 8, 8, "#1e3a8a");
    pRect(ctx, -3, -3, 6, 6, "#60a5fa");
    pRect(ctx, -1, -7, 2, 4, "#94a3b8");
    pRect(ctx, -1, -9, 4, 2, "#ef4444");
  } else if (norm === "range") {
    // Range research: expanding concentric targeting rings
    pRect(ctx, -13, -13, 26, 26, "#0c4a6e");
    pRect(ctx, -11, -11, 22, 22, "#075985");
    pRect(ctx, -9, -9, 18, 18, "#0e7490");
    pRect(ctx, -6, -6, 12, 12, "#155e75");
    pRect(ctx, -3, -3, 6, 6, "#164e63");
    pRect(ctx, -1, -1, 2, 2, "#22d3ee");
    // Crosshair ticks (radar-style ranging)
    pRect(ctx, -13, -1, 4, 2, "#38bdf8");
    pRect(ctx, 9, -1, 4, 2, "#38bdf8");
    pRect(ctx, -1, -13, 2, 4, "#38bdf8");
    pRect(ctx, -1, 9, 2, 4, "#38bdf8");
  } else if (norm === "gunship") {
    // Heavy Attack Helicopter / Rotary Gunship (Vivid & Distinct from Jet Fighter)
    // Spinning top rotor blade disc (spinning blurred rotor line)
    pRect(ctx, -18, -14, 36, 2, "#94a3b8");
    pRect(ctx, -14, -15, 28, 1, "#cbd5e1");
    pRect(ctx, -1, -16, 2, 5, "#475569");
    pRect(ctx, -2, -17, 4, 2, "#facc15");

    // Heavy armored helicopter fuselage
    pRect(ctx, -7, -10, 14, 20, "#1e3a8a");
    pRect(ctx, -6, -9, 12, 18, "#1d4ed8");
    pRect(ctx, -4, -7, 8, 14, "#2563eb");

    // Armored Cockpit Canopy with green tactical HUD glass
    pRect(ctx, -4, -6, 8, 6, "#15803d");
    pRect(ctx, -3, -5, 6, 4, "#22c55e");
    pRect(ctx, -2, -5, 2, 2, "#86efac");

    // Chin-Mounted 3-Barrel Rotary Vulcan Minigun (at bottom)
    pRect(ctx, -2, 10, 4, 6, "#09090b");
    pRect(ctx, -1, 12, 2, 5, "#64748b");
    pRect(ctx, -2, 16, 4, 2, "#facc15");

    // Side Weapon Pylons with Heavy Rocket Pods (Left & Right)
    pRect(ctx, -14, -2, 7, 3, "#334155");
    pRect(ctx, -15, -4, 4, 8, "#1e293b");
    pRect(ctx, -15, -4, 4, 2, "#ea580c");
    pRect(ctx, -15, 2, 4, 2, "#ea580c");

    pRect(ctx, 7, -2, 7, 3, "#334155");
    pRect(ctx, 11, -4, 4, 8, "#1e293b");
    pRect(ctx, 11, -4, 4, 2, "#ea580c");
    pRect(ctx, 11, 2, 4, 2, "#ea580c");

    // Tail boom extending upwards with fenestron tail rotor
    pRect(ctx, -2, -12, 4, 5, "#334155");
    pRect(ctx, -6, -11, 4, 3, "#eab308");
  } else if (norm === "tab_aircraft" || norm === "aircraft" || norm === "air_tab" || norm === "interceptor") {
    // Supersonic Stealth Jet Fighter (Delta Wings, Sharp Needle Radome, Plasma Afterburners)
    // Swept delta wings
    pRect(ctx, -18, 0, 36, 4, "#1e3a8a");
    pRect(ctx, -16, -3, 32, 4, "#1d4ed8");
    pRect(ctx, -13, -6, 26, 4, "#2563eb");
    pRect(ctx, -10, 4, 20, 4, "#1e40af");

    // Wingtip missiles / laser pods
    pRect(ctx, -18, -8, 2, 14, "#38bdf8");
    pRect(ctx, 16, -8, 2, 14, "#38bdf8");

    // Twin canted vertical stabilizers / rudders
    pRect(ctx, -7, 2, 3, 7, "#60a5fa");
    pRect(ctx, 4, 2, 3, 7, "#60a5fa");

    // Needle fuselage & Sharp Radar Nose Radome
    pRect(ctx, -4, -14, 8, 22, "#334155");
    pRect(ctx, -3, -16, 6, 24, "#475569");
    pRect(ctx, -2, -18, 4, 26, "#94a3b8");
    pRect(ctx, -1, -20, 2, 4, "#cbd5e1");

    // Holographic Cyan Pilot Cockpit Canopy
    pRect(ctx, -2, -10, 4, 8, "#0284c7");
    pRect(ctx, -1, -9, 2, 6, "#38bdf8");
    pRect(ctx, 0, -8, 1, 3, "#ffffff");

    // Twin High-Thrust Flaming Orange/Yellow Plasma Rocket Afterburners
    pRect(ctx, -5, 8, 3, 7, "#ea580c");
    pRect(ctx, 2, 8, 3, 7, "#ea580c");
    pRect(ctx, -4, 10, 2, 6, "#facc15");
    pRect(ctx, 3, 10, 2, 6, "#facc15");
    pRect(ctx, -4, 14, 2, 3, "#ffffff");
    pRect(ctx, 3, 14, 2, 3, "#ffffff");
  } else if (norm === "scout") {
    // Light recon buggy
    pRect(ctx, -16, -13, 32, 6, "#1e293b");
    pRect(ctx, -15, -12, 30, 4, "#334155");
    pRect(ctx, -16, 7, 32, 6, "#1e293b");
    pRect(ctx, -15, 8, 30, 4, "#334155");
    pRect(ctx, -15, -7, 30, 14, "#1d4ed8");
    pRect(ctx, -13, -6, 26, 12, "#2563eb");
    pRect(ctx, -11, -5, 22, 10, "#3b82f6");
    // Roll cage
    pRect(ctx, -9, -12, 4, 8, "#64748b");
    pRect(ctx, 5, -12, 4, 8, "#64748b");
    pRect(ctx, -9, -13, 18, 3, "#64748b");
    // Machine gun
    pRect(ctx, 6, -15, 9, 3, "#334155");
    pRect(ctx, 8, -14, 7, 2, "#94a3b8");
    // Whip antenna
    pRect(ctx, -14, -18, 2, 7, "#94a3b8");
    pRect(ctx, -15, -19, 4, 2, "#38bdf8");
  } else if (norm === "rockettrooper") {
    // Infantry with shoulder rocket tube
    pRect(ctx, -9, -1, 18, 16, "#1e3a8a");
    pRect(ctx, -8, 0, 16, 14, "#1d4ed8");
    pRect(ctx, -6, 2, 12, 10, "#2563eb");
    pRect(ctx, -6, 6, 4, 5, "#334155");
    pRect(ctx, 2, 6, 4, 5, "#334155");
    pRect(ctx, -9, -14, 18, 12, "#1e293b");
    pRect(ctx, -8, -13, 16, 10, "#1d4ed8");
    pRect(ctx, -7, -6, 14, 5, "#0284c7");
    // Rocket tube on shoulder
    pRect(ctx, 3, -12, 14, 5, "#334155");
    pRect(ctx, 5, -11, 11, 3, "#64748b");
    pRect(ctx, 14, -13, 4, 7, "#09090b");
    pRect(ctx, 16, -12, 2, 5, "#facc15");
  } else if (norm === "samlauncher") {
    // Wheeled AA missile truck
    pRect(ctx, -16, 6, 32, 6, "#1e293b");
    pRect(ctx, -15, 7, 30, 4, "#334155");
    pRect(ctx, -16, -1, 32, 7, "#1d4ed8");
    pRect(ctx, -14, 0, 28, 5, "#2563eb");
    // Missile pod
    pRect(ctx, -2, -16, 9, 15, "#334155");
    pRect(ctx, -1, -15, 7, 13, "#64748b");
    pRect(ctx, 0, -14, 5, 2, "#1f2937");
    pRect(ctx, 0, -8, 5, 2, "#1f2937");
    // Cab + dish
    pRect(ctx, -16, -8, 7, 7, "#475569");
    pRect(ctx, -18, -11, 11, 3, "#94a3b8");
  } else if (norm === "crystalrefinery") {
    // Refinery with cyan crystal vat
    pRect(ctx, -17, -10, 8, 20, "#1e293b");
    pRect(ctx, -16, -9, 6, 18, "#334155");
    pRect(ctx, -8, -10, 8, 20, "#1e293b");
    pRect(ctx, -7, -9, 6, 18, "#334155");
    pRect(ctx, -16, 4, 32, 9, "#1d4ed8");
    pRect(ctx, -14, 5, 28, 7, "#2563eb");
    // Crystal vat
    pRect(ctx, -8, -2, 16, 8, "#164e63");
    pRect(ctx, -6, -1, 12, 6, "#0891b2");
    pRect(ctx, -4, 0, 8, 4, "#67e8f9");
    pRect(ctx, -1, -3, 3, 4, "#a5f3fc");
    pRect(ctx, -2, -4, 4, 2, "#cffafe");
  } else if (norm === "aaturret") {
    // Point-defense turret with missile pods
    pRect(ctx, -16, 4, 32, 8, "#1e293b");
    pRect(ctx, -15, 5, 30, 6, "#334155");
    pRect(ctx, -9, -6, 18, 10, "#1d4ed8");
    pRect(ctx, -8, -5, 16, 8, "#2563eb");
    // Twin pods
    pRect(ctx, -8, -14, 16, 4, "#334155");
    pRect(ctx, -7, -13, 14, 2, "#64748b");
    pRect(ctx, -8, 2, 16, 4, "#334155");
    pRect(ctx, -7, 3, 14, 2, "#64748b");
    // Sensor dome
    pRect(ctx, -4, -4, 8, 8, "#0e7490");
    pRect(ctx, -3, -3, 6, 6, "#67e8f9");
  } else if (norm === "play") {
    pRect(ctx, -6, -10, 12, 20, color);
  } else if (norm === "pause") {
    pRect(ctx, -8, -10, 6, 20, color);
    pRect(ctx, 2, -10, 6, 20, color);
  } else if (norm === "fast_forward") {
    pRect(ctx, -10, -10, 8, 20, color);
    pRect(ctx, 2, -10, 8, 20, color);
  } else if (norm === "cross") {
    pRect(ctx, -8, -8, 16, 16, "#ef4444");
  }

  ctx.restore();
}

const thumbnailCache = new Map<string, string>();

// ---------------------------------------------------------------------------
// v2 roster sprites: recon scout, rocket trooper, SAM launcher, crystal
// refinery, and the anti-air turret (all single-tile, tile-proportional).
// ---------------------------------------------------------------------------

/** Scout: Lightweight recon buggy — fast, fragile, wide eyes (+X is forward). */
function drawScout(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  _tick: number,
  firingAge: number = -1,
): void {
  const s = Math.max(3, Math.floor(z * 0.38));

  pRect(ctx, -s * 0.7, -s * 0.4 + 2, s * 1.4, s * 0.8, "rgba(0, 0, 0, 0.55)");

  // Open wheels
  pRect(ctx, -s * 0.6, -s * 0.35, s * 0.26, s * 0.5, "#09090b");
  pRect(ctx, -s * 0.58, -s * 0.33, s * 0.2, s * 0.44, "#3f3f46");
  pRect(ctx, s * 0.34, -s * 0.35, s * 0.26, s * 0.5, "#09090b");
  pRect(ctx, s * 0.36, -s * 0.33, s * 0.2, s * 0.44, "#3f3f46");

  // Low chassis
  pRect(ctx, -s * 0.5, -s * 0.28, s * 1.0, s * 0.36, "#27272a");
  pRect(ctx, -s * 0.42, -s * 0.23, s * 0.84, s * 0.26, pal.primaryDark);
  pRect(ctx, -s * 0.4, -s * 0.2, s * 0.8, s * 0.2, pal.primary);

  // Roll cage
  pRect(ctx, -s * 0.2, -s * 0.6, s * 0.12, s * 0.36, "#52525b");
  pRect(ctx, s * 0.22, -s * 0.6, s * 0.12, s * 0.36, "#52525b");
  pRect(ctx, -s * 0.22, -s * 0.62, s * 0.56, s * 0.09, "#52525b");

  // Gunner + pintle machine gun
  pRect(ctx, -s * 0.04, -s * 0.55, s * 0.2, s * 0.3, pal.primary);
  pRect(ctx, s * 0.16, -s * 0.62, s * 0.55, s * 0.09, "#09090b");
  if (firingAge === 0 || firingAge === 1) {
    pRect(ctx, s * 0.7, -s * 0.64, 4, 3, "#fef08a");
    pRect(ctx, s * 0.75, -s * 0.62, 2, 2, "#ffffff");
  }

  // Whip antenna with blinking tip
  pRect(ctx, -s * 0.55, -s * 0.9, 1.5, s * 0.38, "#94a3b8");
  if (Math.floor(performance.now() / 300) % 2 === 0) {
    pRect(ctx, -s * 0.55, -s * 0.95, 3, 1.5, pal.accent);
  }
}

/** RocketTrooper: Infantry with a shoulder-fired anti-armor/AA rocket tube. */
function drawRocketTrooper(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  firingAge: number = -1,
): void {
  const s = Math.max(3, Math.floor(z * 0.36));

  pRect(ctx, -s * 0.4, -s * 0.25 + 2, s * 0.8, s * 0.5, "rgba(0, 0, 0, 0.55)");

  // Combat boots
  pRect(ctx, -s * 0.45, -s * 0.35, s * 0.3, s * 0.2, "#09090b");
  pRect(ctx, -s * 0.45, s * 0.15, s * 0.3, s * 0.2, "#09090b");

  // Camo fatigue torso
  pRect(ctx, -s * 0.35, -s * 0.3, s * 0.65, s * 0.6, "#27272a");
  pRect(ctx, -s * 0.25, -s * 0.25, s * 0.45, s * 0.5, pal.primary);

  // Shoulder rocket launcher tube (angled up, +X is forward)
  const kick = firingAge === 0 ? -2 : 0;
  pRect(ctx, s * 0.05 + kick, -s * 0.62, s * 0.75, s * 0.22, "#09090b");
  pRect(ctx, s * 0.07 + kick, -s * 0.59, s * 0.7, s * 0.16, "#3f3f46");
  pRect(ctx, s * 0.72 + kick, -s * 0.66, s * 0.2, s * 0.3, "#18181b");
  pRect(ctx, s * 0.78 + kick, -s * 0.62, 2, s * 0.22, "#facc15");

  // Backblast + muzzle flash on fire
  if (firingAge >= 0 && firingAge <= 2) {
    pRect(ctx, s * 0.05 + kick, -s * 0.68, 5, 3, "#f97316");
    pRect(ctx, s * 0.9 + kick, -s * 0.64, 4, 3, "#fef08a");
    pRect(ctx, s * 0.94 + kick, -s * 0.62, 2, 2, "#ffffff");
  }

  // Helmet / head
  const headW = Math.max(6, Math.floor(s * 0.5));
  pRect(ctx, -s * 0.25, -headW / 2, headW, headW, "#18181b");
  pRect(ctx, -s * 0.15, -headW / 2 + 1, headW - 2, headW - 2, pal.primaryDark);
  pRect(ctx, s * 0.05, -headW * 0.3, 2, headW * 0.6, pal.accent);
}

/** SamLauncher: Wheeled surface-to-air missile launcher (+X is forward). */
function drawSamLauncher(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  firingAge: number = -1,
): void {
  const s = Math.max(3, Math.floor(z * 0.4));

  pRect(ctx, -s * 0.8, -s * 0.5 + 2, s * 1.6, s * 1.0, "rgba(0, 0, 0, 0.55)");

  // Three wheels
  pRect(ctx, -s * 0.72, -s * 0.2, s * 0.2, s * 0.42, "#09090b");
  pRect(ctx, -s * 0.22, -s * 0.2, s * 0.2, s * 0.42, "#09090b");
  pRect(ctx, s * 0.52, -s * 0.2, s * 0.2, s * 0.42, "#09090b");

  // Truck bed chassis
  pRect(ctx, -s * 0.7, -s * 0.38, s * 1.4, s * 0.32, "#27272a");
  pRect(ctx, -s * 0.62, -s * 0.33, s * 1.24, s * 0.22, pal.primaryDark);
  pRect(ctx, -s * 0.6, -s * 0.3, s * 1.2, s * 0.16, pal.primary);

  // Angled missile pod (three tubes)
  const podH = Math.floor(s * 0.8);
  pRect(ctx, s * 0.02, -s * 0.9, s * 0.52, podH, "#09090b");
  pRect(ctx, s * 0.09, -s * 0.83, s * 0.38, podH - 0.5, "#3f3f46");
  for (let i = 0; i < 3; i++) {
    pRect(ctx, s * 0.13, -s * 0.83 + i * (podH / 3), s * 0.3, 1.5, "#1f2937");
  }

  // Launch flash
  if (firingAge >= 0 && firingAge <= 2) {
    pRect(ctx, s * 0.52, -s * 0.9, 6, 5, "#fef08a");
    pRect(ctx, s * 0.55, -s * 0.86, 3, 3, "#ffffff");
  }

  // Cab + tracking dish
  pRect(ctx, -s * 0.6, -s * 0.72, s * 0.3, s * 0.3, "#334155");
  pRect(ctx, -s * 0.68, -s * 0.88, s * 0.46, s * 0.12, "#94a3b8");
}

/** CrystalRefinery: Refinery built on a cyan crystal field. */
function drawCrystalRefinery(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  tick: number,
): void {
  const r = Math.max(3, Math.floor(z * 0.46));

  // Shadow + concrete foundation
  pRect(ctx, -r + 5, -r + 7, r * 2 + 5, r * 2 + 3, "rgba(0, 0, 0, 0.6)");
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#0f172a");

  // Twin cryo-cooling stacks (North-West), teal-tipped
  const stackW = Math.max(6, Math.floor(r * 0.34));
  const stackH = Math.floor(r * 1.0);
  const stackY = -r * 1.02;
  for (const sx of [-r * 0.88, -r * 0.42]) {
    pRect(ctx, sx, stackY, stackW, stackH, "#090d16");
    pRect(ctx, sx + 1, stackY + 1, stackW - 2, stackH - 2, "#1e293b");
    pRect(ctx, sx + 1, stackY + 1, 1.5, stackH - 2, "#334155");
    pRect(ctx, sx, stackY + stackH * 0.35, stackW, 2, "#090d16");
    pRect(ctx, sx, stackY + stackH * 0.7, stackW, 2, "#090d16");
    pRect(ctx, sx + 1, stackY - 3, stackW - 2, 3, "#0e7490");
  }

  // Cold vapor puffs
  const puff = Math.floor((tick * 0.25) % 4);
  pRect(ctx, -r * 0.88 + puff - 1, stackY - puff * 3 - 4, 5 + puff, 4 + puff, "#67e8f9");
  pRect(ctx, -r * 0.42 + puff - 1, stackY - puff * 3 - 6, 4 + puff, 3 + puff, "#a5f3fc");

  // Crystal storage silo (North-East)
  const siloW = Math.floor(r * 0.7);
  const siloH = Math.floor(r * 0.9);
  pRect(ctx, r * 0.15, -r * 0.9, siloW, siloH, "#090d16");
  pRect(ctx, r * 0.15 + 1, -r * 0.9 + 1, siloW - 2, siloH - 2, pal.primary);
  pRect(ctx, r * 0.15 + 2, -r * 0.9 + 1, 1.5, siloH - 2, pal.primaryLight);
  pRect(ctx, r * 0.15 + siloW - 4, -r * 0.9 + 3, 2, 2, "#22d3ee");
  pRect(ctx, r * 0.15 + siloW - 4, -r * 0.9 + 7, 2, 2, "#22d3ee");
  pRect(ctx, r * 0.15 + siloW - 4, -r * 0.9 + 11, 2, 2, "#0e7490");

  // Central glowing crystal processing vat
  const vatW = Math.floor(r * 1.2);
  const vatH = Math.floor(r * 0.52);
  pRect(ctx, -vatW / 2, -r * 0.1, vatW, vatH, "#090d16");
  pRect(ctx, -vatW / 2 + 1, -r * 0.1 + 1, vatW - 2, vatH - 2, "#155e75");
  pRect(ctx, -vatW / 2 + 2, -r * 0.1 + 2, vatW - 4, vatH - 4, "#0891b2");
  pRect(ctx, -vatW / 2 + 4, -r * 0.1 + 4, vatW - 8, vatH - 8, "#67e8f9");

  // Rising shard pixel
  const bubble = Math.sin(tick * 0.3) > 0 ? 1 : -1;
  pRect(ctx, -2 + bubble * 5, -r * 0.1 + 4, 3, 3, "#cffafe");

  // Unloading dock with hazard ramp
  const dockW = Math.floor(r * 1.35);
  const dockH = Math.floor(r * 0.6);
  pRect(ctx, -dockW / 2, r * 0.35, dockW, dockH, "#090d16");
  pRect(ctx, -dockW / 2 + 1, r * 0.35 + 1, dockW - 2, dockH - 2, pal.primaryDark);
  pRect(ctx, -dockW * 0.35, r * 0.38, dockW * 0.7, dockH * 0.55, "#164e63");
  pHazard(ctx, -dockW / 2 + 2, r * 0.72, dockW - 4, 3);
}

/** AATurret: Point-defense turret with twin anti-air missile pods. */
function drawAATurret(
  ctx: CanvasRenderingContext2D,
  z: number,
  pal: TeamPalette,
  heading: number = 0,
  tick: number = 0,
  firingAge: number = -1,
): void {
  const r = Math.max(3, Math.floor(z * 0.44));

  // Shadow + circular concrete barbette
  pRect(ctx, -r + 4, -r + 5, r * 2 + 3, r * 2 + 2, "rgba(0, 0, 0, 0.6)");
  pRect(ctx, -r, -r, r * 2, r * 2, "#090d16");
  pRect(ctx, -r + 1, -r + 1, r * 2 - 2, r * 2 - 2, "#1e293b");
  const boltR = r - 3;
  const boltPositions = [
    [-boltR, -boltR], [0, -boltR - 1], [boltR, -boltR],
    [-boltR - 1, 0], [boltR + 1, 0],
    [-boltR, boltR], [0, boltR + 1], [boltR, boltR],
  ];
  for (const [bx, by] of boltPositions) {
    pRect(ctx, bx, by, 2, 2, "#64748b");
  }

  // Rotating turret with twin missile pods (aimed by heading)
  ctx.save();
  ctx.rotate(heading);

  const podLen = Math.floor(r * 1.35);
  const podW = Math.max(3, Math.floor(r * 0.3));
  const spacing = Math.floor(r * 0.52);

  let recoilLeft = 0;
  let recoilRight = 0;
  if (firingAge >= 0 && firingAge <= 3) {
    const rAmt = (3 - firingAge) * 1.5;
    if (tick % 2 === 0) recoilLeft = rAmt;
    else recoilRight = rAmt;
  }

  // Left pod
  pRect(ctx, -recoilLeft, -spacing - podW / 2, podLen, podW, "#09090b");
  pRect(ctx, -recoilLeft + 1, -spacing - podW / 2 + 0.5, podLen - 2, podW - 1, "#475569");
  pRect(ctx, podLen - recoilLeft - 3, -spacing - podW / 2 - 1, 3, podW + 2, "#181f2a");

  // Right pod
  pRect(ctx, -recoilRight, spacing - podW / 2, podLen, podW, "#09090b");
  pRect(ctx, -recoilRight + 1, spacing - podW / 2 + 0.5, podLen - 2, podW - 1, "#475569");
  pRect(ctx, podLen - recoilRight - 3, spacing - podW / 2 - 1, 3, podW + 2, "#181f2a");

  // Launch flashes
  if (firingAge >= 0 && firingAge <= 1) {
    const flashSize = Math.floor(r * 0.45);
    if (recoilLeft > 0) {
      pRect(ctx, podLen - recoilLeft, -spacing - flashSize / 2, flashSize * 1.2, flashSize, "#fef08a");
      pRect(ctx, podLen - recoilLeft + 1, -spacing - flashSize / 4, flashSize * 0.7, flashSize / 2, "#ffffff");
    }
    if (recoilRight > 0) {
      pRect(ctx, podLen - recoilRight, spacing - flashSize / 2, flashSize * 1.2, flashSize, "#fef08a");
      pRect(ctx, podLen - recoilRight + 1, spacing - flashSize / 4, flashSize * 0.7, flashSize / 2, "#ffffff");
    }
  }

  // Beveled cupola with sensor dome
  const cupW = Math.floor(r * 0.95);
  pRect(ctx, -cupW / 2, -cupW / 2, cupW, cupW, "#09090b");
  pRect(ctx, -cupW / 2 + 1, -cupW / 2 + 1, cupW - 2, cupW - 2, pal.primaryDark);
  pRect(ctx, -cupW / 2 + 2, -cupW / 2 + 2, cupW - 4, cupW - 4, pal.primary);
  pRect(ctx, -3, -3, 6, 6, "#0e7490");
  pRect(ctx, -2, -2, 4, 4, "#67e8f9");
  ctx.restore();
}

/** Kinds with a dedicated SVG icon in the in-repo asset pack; these are
 *  served as crisp `<img>` sources instead of canvas thumbnails (A2). */
const SVG_THUMBNAIL_KINDS = new Set([
  // Units
  "Infantry", "Scout", "RocketTrooper", "Tank", "Artillery", "MammothTank",
  "Gunship", "Interceptor", "SamLauncher",
  // Buildings
  "Hq", "PowerPlant", "Refinery", "CrystalRefinery", "Barracks", "Factory",
  "TechLab", "Airfield", "Radar", "TeslaCoil", "Turret", "AATurret",
]);

function unitAssetPath(kind: string): string {
  return `/assets/units/${kind.toLowerCase()}.svg`;
}

export function getThumbnailDataUrl(kind: string, _owner: number = 0): string {
  const cached = thumbnailCache.get(kind);
  if (cached) return cached;

  if (SVG_THUMBNAIL_KINDS.has(kind)) {
    thumbnailCache.set(kind, unitAssetPath(kind));
    return unitAssetPath(kind);
  }

  if (typeof document === "undefined") {
    const fallback = `data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48"><rect width="48" height="48" fill="%2318181b"/></svg>`;
    thumbnailCache.set(kind, fallback);
    return fallback;
  }

  const c = document.createElement("canvas");
  c.width = 48;
  c.height = 48;
  const ctx = c.getContext("2d");
  if (ctx) {
    drawTacticalIcon(ctx, kind, 24, 24, 46);
  }
  const url = c.toDataURL();
  thumbnailCache.set(kind, url);
  return url;
}

const cursorCache = new Map<string, string>();

/**
 * A C&C-style cursor: the tactical symbol (no frame) rasterized to a 32px
 * data-URL with the hotspot at the center. Kinds map to the icon art
 * (`sell` = $ medallion, `repair` = wrench, `attack` = targeting reticle).
 */
export function getCursorDataUrl(kind: string): string {
  const cached = cursorCache.get(kind);
  if (cached) return cached;

  if (typeof document === "undefined") {
    cursorCache.set(kind, "default");
    return "default";
  }

  // The `attack` cursor reuses the red targeting-reticle icon art.
  const icon = kind === "attack" ? "damage" : kind;
  const c = document.createElement("canvas");
  c.width = 32;
  c.height = 32;
  const ctx = c.getContext("2d");
  if (ctx) {
    drawTacticalIcon(ctx, icon, 16, 16, 32, "#f59e0b", false);
  }
  const url = `url(${c.toDataURL()}) 16 16, auto`;
  cursorCache.set(kind, url);
  return url;
}

export function drawSelectionReticle(
  ctx: CanvasRenderingContext2D,
  px: number,
  py: number,
  size: number,
  _tick: number,
): void {
  const r = Math.floor(size * 0.6);
  const len = Math.max(4, Math.floor(r * 0.35));

  ctx.strokeStyle = "#ffe27a";
  ctx.lineWidth = 1.5;

  ctx.beginPath();
  // Top-left corner
  ctx.moveTo(px - r, py - r + len);
  ctx.lineTo(px - r, py - r);
  ctx.lineTo(px - r + len, py - r);
  // Top-right corner
  ctx.moveTo(px + r - len, py - r);
  ctx.lineTo(px + r, py - r);
  ctx.lineTo(px + r, py - r + len);
  // Bottom-left corner
  ctx.moveTo(px - r, py + r - len);
  ctx.lineTo(px - r, py + r);
  ctx.lineTo(px - r + len, py + r);
  // Bottom-right corner
  ctx.moveTo(px + r - len, py + r);
  ctx.lineTo(px + r, py + r);
  ctx.lineTo(px + r, py + r - len);
  ctx.stroke();
}

export function drawHealthBar(
  ctx: CanvasRenderingContext2D,
  px: number,
  py: number,
  size: number,
  hp: number,
  maxHp: number,
): void {
  const w = Math.floor(size * 0.85);
  const h = 3.5;
  const x = Math.floor(px - w / 2);
  const y = Math.floor(py - size * 0.6 - 5);

  pRect(ctx, x - 1, y - 1, w + 2, h + 2, "#09090b");
  const pct = Math.max(0, Math.min(1, hp / Math.max(1, maxHp)));
  const barCol = pct > 0.5 ? "#22c55e" : pct > 0.25 ? "#f59e0b" : "#ef4444";
  pRect(ctx, x, y, Math.floor(w * pct), h, barCol);
}


