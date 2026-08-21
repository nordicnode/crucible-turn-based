// Camera math: zoom bounds, map-bounds clamping, and viewport rects. Pure logic, no DOM.

import { describe, expect, it } from "vitest";
import { Camera, cameraViewRect, isBuildingPlacable, Renderer } from "./renderer";
import { World } from "./world";

describe("Camera", () => {
  it("clamps zoom to the min/max bounds", () => {
    const c = new Camera();
    c.zoomAt(0, 0, 1000);
    expect(c.zoom).toBeLessThanOrEqual(96);
    c.zoomAt(0, 0, 0.00001);
    expect(c.zoom).toBeGreaterThanOrEqual(4);
  });

  it("centers the viewport on the requested world point", () => {
    const c = new Camera();
    c.focusOn(15.5, 13.5, 18, 800, 600);
    expect(c.screenX(15.5)).toBeCloseTo(400, 5);
    expect(c.screenY(13.5)).toBeCloseTo(300, 5);
  });

  it("keeps the view inside the map when zoomed in", () => {
    const c = new Camera();
    c.setViewport(800, 600);
    c.zoom = 32; // view = 25 x 18.75 tiles, smaller than the map
    c.cx = -500;
    c.cy = 999;
    c.pan(0, 0); // triggers clamp
    expect(c.cx).toBeGreaterThanOrEqual(0);
    expect(c.cx).toBeLessThanOrEqual(64 - 25);
    expect(c.cy).toBeGreaterThanOrEqual(0);
    expect(c.cy).toBeLessThanOrEqual(64 - 18.75);
  });

  it("centers the map when the viewport is larger than the map", () => {
    const c = new Camera();
    c.setViewport(2000, 2000);
    c.zoom = 4; // view is 500 tiles > 64
    c.cx = 3;
    c.cy = 3;
    c.pan(0, 0);
    // cx/cy anchor the top-left, so centering means cx == (MAP - viewW) / 2
    expect(c.cx).toBeCloseTo((64 - 500) / 2, 5);
    expect(c.cy).toBeCloseTo((64 - 500) / 2, 5);
    // And the whole map is on screen: the map left/right edges land inside
    // the viewport, symmetric around its center.
    expect(c.screenX(0)).toBeGreaterThan(0);
    expect(c.screenX(64)).toBeLessThan(2000);
    expect(c.screenX(32)).toBeCloseTo(1000, 5);
  });

  it("keeps the view inside the map when the viewport is smaller", () => {
    const c = new Camera();
    c.setViewport(400, 400);
    c.zoom = 32; // view = 12.5 tiles
    c.cx = -10;
    c.cy = -10;
    c.pan(0, 0);
    expect(c.cx).toBeCloseTo(0, 5);
    expect(c.cy).toBeCloseTo(0, 5);
  });

  it("zooming in at the screen center keeps the cursor's world point fixed", () => {
    const c = new Camera();
    c.focusOn(30, 30, 18, 800, 600);
    const sx = 200;
    const sy = 150;
    const wx = c.worldX(sx);
    const wy = c.worldY(sy);
    c.zoomAt(sx, sy, 2);
    expect(c.worldX(sx)).toBeCloseTo(wx, 5);
    expect(c.worldY(sy)).toBeCloseTo(wy, 5);
  });
});

describe("cameraViewRect", () => {
  it("is the visible world rect when fully inside the map", () => {
    const c = new Camera();
    c.focusOn(32, 32, 32, 800, 600);
    const vr = cameraViewRect(c, 800, 600);
    expect(vr).not.toBeNull();
    expect(vr!.x).toBeCloseTo(32 - 12.5, 5);
    expect(vr!.y).toBeCloseTo(32 - 9.375, 5);
    expect(vr!.w).toBeCloseTo(25, 5);
    expect(vr!.h).toBeCloseTo(18.75, 5);
  });

  it("clips to the map when the camera hangs off the left edge", () => {
    const c = new Camera();
    c.focusOn(4, 4, 32, 800, 600);
    // Disable automatic clamp to test clipping logic
    c.cx = 4 - 400 / 32;
    c.cy = 4 - 300 / 32;
    const vr = cameraViewRect(c, 800, 600);
    expect(vr).not.toBeNull();
    expect(vr!.x).toBe(0);
    expect(vr!.y).toBe(0);
    expect(vr!.w).toBeCloseTo(16.5, 5);
    expect(vr!.h).toBeCloseTo(13.375, 5);
  });

  it("covers the whole map when zoomed out past the map size", () => {
    const c = new Camera();
    c.setViewport(2000, 2000);
    c.zoom = 4;
    c.cx = (64 - 500) / 2;
    c.cy = (64 - 500) / 2;
    const vr = cameraViewRect(c, 2000, 2000);
    expect(vr).toEqual({ x: 0, y: 0, w: 64, h: 64 });
  });

  it("returns null when the camera is entirely off the map", () => {
    const c = new Camera();
    c.cx = -100;
    c.cy = -100;
    c.zoom = 4;
    expect(cameraViewRect(c, 200, 200)).toBeNull();
  });
});

describe("isBuildingPlacable", () => {
  it("allows placement on clear tile within range of base with enough ore", () => {
    const world = new World();
    world.setMap(1, new Array(64 * 64).fill(true), [[10, 10], [50, 50]]);
    world.ore = 500;
    world.entities.set(1, { id: 1, kind: "Hq", owner: 0, x: 10, y: 10, hp: 1500, maxHp: 1500 });

    expect(isBuildingPlacable("Refinery", [12, 10], world)).toBe(true);
  });

  it("rejects placement on impassable tile", () => {
    const world = new World();
    const passable = new Array(64 * 64).fill(true);
    passable[10 * 64 + 12] = false;
    world.setMap(1, passable, [[10, 10], [50, 50]]);
    world.ore = 500;
    world.entities.set(1, { id: 1, kind: "Hq", owner: 0, x: 10, y: 10, hp: 1500, maxHp: 1500 });

    expect(isBuildingPlacable("Refinery", [12, 10], world)).toBe(false);
  });

  it("rejects placement on tile with existing building", () => {
    const world = new World();
    world.setMap(1, new Array(64 * 64).fill(true), [[10, 10], [50, 50]]);
    world.ore = 500;
    world.entities.set(1, { id: 1, kind: "Hq", owner: 0, x: 10, y: 10, hp: 1500, maxHp: 1500 });

    expect(isBuildingPlacable("Barracks", [10, 10], world)).toBe(false);
  });

  it("rejects placement on tile with ore", () => {
    const world = new World();
    world.setMap(1, new Array(64 * 64).fill(true), [[10, 10], [50, 50]]);
    world.ore = 500;
    world.entities.set(1, { id: 1, kind: "Hq", owner: 0, x: 10, y: 10, hp: 1500, maxHp: 1500 });
    world.oreTiles.set("12,10", { x: 12, y: 10, amount: 200 });

    expect(isBuildingPlacable("Refinery", [12, 10], world)).toBe(false);
  });

  it("rejects placement too far from own base", () => {
    const world = new World();
    world.setMap(1, new Array(64 * 64).fill(true), [[10, 10], [50, 50]]);
    world.ore = 500;
    world.entities.set(1, { id: 1, kind: "Hq", owner: 0, x: 10, y: 10, hp: 1500, maxHp: 1500 });

    // Distance 10 tiles away (> 5 tiles limit)
    expect(isBuildingPlacable("Refinery", [20, 10], world)).toBe(false);
  });

  it("rejects TechLab without Factory", () => {
    const world = new World();
    world.setMap(1, new Array(64 * 64).fill(true), [[10, 10], [50, 50]]);
    world.ore = 500;
    world.entities.set(1, { id: 1, kind: "Hq", owner: 0, x: 10, y: 10, hp: 1500, maxHp: 1500 });

    expect(isBuildingPlacable("TechLab", [12, 10], world)).toBe(false);

    // Add Factory -> TechLab now valid
    world.entities.set(2, { id: 2, kind: "Factory", owner: 0, x: 11, y: 10, hp: 350, maxHp: 350 });
    expect(isBuildingPlacable("TechLab", [12, 10], world)).toBe(true);
  });

  it("rejects placement when player cannot afford building", () => {
    const world = new World();
    world.setMap(1, new Array(64 * 64).fill(true), [[10, 10], [50, 50]]);
    world.ore = 50; // Refinery costs 300
    world.entities.set(1, { id: 1, kind: "Hq", owner: 0, x: 10, y: 10, hp: 1500, maxHp: 1500 });

    expect(isBuildingPlacable("Refinery", [12, 10], world)).toBe(false);
  });

  it("validates PowerPlant placability and affordability", () => {
    const world = new World();
    world.setMap(1, new Array(64 * 64).fill(true), [[10, 10], [50, 50]]);
    world.ore = 100; // PowerPlant costs 150
    world.entities.set(1, { id: 1, kind: "Hq", owner: 0, x: 10, y: 10, hp: 1500, maxHp: 1500 });

    expect(isBuildingPlacable("PowerPlant", [12, 10], world)).toBe(false);

    world.ore = 150;
    expect(isBuildingPlacable("PowerPlant", [12, 10], world)).toBe(true);
  });

  it("filters ownBuildings and calculates power produced and consumed", () => {
    const world = new World();
    world.entities.set(1, { id: 1, kind: "Hq", owner: 0, x: 10, y: 10, hp: 1500, maxHp: 1500 });
    world.entities.set(2, { id: 2, kind: "PowerPlant", owner: 0, x: 12, y: 10, hp: 400, maxHp: 400 });
    world.entities.set(3, { id: 3, kind: "Factory", owner: 0, x: 14, y: 10, hp: 600, maxHp: 600 });
    world.entities.set(4, { id: 4, kind: "Tank", owner: 0, x: 14, y: 12, hp: 120, maxHp: 120 });
    world.entities.set(5, { id: 5, kind: "Factory", owner: 1, x: 50, y: 50, hp: 600, maxHp: 600 });

    expect(world.ownBuildings.length).toBe(3);
    expect(world.ownUnits.length).toBe(1);

    // HQ (+50) + PowerPlant (+100) = 150 produced
    // Factory (-25) = 25 consumed
    expect(world.ownPower.produced).toBe(150);
    expect(world.ownPower.consumed).toBe(25);
  });

  it("validates Airfield factory requirement and affordability", () => {
    const world = new World();
    world.setMap(1, new Array(64 * 64).fill(true), [[10, 10], [50, 50]]);
    world.ore = 500;
    world.entities.set(1, { id: 1, kind: "Hq", owner: 0, x: 10, y: 10, hp: 1500, maxHp: 1500 });

    expect(isBuildingPlacable("Airfield", [12, 10], world)).toBe(false);

    // Add Factory -> Airfield now placable
    world.entities.set(2, { id: 2, kind: "Factory", owner: 0, x: 11, y: 10, hp: 350, maxHp: 350 });
    expect(isBuildingPlacable("Airfield", [12, 10], world)).toBe(true);

    // Can't afford Airfield (costs 250)
    world.ore = 200;
    expect(isBuildingPlacable("Airfield", [12, 10], world)).toBe(false);
  });

  it("draws a world with a building without error", () => {
    const world = new World();
    world.setMap(1, new Array(64 * 64).fill(true), [[10, 10], [50, 50]]);
    world.entities.set(1, {
      id: 1,
      kind: "Barracks",
      owner: 0,
      x: 12,
      y: 10,
      hp: 300,
      maxHp: 300,
    });

    const renderer = new Renderer();
    const mockCtx = {
      save: () => {},
      restore: () => {},
      beginPath: () => {},
      moveTo: () => {},
      lineTo: () => {},
      stroke: () => {},
      arc: () => {},
      fillRect: () => {},
      strokeRect: () => {},
      setLineDash: () => {},
      clearRect: () => {},
      fill: () => {},
      closePath: () => {},
      measureText: () => ({ width: 10 }),
      fillText: () => {},
      createLinearGradient: () => ({ addColorStop: () => {} }),
      createRadialGradient: () => ({ addColorStop: () => {} }),
    } as unknown as CanvasRenderingContext2D;

    expect(() => {
      renderer.draw(mockCtx, world, new Set([1]), 800, 600, {
        waypoints: new Map(),
      });
    }).not.toThrow();
  });
});
