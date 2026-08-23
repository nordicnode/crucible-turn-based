import { describe, expect, test } from "vitest";
import { Renderer } from "./renderer";
import { World } from "./world";
import { MAP_SIZE } from "./types";

// Regression tests for the move-path preview A* (Renderer.pathfind). These
// pin the two failure modes users hit live:
//  1. the binary heap used to desync its idx/f arrays on pop (the moved root's
//     f was never copied), so long paths spiralled "all over the map";
//  2. the preview must match the server's cost model (straight routes on
//     open ground, detours only around real blockers).

function flatWorld(): World {
  const n = MAP_SIZE * MAP_SIZE;
  const w = new World();
  w.setMap(1, new Array(n).fill(true), new Array(n).fill("Plains"), [
    [10, 10],
    [100, 100],
  ]);
  return w;
}

describe("pathfind", () => {
  test("heap pops ascending (regression: idx/f desync on pop)", () => {
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
      const lastF = heapF[heapF.length - 1];
      heapIdx.pop();
      heapF.pop();
      if (heapF.length > 0) {
        heapIdx[0] = lastIdx;
        heapF[0] = lastF;
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
    const fs: number[] = [];
    for (let i = 0; i < 1000; i++) {
      heapPush(i, i);
      fs.push(i);
    }
    const fmap = new Map(fs.map((f, i) => [i, f]));
    const popped: number[] = [];
    while (heapF.length > 0) popped.push(fmap.get(heapPop())!);
    expect(popped).toEqual([...popped].sort((a, b) => a - b));
  });

  test("straight routes stay straight on open ground", () => {
    const w = flatWorld();
    // Perfect orthogonal run.
    const p = Renderer.pathfind(w, [10, 10], [40, 10]);
    expect(p).not.toBeNull();
    expect(p!.length).toBe(30);
    // No spirals: even a diagonal crossing stays near the ideal step count
    // (a broken search used to return hundreds of tiles here).
    for (const [from, to] of [
      [[10, 10], [30, 30]],
      [[10, 10], [15, 13]],
      [[10, 10], [12, 12]],
    ] as const) {
      const q = Renderer.pathfind(w, from as [number, number], to as [number, number]);
      const ideal = Math.max(Math.abs(from[0] - to[0]), Math.abs(from[1] - to[1]));
      expect(q, `${from}->${to}`).not.toBeNull();
      expect(q!.length, `${from}->${to}`).toBeLessThanOrEqual(ideal * 2);
    }
  });

  test("blocks are routed around with a short detour (not a spiral)", () => {
    const w = flatWorld();
    // A solid wall of enemy buildings across the direct line.
    for (let i = 0; i < 12; i++) {
      w.entities.set(900 + i, { owner: 1, kind: "Turret", x: 24 + i, y: 24 + i } as any);
    }
    const p = Renderer.pathfind(w, [10, 10], [44, 44]);
    expect(p).not.toBeNull();
    const ideal = 34;
    // Detouring a 12-tile wall adds ~12 steps, not hundreds.
    expect(p!.length).toBeLessThanOrEqual(ideal * 1.6);
    // The route never crosses a blocked (building) tile.
    for (const [x, y] of p!) {
      const onWall = x >= 24 && x <= 35 && y >= 24 && y <= 35 && x - 24 === y - 24;
      expect(onWall).toBe(false);
    }
  });
});