import { describe, expect, it } from "vitest";
import { fx } from "./fx";
import { Camera } from "./renderer";
import {
  drawBuildingSprite,
  drawCivUnGreyOverlay,
  drawDesertTile,
  drawForestTile,
  drawHealthBar,
  drawHillsTile,
  drawImpassableTile,
  drawMountainTile,
  drawOreDeposit,
  drawPassableTile,
  drawRiverTile,
  drawSelectionReticle,
  drawSwampTile,
  drawTacticalIcon,
  drawUnitSprite,
  drawWaterTile,
  getCursorDataUrl,
  getTeamPalette,
  getThumbnailDataUrl,
  TEAM_BLUE,
  TEAM_RED,
  TEAM_STALE,
} from "./sprites";
import { PLAYABLE_BUILDING_TYPES, PLAYABLE_UNIT_TYPES } from "./types";

class MockCanvasContext {
  fillStyle: string | CanvasGradient | CanvasPattern = "";
  strokeStyle: string | CanvasGradient | CanvasPattern = "";
  lineWidth = 1;
  globalAlpha = 1;
  lineDashOffset = 0;

  save(): void {}
  restore(): void {}
  translate(_x: number, _y: number): void {}
  rotate(_angle: number): void {}
  beginPath(): void {}
  closePath(): void {}
  moveTo(_x: number, _y: number): void {}
  lineTo(_x: number, _y: number): void {}
  stroke(): void {}
  fill(): void {}
  fillRect(_x: number, _y: number, _w: number, _h: number): void {}
  strokeRect(_x: number, _y: number, _w: number, _h: number): void {}
  arc(_x: number, _y: number, _r: number, _s: number, _e: number): void {}
  ellipse(_x: number, _y: number, _rx: number, _ry: number, _rot: number, _s: number, _e: number): void {}
  quadraticCurveTo(_cpx: number, _cpy: number, _x: number, _y: number): void {}
  roundRect(_x: number, _y: number, _w: number, _h: number, _r: number): void {}
  setLineDash(_segments: number[]): void {}
  createRadialGradient(_x0: number, _y0: number, _r0: number, _x1: number, _y1: number, _r1: number): CanvasGradient {
    return { addColorStop: (_offset: number, _color: string) => {} } as unknown as CanvasGradient;
  }
  createLinearGradient(_x0: number, _y0: number, _x1: number, _y1: number): CanvasGradient {
    return { addColorStop: (_offset: number, _color: string) => {} } as unknown as CanvasGradient;
  }
}

describe("Team palettes", () => {
  it("returns blue for P0 and red for P1", () => {
    expect(getTeamPalette(0)).toEqual(TEAM_BLUE);
    expect(getTeamPalette(1)).toEqual(TEAM_RED);
    expect(getTeamPalette(1, true)).toEqual(TEAM_STALE);
  });
});

describe("Terrain sprite rendering", () => {
  const ctx = new MockCanvasContext() as unknown as CanvasRenderingContext2D;

  it("renders passable tiles for active and fogged views", () => {
    expect(() => drawPassableTile(ctx, 5, 10, 50, 100, 18, false)).not.toThrow();
    expect(() => drawPassableTile(ctx, 5, 10, 50, 100, 18, true)).not.toThrow();
  });

  it("renders full-tile water and rivers with animated ticks", () => {
    expect(() => drawWaterTile(ctx, 3, 4, 30, 40, 24, false, 10)).not.toThrow();
    expect(() => drawWaterTile(ctx, 3, 4, 30, 40, 24, true, 10)).not.toThrow();
    expect(() => drawRiverTile(ctx, 5, 6, 50, 60, 24, false, 15)).not.toThrow();
    expect(() => drawRiverTile(ctx, 5, 6, 50, 60, 24, true, 15)).not.toThrow();
  });

  it("renders forest, hills, desert, swamp, and mountain biomes", () => {
    expect(() => drawForestTile(ctx, 1, 2, 10, 20, 20, false)).not.toThrow();
    expect(() => drawForestTile(ctx, 1, 2, 10, 20, 20, true)).not.toThrow();
    expect(() => drawHillsTile(ctx, 2, 3, 20, 30, 20, false)).not.toThrow();
    expect(() => drawHillsTile(ctx, 2, 3, 20, 30, 20, true)).not.toThrow();
    expect(() => drawDesertTile(ctx, 3, 4, 30, 40, 20, false)).not.toThrow();
    expect(() => drawDesertTile(ctx, 3, 4, 30, 40, 20, true)).not.toThrow();
    expect(() => drawSwampTile(ctx, 4, 5, 40, 50, 20, false)).not.toThrow();
    expect(() => drawSwampTile(ctx, 4, 5, 40, 50, 20, true)).not.toThrow();
    expect(() => drawMountainTile(ctx, 5, 6, 50, 60, 20, false)).not.toThrow();
    expect(() => drawMountainTile(ctx, 5, 6, 50, 60, 20, true)).not.toThrow();
  });

  it("renders impassable tiles for active and fogged views", () => {
    expect(() => drawImpassableTile(ctx, 8, 12, 80, 120, 18, false)).not.toThrow();
    expect(() => drawImpassableTile(ctx, 8, 12, 80, 120, 18, true)).not.toThrow();
  });

  it("renders ore deposits at varying richness tiers", () => {
    expect(() => drawOreDeposit(ctx, 100, 100, 18, 500, 10)).not.toThrow();
    expect(() => drawOreDeposit(ctx, 100, 100, 18, 100, 10)).not.toThrow();
  });
});

describe("Civilization-style un-greying construction rendering", () => {
  const ctx = new MockCanvasContext() as unknown as CanvasRenderingContext2D;

  it("renders construction wipe overlay across turn progress (0/3, 1/3, 2/3, 3/3)", () => {
    expect(() => drawCivUnGreyOverlay(ctx, 50, 50, 30, 30, 0, 3, 5)).not.toThrow();
    expect(() => drawCivUnGreyOverlay(ctx, 50, 50, 30, 30, 1, 3, 10)).not.toThrow();
    expect(() => drawCivUnGreyOverlay(ctx, 50, 50, 30, 30, 2, 3, 15)).not.toThrow();
    expect(() => drawCivUnGreyOverlay(ctx, 50, 50, 30, 30, 3, 3, 20)).not.toThrow();
  });
});

describe("Building sprite rendering", () => {
  const ctx = new MockCanvasContext() as unknown as CanvasRenderingContext2D;
  const buildings = PLAYABLE_BUILDING_TYPES;

  for (const b of buildings) {
    it(`renders ${b} for P0, P1, and stale state`, () => {
      expect(() => drawBuildingSprite(ctx, b, 100, 100, 18, 0, 0, 5)).not.toThrow();
      expect(() => drawBuildingSprite(ctx, b, 100, 100, 18, 1, 0, 5)).not.toThrow();
      expect(() => drawBuildingSprite(ctx, b, 100, 100, 18, 1, 0, 5, true)).not.toThrow();
    });

    it(`renders ${b} with Civilization un-grey construction progression`, () => {
      expect(() => drawBuildingSprite(ctx, b, 100, 100, 18, 0, 0, 5, false, 0, 2)).not.toThrow();
      expect(() => drawBuildingSprite(ctx, b, 100, 100, 18, 0, 0, 5, false, 1, 2)).not.toThrow();
      expect(() => drawBuildingSprite(ctx, b, 100, 100, 18, 0, 0, 5, false, 2, 2)).not.toThrow();
    });
  }
});

describe("Unit sprite rendering", () => {
  const ctx = new MockCanvasContext() as unknown as CanvasRenderingContext2D;
  const units = PLAYABLE_UNIT_TYPES;

  for (const u of units) {
    it(`renders ${u} with direction and owner`, () => {
      expect(() => drawUnitSprite(ctx, u, 50, 50, 18, 0, Math.PI / 4, 10)).not.toThrow();
      expect(() => drawUnitSprite(ctx, u, 50, 50, 18, 1, -Math.PI / 2, 10)).not.toThrow();
      expect(() => drawUnitSprite(ctx, u, 50, 50, 18, 1, 0, 10, true)).not.toThrow();
    });
  }

  it("renders units undergoing construction with Civilization un-grey overlay", () => {
    expect(() => drawUnitSprite(ctx, "Tank", 50, 50, 18, 0, 0, 10, false, -1, false, 0, 2)).not.toThrow();
    expect(() => drawUnitSprite(ctx, "Tank", 50, 50, 18, 0, 0, 10, false, -1, false, 1, 2)).not.toThrow();
    expect(() => drawUnitSprite(ctx, "Tank", 50, 50, 18, 0, 0, 10, false, -1, false, 2, 2)).not.toThrow();
  });

  it("renders Tank with active firing recoil and muzzle blast", () => {
    expect(() => drawUnitSprite(ctx, "Tank", 50, 50, 18, 0, 0, 10, false, 0, false)).not.toThrow();
    expect(() => drawUnitSprite(ctx, "Tank", 50, 50, 18, 0, 0, 10, false, 1, false)).not.toThrow();
    expect(() => drawUnitSprite(ctx, "Tank", 50, 50, 18, 0, 0, 10, false, 2, false)).not.toThrow();
  });

  it("renders Artillery and Infantry with firing animations", () => {
    expect(() => drawUnitSprite(ctx, "Artillery", 50, 50, 18, 0, 0, 10, false, 0, false)).not.toThrow();
    expect(() => drawUnitSprite(ctx, "Infantry", 50, 50, 18, 0, 0, 10, false, 0, false)).not.toThrow();
  });

  it("renders Infantry moving vs standing still", () => {
    expect(() => drawUnitSprite(ctx, "Infantry", 50, 50, 18, 0, 0, 10, false, -1, true)).not.toThrow();
    expect(() => drawUnitSprite(ctx, "Infantry", 50, 50, 18, 0, 0, 10, false, -1, false)).not.toThrow();
  });

  it("renders Turret with firing recoil and flashes", () => {
    expect(() => drawBuildingSprite(ctx, "Turret", 100, 100, 18, 0, 0, 10, false, 0, 0, 0)).not.toThrow();
  });

  it("renders the tech-tier buildings", () => {
    expect(() => drawBuildingSprite(ctx, "Radar", 100, 100, 18, 0, 0, 10)).not.toThrow();
    expect(() => drawBuildingSprite(ctx, "TeslaCoil", 100, 100, 18, 0, 0, 10)).not.toThrow();
    expect(() => drawBuildingSprite(ctx, "TeslaCoil", 100, 100, 18, 0, 0, 10, false, 0, 0, 2)).not.toThrow();
  });
});

describe("Thumbnails and Tactical Icons", () => {
  const ctx = new MockCanvasContext() as unknown as CanvasRenderingContext2D;

  it("generates a stable asset URL for every playable building and unit", () => {
    for (const kind of [...PLAYABLE_BUILDING_TYPES, ...PLAYABLE_UNIT_TYPES]) {
      const url = getThumbnailDataUrl(kind, 0);
      expect(url).toBe(`/assets/units/${kind.toLowerCase()}.svg`);
    }
  });

  it("renders tactical icons without throwing", () => {
    const icons = [
      "damage", "Damage", "hp", "Hp", "sell", "Sell", "repair", "Repair",
      "tab_buildings", "tab_troops", "tab_vehicles", "tab_aircraft",
      "airfield", "radar", "teslacoil", "mammothtank", "range",
      "gunship", "interceptor",
      "play", "pause", "fast_forward", "cross",
    ];
    for (const ic of icons) {
      expect(() => drawTacticalIcon(ctx, ic, 24, 24, 24, "#f59e0b")).not.toThrow();
    }
  });

  it("builds CSS cursor data URLs for the C&C tool cursors", () => {
    const sell = getCursorDataUrl("sell");
    const repair = getCursorDataUrl("repair");
    const attack = getCursorDataUrl("attack");
    // CSS cursor contract: with a DOM, a data URL plus a centered hotspot
    // (real browsers embed the PNG; jsdom's canvas may yield an empty URL);
    // without a DOM the helper degrades to the plain `default` cursor.
    if (typeof document === "undefined") {
      expect(sell).toBe("default");
    } else {
      for (const c of [sell, repair, attack]) {
        expect(c.startsWith("url(")).toBe(true);
        expect(c).toContain("16 16, auto");
      }
    }
    expect(getCursorDataUrl("sell")).toBe(sell);
  });
});

describe("FX Engine", () => {
  const ctx = new MockCanvasContext() as unknown as CanvasRenderingContext2D;

  it("spawns attacks, explosions, tracks, and updates properly", () => {
    fx.spawnAttack(10, 10, 15, 15, "bullet", "#60a5fa");
    fx.spawnAttack(10, 10, 20, 20, "artillery", "#f97316");
    fx.spawnExplosion(15, 15, "heavy");
    fx.recordVehicleMovement(1, "Tank", 10, 10, 0);
    fx.recordVehicleMovement(1, "Tank", 10.5, 10.5, 0.5);

    expect(fx.projectiles.length).toBeGreaterThan(0);
    expect(fx.explosions.length).toBeGreaterThan(0);
    expect(fx.tracks.length).toBeGreaterThan(0);

    fx.update(0.1);

    const cam = { screenX: (wx: number) => wx * 18, screenY: (wy: number) => wy * 18, zoom: 18 };
    expect(() => fx.drawGroundLayer(ctx, cam, 1000, 1000)).not.toThrow();
    expect(() => fx.drawAirLayer(ctx, cam, 1000, 1000)).not.toThrow();
  });
});

describe("Camera math", () => {
  it("centers and converts world/screen coordinates accurately", () => {
    const cam = new Camera();
    cam.centerOn(32, 32, 400, 300, 32);
    expect(cam.zoom).toBe(32);

    // Screen center corresponds to centered world point
    expect(cam.worldX(400 / 2)).toBeCloseTo(32, 1);
    expect(cam.worldY(300 / 2)).toBeCloseTo(32, 1);

    // Zooming at mouse cursor keeps world point stationary
    const mx = 200, my = 150;
    const wxBefore = cam.worldX(mx);
    const wyBefore = cam.worldY(my);
    cam.zoomAt(mx, my, 1.2, 400, 300);
    expect(cam.worldX(mx)).toBeCloseTo(wxBefore, 2);
    expect(cam.worldY(my)).toBeCloseTo(wyBefore, 2);
  });
});

describe("Tactical FX & Reticles", () => {
  const ctx = new MockCanvasContext() as unknown as CanvasRenderingContext2D;

  it("renders selection reticle", () => {
    expect(() => drawSelectionReticle(ctx, 50, 50, 20, 15)).not.toThrow();
  });

  it("renders health bars across various HP percentages", () => {
    expect(() => drawHealthBar(ctx, 50, 50, 20, 100, 100)).not.toThrow();
    expect(() => drawHealthBar(ctx, 50, 50, 20, 40, 100)).not.toThrow();
    expect(() => drawHealthBar(ctx, 50, 50, 20, 100, 100)).not.toThrow();
  });

});
