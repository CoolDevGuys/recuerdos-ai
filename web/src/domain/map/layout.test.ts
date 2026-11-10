import { describe, expect, it } from "vitest";
import { memory } from "../../api/fake";
import type { MapData } from "../../api/types";
import { layoutMap } from "./layout";

function data(): MapData {
  return {
    generation: "1",
    minWeight: 0.15,
    edgesOmitted: false,
    nodes: [
      memory({ id: "a", content: "Vitest for unit tests", subcategory: "testing" }),
      memory({ id: "b", content: "TDD first", subcategory: "testing" }),
      memory({ id: "c", content: "pnpm" }),
      memory({ id: "d", content: "SQLite over Postgres", category: "decision" }),
    ],
    edges: [
      { kind: "similar", source: "a", target: "b", weight: 0.9 },
      { kind: "relation", source: "c", target: "d", predicate: "uses", entity: "db" },
      { kind: "similar", source: "a", target: "gone", weight: 0.9 },
    ],
    categories: [
      { id: "preference.coding", label: "Coding preference", built_in: true, count: 3, subcategories: [{ name: "testing", count: 2 }] },
      { id: "decision", label: "Decision", built_in: true, count: 1, subcategories: [] },
    ],
    categoryEdges: [],
  };
}

const distance = (p: { x: number; y: number }, q: { x: number; y: number }) => Math.hypot(p.x - q.x, p.y - q.y);

describe("layoutMap", () => {
  it("places every memory, groups them, and drops edges to missing memories", () => {
    const model = layoutMap(data(), 50);

    expect(model.memories.map((m) => m.id)).toEqual(["a", "b", "c", "d"]);
    expect(model.categories.map((c) => [c.category, c.count])).toEqual([
      ["preference.coding", 3],
      ["decision", 1],
    ]);
    expect(model.edges).toHaveLength(2);
    for (const m of model.memories) {
      expect(Number.isFinite(m.position.x) && Number.isFinite(m.position.y)).toBe(true);
    }
  });

  it("splits a category into its sub-label clusters, naming the rest", () => {
    const model = layoutMap(data(), 50);

    const coding = model.clusters.filter((c) => c.category === "preference.coding").map((c) => [c.label, c.count]);
    expect(coding).toEqual([
      ["Coding preference · other", 1],
      ["testing", 2],
    ]);
    // A category with one cluster is that category.
    expect(model.clusters.find((c) => c.category === "decision")?.label).toBe("Decision");
  });

  it("puts a group at the centroid of its members", () => {
    const model = layoutMap(data(), 50);
    const testing = model.clusterById.get("cluster:preference.coding::testing")!;
    const [a, b] = [model.byId.get("a")!, model.byId.get("b")!];

    expect(testing.position.x).toBeCloseTo((a.position.x + b.position.x) / 2);
    expect(testing.position.y).toBeCloseTo((a.position.y + b.position.y) / 2);
  });

  it("keeps strongly similar memories closer than unrelated ones", () => {
    const model = layoutMap(data(), 300);
    const [a, b, d] = ["a", "b", "d"].map((id) => model.byId.get(id)!.position);

    expect(distance(a!, b!)).toBeLessThan(distance(a!, d!));
  });

  it("is deterministic", () => {
    const first = layoutMap(data(), 100).memories.map((m) => m.position);
    const second = layoutMap(data(), 100).memories.map((m) => m.position);
    expect(second).toEqual(first);
  });

  it("handles an empty and a single-memory store", () => {
    expect(layoutMap({ ...data(), nodes: [], edges: [], categories: [] }).memories).toEqual([]);
    const one = layoutMap({ ...data(), nodes: [data().nodes[0]!], edges: [] });
    expect(one.categories).toHaveLength(1);
  });
});

describe("clusters when sub-labels are sparse", () => {
  function sparse(): MapData {
    const m = (id: string, tags: string[]) => memory({ id, content: id, tags });
    return {
      generation: "1",
      minWeight: 0.15,
      edgesOmitted: false,
      nodes: [
        m("t1", ["testing"]),
        m("t2", ["testing", "tdd"]),
        m("t3", ["testing"]),
        m("p1", ["tooling", "pnpm"]),
        m("p2", ["tooling"]),
        m("p3", ["tooling", "pnpm"]),
        m("loner", ["misc"]),
      ],
      edges: [
        { kind: "similar", source: "t1", target: "t2", weight: 0.8 },
        { kind: "similar", source: "t2", target: "t3", weight: 0.8 },
        { kind: "similar", source: "t1", target: "t3", weight: 0.7 },
        { kind: "similar", source: "p1", target: "p2", weight: 0.8 },
        { kind: "similar", source: "p2", target: "p3", weight: 0.8 },
        { kind: "similar", source: "p1", target: "p3", weight: 0.7 },
        { kind: "similar", source: "t3", target: "p1", weight: 0.2 },
      ],
      categories: [{ id: "preference.coding", label: "Coding preference", built_in: true, count: 7, subcategories: [] }],
      categoryEdges: [],
    };
  }

  it("groups by similarity and names each group after its shared tag", () => {
    const model = layoutMap(sparse(), 50);
    const groups = model.clusters.map((c) => [c.label, [...c.members].sort()]);

    expect(groups).toContainEqual(["testing", ["t1", "t2", "t3"]]);
    expect(groups).toContainEqual(["tooling", ["p1", "p2", "p3"]]);
    expect(groups).toContainEqual(["Coding preference · other", ["loner"]]);
  });

  it("forms the same groups every time", () => {
    const first = layoutMap(sparse(), 10).clusters.map((c) => c.id + c.members.join());
    const second = layoutMap(sparse(), 10).clusters.map((c) => c.id + c.members.join());
    expect(second).toEqual(first);
  });
});
