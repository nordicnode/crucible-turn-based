export type PixelPattern = readonly string[];
export type PixelPalette = Readonly<Record<string, string>>;

/** Return a crisp raster cell size; fractional sizes become alternating 1/2px cells. */
export function pixelScale(span: number, cells: number): number {
  return Math.max(1, Math.max(1, span) / Math.max(1, cells));
}

/**
 * Paint a tiny indexed-color sprite around its center. A dot or space is
 * transparent; every other character is looked up in the supplied palette.
 */
export function drawPixelSprite(
  ctx: CanvasRenderingContext2D,
  pattern: PixelPattern,
  cx: number,
  cy: number,
  requestedScale: number,
  palette: PixelPalette,
  requestedYScale: number = requestedScale,
): void {
  const scale = Math.max(1, requestedScale);
  const yScale = Math.max(1, requestedYScale);
  const width = pattern.reduce((max, row) => Math.max(max, row.length), 0);
  const height = pattern.length;
  const x0 = Math.floor(cx - (width * scale) / 2);
  const y0 = Math.floor(cy - (height * yScale) / 2);

  for (let row = 0; row < height; row++) {
    const line = pattern[row];
    const y = Math.floor(y0 + row * yScale);
    const nextY = Math.floor(y0 + (row + 1) * yScale);
    for (let col = 0; col < line.length; col++) {
      const color = palette[line[col]];
      if (!color) continue;
      const x = Math.floor(x0 + col * scale);
      const nextX = Math.floor(x0 + (col + 1) * scale);
      ctx.fillStyle = color;
      ctx.fillRect(x, y, Math.max(1, nextX - x), Math.max(1, nextY - y));
    }
  }
}

/** Draw a square-stepped projectile, beam, or track segment. */
export function drawPixelLine(
  ctx: CanvasRenderingContext2D,
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  thickness: number,
  color: string,
): void {
  const width = Math.max(1, Math.floor(thickness));
  const distance = Math.hypot(x2 - x1, y2 - y1);
  const steps = Math.max(1, Math.ceil(distance / Math.max(1, width * 0.65)));
  ctx.fillStyle = color;
  for (let i = 0; i <= steps; i++) {
    const t = i / steps;
    const x = Math.floor(x1 + (x2 - x1) * t - width / 2);
    const y = Math.floor(y1 + (y2 - y1) * t - width / 2);
    ctx.fillRect(x, y, width, width);
  }
}
