import { beforeEach, describe, expect, it, vi } from "vitest";
import { clearLayoutSeeds, readLayoutSeed, writeLayoutSeed } from "./layoutCache";

const seed = { anchors: { skill: [1, 2] as [number, number] }, ring: 12, positions: { m1: [3, 4, "skill"] as [number, number, string] } };

describe("the layout cache", () => {
  beforeEach(() => window.localStorage.clear());

  it("reads back what it wrote, per user and scope", () => {
    writeLayoutSeed("alex", null, seed);
    writeLayoutSeed("alex", "skill", { ...seed, ring: 3 });

    expect(readLayoutSeed("alex", null)).toEqual(seed);
    expect(readLayoutSeed("alex", "skill")?.ring).toBe(3);
    expect(readLayoutSeed("sam", null)).toBeNull();
  });

  it("treats anything malformed as no layout", () => {
    window.localStorage.setItem("recuerdos.map-layout.v1:alex:*", "{not json");
    expect(readLayoutSeed("alex", null)).toBeNull();

    window.localStorage.setItem("recuerdos.map-layout.v1:alex:*", JSON.stringify({ ...seed, positions: { m1: ["x", 4, "skill"] } }));
    expect(readLayoutSeed("alex", null)).toBeNull();
  });

  it("clears only its own keys", () => {
    writeLayoutSeed("alex", null, seed);
    window.localStorage.setItem("other", "kept");

    clearLayoutSeeds();

    expect(readLayoutSeed("alex", null)).toBeNull();
    expect(window.localStorage.getItem("other")).toBe("kept");
  });

  it("survives storage that refuses writes", () => {
    const spy = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("full", "QuotaExceededError");
    });

    expect(() => writeLayoutSeed("alex", null, seed)).not.toThrow();
    spy.mockRestore();
  });
});
