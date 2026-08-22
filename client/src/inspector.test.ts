import { describe, expect, it } from "vitest";
import { MAP_SIZE, MAP_TILES } from "./types";
import { inspectionForTile, inspectTile } from "./inspector";
import { World } from "./world";

function worldWithVisibleMap(): World {
  const world = new World();
  world.setMap(
    7,
    new Array<boolean>(MAP_TILES).fill(true),
    new Array<string>(MAP_TILES).fill("Plains"),
    [[10, 10], [53, 53]],
  );
  const visible = new Set<number>([10 * MAP_SIZE + 10, 10 * MAP_SIZE + 11, 10 * MAP_SIZE + 12]);
  world.visible = visible;
  for (const index of visible) world.explored.add(index);
  world.entities.set(1, {
    id: 1,
    kind: "Hq",
    owner: 0,
    x: 10.5,
    y: 10.5,
    hp: 1500,
    maxHp: 1500,
  });
  world.entities.set(2, {
    id: 2,
    kind: "Infantry",
    owner: 0,
    x: 11.5,
    y: 10.5,
    hp: 90,
    maxHp: 90,
    mp: 3,
    maxMp: 3,
    moveTarget: [20, 20],
  });
  world.resourceTiles.set("12,10", {
    x: 12,
    y: 10,
    resource: "Steel",
    amount: 1,
    richness: 3,
    infinite: true,
    yieldPerTurn: 90,
    refineryOwner: null,
  });
  return world;
}

describe("tile inspector", () => {
  it("reports visible terrain, occupants, movement, and infinite richness", () => {
    const world = worldWithVisibleMap();
    const info = inspectTile(world, 12, 10, new Set([2]));

    expect(info.visibility).toBe("visible");
    expect(info.terrain?.label).toBe("Plains");
    expect(info.resource).toMatchObject({
      resource: "Steel",
      richness: 3,
      infinite: true,
      yieldPerTurn: 90,
    });
    expect(info.occupants).toHaveLength(0);
    expect(info.movement).toEqual([
      expect.objectContaining({
        unitId: 2,
        unitKind: "Infantry",
        movePoints: 3,
        terrainCost: 1,
        canEnter: true,
      }),
    ]);
    expect(info.routeTargets).toEqual([{ id: 2, target: [20, 20] }]);
    expect(info.placement.resource).toBe("Steel");
    expect(info.placement.refinerySiteAvailable).toBe(false);
  });

  it("does not leak hidden map facts for an unexplored tile", () => {
    const world = worldWithVisibleMap();
    world.resourceTiles.set("40,40", {
      x: 40,
      y: 40,
      resource: "Crystal",
      amount: 1000,
      richness: 3,
      infinite: true,
    });

    const info = inspectTile(world, 40, 40);
    expect(info.visibility).toBe("unexplored");
    expect(info.terrain).toBeNull();
    expect(info.resource).toBeNull();
    expect(info.occupants).toEqual([]);
    expect(info.placement.known).toBe(false);
  });

  it("prefers a matching server inspection while retaining the selected tile", () => {
    const world = worldWithVisibleMap();
    world.applyTileInspection({
      x: 12,
      y: 10,
      index: 652,
      visibility: "visible",
      terrain: {
        kind: "Hills",
        label: "Hills",
        passable: true,
        moveMultiplier: 2,
        defenseReduction: 30,
        tacticalTag: "high ground",
      },
      elevation: 220,
      moisture: 80,
      resource: {
        x: 12,
        y: 10,
        resource: "Steel",
        amount: 1,
        richness: 3,
        infinite: true,
        yieldPerTurn: 90,
        refineryOwner: 0,
      },
      occupants: [],
      movement: [],
      routeTargets: [],
      placement: {
        known: true,
        passable: true,
        occupiedByBuilding: false,
        occupiedByUnit: false,
        resource: "Steel",
        withinBaseRadius: true,
        structureSiteAvailable: false,
        refinerySiteAvailable: false,
      },
    });

    const info = inspectionForTile(world, 12, 10);
    expect(info.authoritative).toBe(true);
    expect(info.terrain?.kind).toBe("Hills");
    expect(info.elevation).toBe(220);
    expect(info.resource?.refineryOwner).toBe(0);
  });
});
