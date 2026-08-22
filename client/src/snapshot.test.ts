import { describe, expect, it } from "vitest";
import { MAP_SIZE, MAP_TILES } from "./types";
import { applyFrame, applyMeta, type ReplayFrame, type ReplayMeta } from "./snapshot";
import { World } from "./world";

function metaFixture(): ReplayMeta {
  const passable = new Array<boolean>(MAP_TILES).fill(true);
  const ore = new Array<number>(MAP_TILES).fill(0);
  ore[10 * MAP_SIZE + 10] = 400;
  const crystal = new Array<number>(MAP_TILES).fill(0);
  crystal[20 * MAP_SIZE + 20] = 150;
  return {
    map_seed: 7,
    passable,
    terrain: [],
    hq_tiles: [
      [8, 8],
      [55, 55],
    ],
    ore,
    crystal,
    duration_turns: 100,
    winner: null,
    win_reason: null,
  };
}

describe("snapshot mapping", () => {
  it("applies map meta with full visibility and ore tiles", () => {
    const w = new World();
    applyMeta(w, metaFixture());
    expect(w.mapSeed).toBe(7);
    expect(w.hq).toEqual([
      [8, 8],
      [55, 55],
    ]);
    expect(w.visible.size).toBe(MAP_TILES);
    expect(w.oreTiles.get("10,10")).toEqual({ x: 10, y: 10, amount: 400 });
  });

  it("maps a frame's units/buildings into entities (camelCase maxHp)", () => {
    const w = new World();
    applyMeta(w, metaFixture());
    const frame: ReplayFrame = {
      turn: 50,
      active: 0,
      ore0: 300,
      ore1: 400,
      units: [{ id: 3, kind: "Infantry", owner: 0, x: 8, y: 9, hp: 40, max_hp: 40 }],
      buildings: [{ id: 1, kind: "Hq", owner: 0, x: 8, y: 8, hp: 1500, max_hp: 1500 }],
      winner: null,
      win_reason: null,
    };
    applyFrame(w, frame);
    expect(w.turn).toBe(50);
    expect(w.activePlayer).toBe(0);
    expect(w.ore).toBe(300);
    expect(w.entities.get(3)).toMatchObject({ kind: "Infantry", owner: 0, maxHp: 40 });
    expect(w.entities.get(1)?.kind).toBe("Hq");
  });

  it("sets the result when the frame has a winner", () => {
    const w = new World();
    applyMeta(w, metaFixture());
    applyFrame(w, {
      turn: 100,
      active: 1,
      ore0: 0,
      ore1: 500,
      units: [],
      buildings: [],
      winner: 1,
      win_reason: "HqDestroyed",
    });
    expect(w.result).toEqual({ winner: 1, reason: "HqDestroyed" });
  });
});

describe("turn-based frames", () => {
  it("replaces entities wholesale on each frame", () => {
    const w = new World();
    applyMeta(w, metaFixture());
    const frame = (turn: number): ReplayFrame => ({
      turn,
      active: 0,
      ore0: 100,
      ore1: 100,
      units: [{ id: 3, kind: "Infantry", owner: 0, x: 10, y: 10, hp: 40, max_hp: 40 }],
      buildings: [],
      winner: null,
      win_reason: null,
    });
    applyFrame(w, frame(10));
    expect(w.turn).toBe(10);
    expect(w.entities.size).toBe(1);
    expect(w.entities.get(3)).toMatchObject({ x: 10, y: 10 });

    // A later frame with no units clears the board — no leftover state.
    applyFrame(w, { ...frame(11), units: [] });
    expect(w.turn).toBe(11);
    expect(w.entities.size).toBe(0);
  });
});
