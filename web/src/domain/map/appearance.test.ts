import { describe, expect, it } from "vitest";
import { memory } from "../../api/fake";
import type { MapData } from "../../api/types";
import { categoryLook, clusterLook, edgeLook, memoryLook, memoryPosition, neighbourhood, type ViewState } from "./appearance";
import { layoutMap } from "./layout";

const model = layoutMap(
  {
    generation: "1",
    minWeight: 0.15,
    edgesOmitted: false,
    detail: "memories",
    total: 4,
    category: null,
    nodes: [
      memory({ id: "a", content: "Vitest for unit tests", subcategory: "testing" }),
      memory({ id: "b", content: "TDD first", subcategory: "testing" }),
      memory({ id: "d", content: "SQLite over Postgres", category: "decision" }),
    ],
    edges: [
      { kind: "similar", source: "a", target: "b", weight: 0.8 },
      { kind: "similar", source: "a", target: "d", weight: 0.2 },
      { kind: "relation", source: "b", target: "d", predicate: "uses", entity: "db" },
    ],
    categories: [
      { id: "preference.coding", label: "Coding preference", built_in: true, count: 2, subcategories: [{ name: "testing", count: 2 }] },
      { id: "decision", label: "Decision", built_in: true, count: 1, subcategories: [] },
    ],
    categoryEdges: [{ source: "decision", target: "preference.coding", weight: 0.3 }],
  } satisfies MapData,
  { iterations: 50 },
);

const view: ViewState = {
  hiddenCategories: new Set(),
  showSimilar: true,
  showRelations: true,
  minWeight: 0.15,
  highlighted: null,
  hovered: null,
};

const OUT = { cluster: 0, memory: 0 };
const MIDDLE = { cluster: 1, memory: 0 };
const IN = { cluster: 1, memory: 1 };

describe("zoomed out", () => {
  it("shows categories with their counts and hides memories and their edges", () => {
    const coding = categoryLook(model.categories[0]!, OUT, view);
    expect(coding.hidden).toBe(false);
    expect(coding.label).toBe("Coding preference (2)");
    expect(memoryLook(model.byId.get("a")!, model, OUT, view).hidden).toBe(true);
    expect(edgeLook(model.edges[0]!, model, OUT, view).hidden).toBe(true);
  });
});

describe("dissolving", () => {
  it("fades categories out as clusters fade in", () => {
    const cluster = model.clusterById.get("cluster:preference.coding::testing")!;
    expect(clusterLook(cluster, model, OUT, view).hidden).toBe(true);
    expect(clusterLook(cluster, model, MIDDLE, view).hidden).toBe(false);
    expect(categoryLook(model.categories[0]!, MIDDLE, view).hidden).toBe(true);
  });

  it("slides each memory from its cluster's centre to its own place", () => {
    const a = model.byId.get("a")!;
    const cluster = model.clusterById.get(a.cluster)!;
    expect(memoryPosition(a, model, MIDDLE)).toEqual(cluster.position);
    expect(memoryPosition(a, model, IN)).toEqual(a.position);
    const halfway = memoryPosition(a, model, { cluster: 1, memory: 0.5 });
    expect(halfway.x).toBeCloseTo((cluster.position.x + a.position.x) / 2);
  });
});

describe("zoomed in", () => {
  it("shows memories labelled with their text, and their edges", () => {
    const look = memoryLook(model.byId.get("a")!, model, IN, view);
    expect(look.hidden).toBe(false);
    expect(look.label).toBe("Vitest for unit tests");
    expect(edgeLook(model.edges[0]!, model, IN, view).hidden).toBe(false);
  });

  it("draws relations differently from similarity, not by colour alone", () => {
    expect(edgeLook(model.edges[2]!, model, IN, view).type).toBe("arrow");
    expect(edgeLook(model.edges[0]!, model, IN, view).type).toBe("line");
  });

  it("thickens stronger similarity", () => {
    expect(edgeLook(model.edges[0]!, model, IN, view).size).toBeGreaterThan(edgeLook(model.edges[1]!, model, IN, view).size);
  });
});

describe("filters and focus", () => {
  it("hides a filtered-out category everywhere", () => {
    const filtered = { ...view, hiddenCategories: new Set(["decision"]) };
    expect(categoryLook(model.categoryById.get("decision")!, OUT, filtered).hidden).toBe(true);
    expect(memoryLook(model.byId.get("d")!, model, IN, filtered).hidden).toBe(true);
    expect(edgeLook(model.edges[2]!, model, IN, filtered).hidden).toBe(true);
  });

  it("hides similarity below the chosen floor and edge kinds turned off", () => {
    expect(edgeLook(model.edges[1]!, model, IN, { ...view, minWeight: 0.5 }).hidden).toBe(true);
    expect(edgeLook(model.edges[2]!, model, IN, { ...view, showRelations: false }).hidden).toBe(true);
  });

  it("dims what is not highlighted and drops its label", () => {
    const focused = { ...view, highlighted: new Set(["a"]) };
    const a = memoryLook(model.byId.get("a")!, model, IN, focused);
    const d = memoryLook(model.byId.get("d")!, model, IN, focused);
    expect(a.forceLabel).toBe(true);
    expect(d.label).toBeNull();
    expect(d.zIndex).toBeLessThan(a.zIndex);
  });

  it("finds a memory's neighbourhood over the edges shown", () => {
    expect(neighbourhood("a", model, view)).toEqual(new Set(["a", "b", "d"]));
    expect(neighbourhood("a", model, { ...view, minWeight: 0.5 })).toEqual(new Set(["a", "b"]));
  });
});

describe("a highlighted memory", () => {
  it("shows at any zoom, in its true place, while groups step back", () => {
    const focused = { ...view, highlighted: new Set(["d"]) };
    const d = model.byId.get("d")!;

    const look = memoryLook(d, model, OUT, focused);

    expect(look.hidden).toBe(false);
    expect({ x: look.x, y: look.y }).toEqual(d.position);
    expect(look.forceLabel).toBe(true);
    expect(memoryLook(model.byId.get("a")!, model, OUT, focused).hidden).toBe(true);
    expect(categoryLook(model.categories[0]!, OUT, focused).color).toMatch(/,0\.35\)$/);
  });

  it("in a lasso selection shows as a dot, labelled only under the pointer", () => {
    const selected = { ...view, highlighted: new Set(["d"]), labelHighlighted: false };
    const d = model.byId.get("d")!;

    const look = memoryLook(d, model, OUT, selected);
    const hovered = memoryLook(d, model, OUT, { ...selected, hovered: "d" });

    expect(look.hidden).toBe(false);
    expect([look.label, look.forceLabel]).toEqual([null, false]);
    expect(hovered.forceLabel).toBe(true);
  });
});
