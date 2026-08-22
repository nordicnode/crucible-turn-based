// Standalone terrain preview: renders the real sprites.ts tile functions at
// several zoom levels so the visuals can be inspected without a live match.
import {
  drawPassableTile,
  drawForestTile,
  drawHillsTile,
  drawDesertTile,
  drawSwampTile,
  drawWaterTile,
  drawRiverTile,
  drawMountainTile,
  drawResourceDeposit,
  drawCrystalDeposit,
  drawOreDeposit,
} from "./src/sprites";

const canvas = document.createElement("canvas");
document.body.appendChild(canvas);
const ctx = canvas.getContext("2d")!;
ctx.imageSmoothingEnabled = false;

const kinds: Array<[string, (c: CanvasRenderingContext2D, tx: number, ty: number, px: number, py: number, size: number, sil: boolean) => void]> = [
  ["PLAINS", drawPassableTile],
  ["FOREST", drawForestTile],
  ["HILLS", drawHillsTile],
  ["DESERT", drawDesertTile],
  ["SWAMP", drawSwampTile],
  ["WATER", drawWaterTile],
  ["RIVER", drawRiverTile],
  ["MOUNTAIN", drawMountainTile],
];

// Row 1: large detail tiles (zoom 96)
// Row 2: tactical zoom (zoom 20), 4 variants each to show per-tile variation
// Row 3: resource deposits on top of their natural terrain
const big = 96;
const small = 20;
const gap = 12;
const cols = 4;
const rows = 4;
const r1h = big + 30;
const r2h = small + 30;
const r3h = big + 30;
canvas.width = cols * (big + gap) + gap;
canvas.height = r1h + r2h + r3h + 160;

let x = gap;
let y = 8;
for (let i = 0; i < kinds.length; i++) {
  const [name, fn] = kinds[i];
  fn(ctx, i, i, x, y, big, false);
  ctx.fillStyle = "#e7ca8a";
  ctx.font = "bold 12px monospace";
  ctx.fillText(name, x + 4, y + big + 16);
  x += big + gap;
  if (i === 3) {
    x = gap;
    y += r1h;
  }
}

// Row 2: small tactical tiles with per-tile hash variation.
x = gap;
y += 20;
for (let i = 0; i < kinds.length; i++) {
  const [, fn] = kinds[i];
  for (let v = 0; v < 4; v++) {
    fn(ctx, i * 13 + v, i * 7 + v * 3, x, y, small, false);
    x += small + 3;
  }
  x += gap - 3;
  if (i === 3) {
    x = gap;
    y += r2h;
  }
}

// Row 3: deposits on matching terrain.
x = gap;
y += 24;
const deposits: Array<[string, number, string]> = [
  ["Ore", 1800, "Plains"],
  ["Steel", 1800, "Hills"],
  ["Coal", 1800, "Desert"],
  ["Crystal", 700, "Forest"],
];
for (const [res, amount, terrainKind] of deposits) {
  const upper = terrainKind.toUpperCase();
  const [name, fn] = kinds.find(([n]) => n === upper)!;
  fn(ctx, 9, 9, x, y, big, false);
  drawResourceDeposit(ctx, res, x, y, big, amount, 0, 3);
  ctx.fillStyle = "#e7ca8a";
  ctx.font = "bold 12px monospace";
  ctx.fillText(`${res} (rich)`, x + 4, y + big + 16);
  x += big + gap;
}

// Legacy crystal deposit on dark base for comparison.
void drawCrystalDeposit;
void drawOreDeposit;

ctx.fillStyle = "#64748b";
ctx.font = "11px monospace";
ctx.fillText("top row: zoom 96 · middle: zoom 20 (4 hash variants) · bottom: deposits on natural terrain", gap, canvas.height - 20);
