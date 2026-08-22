// Pure tile-inspection view models. This module combines only the fogged World
// state and server-supplied terrain metadata; it never invents hidden map data
// or issues commands.

import {
  BUILDING_KINDS,
  BUILD_COSTS,
  MAP_SIZE,
  resourceBundleAffordable,
  UNIT_KINDS,
} from "./types";
import type {
  PlacementFacts,
  ResourceTile,
  TerrainRule,
  TileInspection as WireTileInspection,
} from "./types";
import type { Entity } from "./world";
import { World } from "./world";

export type TileVisibility = "unexplored" | "explored" | "visible";

export interface TileMovement {
  unitId: number;
  unitKind: string;
  movePoints: number;
  terrainCost: number;
  canEnter: boolean;
}

export interface TileInspection {
  /** True when the values came from the server's fog-filtered response. */
  authoritative?: boolean;
  x: number;
  y: number;
  index: number;
  visibility: TileVisibility;
  terrain: TerrainRule | null;
  elevation: number | null;
  moisture: number | null;
  temperature: number | null;
  resource: ResourceTile | null;
  occupants: Entity[];
  refinery: Entity | null;
  movement: TileMovement[];
  routeTargets: Array<{ id: number; target: [number, number] }>;
  placement: PlacementFacts;
}

function entityAtTile(world: World, x: number, y: number): Entity[] {
  return [...world.entities.values()]
    .filter((entity) => Math.floor(entity.x) === x && Math.floor(entity.y) === y)
    .sort((a, b) => a.id - b.id);
}

function placementFacts(
  world: World,
  x: number,
  y: number,
  known: boolean,
  terrain: TerrainRule | null,
  resource: ResourceTile | null,
  occupants: Entity[],
): PlacementFacts {
  if (!known || !terrain) {
    return {
      known: false,
      passable: null,
      occupiedByBuilding: false,
      occupiedByUnit: false,
      resource: null,
      withinBaseRadius: false,
      structureSiteAvailable: false,
      refinerySiteAvailable: false,
    };
  }

  const occupiedByBuilding = occupants.some((entity) => BUILDING_KINDS.has(entity.kind));
  const occupiedByUnit = occupants.some((entity) => UNIT_KINDS.has(entity.kind));
  const withinBaseRadius = world.ownBuildings.some((building) => {
    const dx = Math.abs(Math.floor(building.x) - x);
    const dy = Math.abs(Math.floor(building.y) - y);
    return Math.max(dx, dy) <= 5;
  });
  const clear = !occupiedByBuilding && !occupiedByUnit;
  const resourceAvailable = resource != null && (resource.infinite === true || resource.amount > 0);
  const refineryCost = BUILD_COSTS.Refinery ?? { ore: 0, steel: 0, coal: 0, crystal: 0 };

  return {
    known: true,
    passable: terrain.passable,
    occupiedByBuilding,
    occupiedByUnit,
    resource: resource?.resource ?? null,
    withinBaseRadius,
    structureSiteAvailable: terrain.passable && clear && !resourceAvailable && withinBaseRadius,
    refinerySiteAvailable: terrain.passable
      && clear
      && resourceAvailable
      && resourceBundleAffordable(world.resources, refineryCost),
  };
}

/**
 * Build the complete information available for one tile. Unexplored tiles
 * intentionally return no terrain/resource/occupant details, even though the
 * match-start map is present in the browser for rendering and navigation.
 */
export function inspectTile(
  world: World,
  x: number,
  y: number,
  selectedIds: ReadonlySet<number> = new Set(),
): TileInspection {
  const index = y * MAP_SIZE + x;
  const visible = world.visible.has(index);
  const explored = world.explored.has(index);
  const visibility: TileVisibility = visible ? "visible" : explored ? "explored" : "unexplored";
  const known = visibility !== "unexplored";
  const terrain = known ? world.terrainRuleAt(x, y) : null;
  const occupants = known ? entityAtTile(world, x, y) : [];
  const resource = known ? world.resourceTiles.get(`${x},${y}`) ?? null : null;
  const refinery = occupants.find(
    (entity) => entity.kind === "Refinery" || entity.kind === "CrystalRefinery",
  ) ?? null;

  const movement: TileMovement[] = [];
  if (known && terrain) {
    for (const id of selectedIds) {
      const unit = world.entities.get(id);
      if (!unit || unit.owner !== 0 || !world.ownUnits.some((candidate) => candidate.id === id)) continue;
      const dx = Math.abs(Math.floor(unit.x) - x);
      const dy = Math.abs(Math.floor(unit.y) - y);
      const baseCost = dx === 0 && dy === 0 ? 0 : dx > 0 && dy > 0 ? 2 : 1;
      const terrainCost = baseCost * terrain.moveMultiplier;
      movement.push({
        unitId: id,
        unitKind: unit.kind,
        movePoints: unit.mp ?? 0,
        terrainCost,
        canEnter: terrain.passable && (baseCost === 0 || terrainCost <= (unit.mp ?? 0)),
      });
    }
  }

  const routeTargets = known
    ? [...world.ownUnits]
      .filter((entity) => entity.moveTarget)
      .map((entity) => ({ id: entity.id, target: entity.moveTarget! }))
    : [];

  return {
    x,
    y,
    index,
    visibility,
    terrain,
    elevation: known ? world.elevation[index] ?? null : null,
    moisture: known ? world.moisture[index] ?? null : null,
    temperature: known ? world.temperature[index] ?? null : null,
    resource,
    occupants,
    refinery,
    movement,
    routeTargets,
    placement: placementFacts(world, x, y, known, terrain, resource, occupants),
    authoritative: false,
  };
}

/** Prefer the server response for dynamic facts while retaining the local
 * model as an immediate, testable fallback between network frames. */
export function inspectionForTile(
  world: World,
  x: number,
  y: number,
  selectedIds: ReadonlySet<number> = new Set(),
): TileInspection {
  const local = inspectTile(world, x, y, selectedIds);
  const authoritative: WireTileInspection | null =
    world.tileInspection && world.tileInspection.x === x && world.tileInspection.y === y
      ? world.tileInspection
      : null;
  if (!authoritative) return local;
  return {
    ...local,
    visibility: authoritative.visibility,
    terrain: authoritative.terrain,
    elevation: authoritative.elevation ?? null,
    moisture: authoritative.moisture ?? null,
    temperature: authoritative.temperature ?? null,
    resource: authoritative.resource,
    occupants: authoritative.occupants,
    movement: authoritative.movement,
    refinery: authoritative.occupants.find(
      (entity) => entity.kind === "Refinery" || entity.kind === "CrystalRefinery",
    ) ?? null,
    routeTargets: authoritative.routeTargets.map(({ unitId, target }) => ({ id: unitId, target })),
    placement: authoritative.placement,
    authoritative: true,
  };
}
