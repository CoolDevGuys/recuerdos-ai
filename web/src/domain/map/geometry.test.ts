import { describe, expect, it } from "vitest";
import { centroid, convexHull, hashUnit, lerpPoint, smoothstep } from "./geometry";

describe("smoothstep", () => {
  it("clamps outside the edges and eases between", () => {
    expect(smoothstep(0, 1, -1)).toBe(0);
    expect(smoothstep(0, 1, 2)).toBe(1);
    expect(smoothstep(0, 1, 0.5)).toBe(0.5);
  });

  it("runs backwards when the edges are reversed (zoom ratios shrink as you zoom in)", () => {
    expect(smoothstep(0.85, 0.55, 1)).toBe(0);
    expect(smoothstep(0.85, 0.55, 0.5)).toBe(1);
  });
});

describe("convexHull", () => {
  it("drops interior points", () => {
    const hull = convexHull([
      { x: 0, y: 0 },
      { x: 2, y: 0 },
      { x: 2, y: 2 },
      { x: 0, y: 2 },
      { x: 1, y: 1 },
    ]);
    expect(hull).toHaveLength(4);
    expect(hull).not.toContainEqual({ x: 1, y: 1 });
  });

  it("returns fewer than three distinct points unchanged", () => {
    expect(convexHull([{ x: 1, y: 1 }, { x: 1, y: 1 }])).toEqual([{ x: 1, y: 1 }]);
  });
});

describe("helpers", () => {
  it("finds the centroid and interpolates points", () => {
    expect(centroid([{ x: 0, y: 0 }, { x: 4, y: 2 }])).toEqual({ x: 2, y: 1 });
    expect(lerpPoint({ x: 0, y: 0 }, { x: 10, y: 4 }, 0.5)).toEqual({ x: 5, y: 2 });
  });

  it("hashes strings stably into [0, 1)", () => {
    expect(hashUnit("mem_1")).toBe(hashUnit("mem_1"));
    expect(hashUnit("mem_1")).not.toBe(hashUnit("mem_2"));
    for (const id of ["", "a", "zzzz"]) {
      expect(hashUnit(id)).toBeGreaterThanOrEqual(0);
      expect(hashUnit(id)).toBeLessThan(1);
    }
  });
});
