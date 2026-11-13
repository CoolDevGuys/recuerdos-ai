import Graph from "graphology";
import louvain from "graphology-communities-louvain";
import forceAtlas2 from "graphology-layout-forceatlas2";
import type { MapData, MapEdge, Memory } from "../../api/types";
import { centroid, hashUnit, type Point } from "./geometry";

/**
 * Where everything sits on the map (plan §5.1). Positions are computed once
 * per map answer and are deterministic: seeds come from id hashes, and
 * ForceAtlas2 is deterministic given its seeds and iteration count, so the
 * same memories always land in the same places.
 *
 * Each category is anchored at a fixed point on a ring, ordered by size,
 * and its memories are tied to that anchor. Similarity inside a category
 * pulls at full strength and across categories at a quarter: categories
 * stay compact, recognisable islands, related islands drift together, and
 * related memories gather inside each island.
 *
 * # Clusters: the middle zoom level
 *
 * A category splits by its sub-labels when most of its memories have one
 * (at least {@link SUBLABEL_SHARE}). Otherwise sub-labels are too sparse to
 * mean much — on a real store 53 of 72 coding preferences had none — and it
 * splits by similarity instead: Louvain communities over the similarity
 * edges inside the category, each named after the tag its members share
 * most. Memories that join no community are gathered as "other".
 *
 * # Warm starts
 *
 * A {@link LayoutSeed} — the positions of the last layout of the same
 * scope, kept by the browser — pins every memory that is still there and
 * still in the same category. Only new or re-filed memories are laid out,
 * so reopening the map, or opening it after one edit, does not reshuffle
 * it. Each category keeps its anchor from the seed, too, even when sizes
 * change the ring order. A seed that lacks a category now present (one
 * was created, or a memory was filed under a new one) is ignored: there is
 * no free place on the old ring to put it.
 *
 * # Aggregate answers
 *
 * For a store above the daemon's detail limit the answer has categories
 * and no memories. {@link layoutMap} then places the category discs alone,
 * related ones pulled together by their category links.
 */
export interface MemoryNode {
  id: string;
  memory: Memory;
  category: string;
  cluster: string;
  /** Where the memory itself sits. */
  position: Point;
}

export interface GroupNode {
  /** `category:<id>` or `cluster:<category>::<sub-label>`. */
  id: string;
  category: string;
  label: string;
  count: number;
  /** The centroid of its members' positions. */
  position: Point;
  members: string[];
}

export interface MapModel {
  /** Categories only: the store is above the detail limit (see {@link MapData.detail}). */
  aggregate: boolean;
  /** The category the map is limited to, or `null`. */
  scope: string | null;
  /** Memories in scope, drawn or not. */
  total: number;
  /** The radius of the category ring, kept in a {@link LayoutSeed}. */
  ring: number;
  /** Where each category's memories are tied, kept in a {@link LayoutSeed}. */
  anchors: Map<string, [number, number]>;
  memories: MemoryNode[];
  categories: GroupNode[];
  clusters: GroupNode[];
  edges: MapEdge[];
  categoryEdges: MapData["categoryEdges"];
  minWeight: number;
  byId: Map<string, MemoryNode>;
  clusterById: Map<string, GroupNode>;
  categoryById: Map<string, GroupNode>;
}

const ANCHOR_WEIGHT = 2.5;
const RELATION_WEIGHT = 0.3;
/** How much similarity across categories pulls, relative to within. */
const CROSS_CATEGORY = 0.25;
/** The share of a category's memories that must carry a sub-label for sub-labels to define its clusters. */
export const SUBLABEL_SHARE = 0.6;

/**
 * The group a memory is drawn in: its topic (ai-specs/memory-structure-plan.md
 * D13: group by topic, colour by kind). The map code calls a group a
 * "category", from before topics existed; it is the topic everywhere here.
 */
export function groupOf(memory: Memory): string {
  return memory.topic;
}

export function categoryNodeId(category: string): string {
  return `category:${category}`;
}

export function clusterNodeId(category: string, subcategory: string | null): string {
  return `cluster:${category}::${subcategory ?? ""}`;
}

/**
 * Where a previous layout put things, to start the next one from. Plain
 * data, so it can be stored as JSON.
 */
export interface LayoutSeed {
  /** Each category's anchor: `[x, y]`. */
  anchors: Record<string, [number, number]>;
  ring: number;
  /** Memory id → `[x, y, category]`. */
  positions: Record<string, [number, number, string]>;
}

export interface LayoutOptions {
  /** ForceAtlas2 iterations for a cold layout; the default scales with size. */
  iterations?: number;
  seed?: LayoutSeed | null;
}

/** What to keep of a layout to start the next one from. */
export function seedOf(model: MapModel): LayoutSeed {
  const positions: LayoutSeed["positions"] = {};
  for (const m of model.memories) positions[m.id] = [m.position.x, m.position.y, m.category];
  return { anchors: Object.fromEntries(model.anchors), ring: model.ring, positions };
}

function ringOrder(data: MapData): string[] {
  const order = [...data.categories].sort((a, b) => b.count - a.count || a.id.localeCompare(b.id)).map((c) => c.id);
  // Memories may name a category the summary missed (it should not happen); give them a slot too.
  for (const m of data.nodes) if (!order.includes(groupOf(m))) order.push(groupOf(m));
  return order;
}

export function layoutMap(data: MapData, options: LayoutOptions = {}): MapModel {
  if (data.detail === "categories") return layoutCategories(data);
  const labels = new Map(data.categories.map((c) => [c.id, c.label]));
  const order = ringOrder(data);
  const usable = options.seed != null && order.every((id) => options.seed!.anchors[id] !== undefined) ? options.seed : null;

  // Ring of anchors, sized to the store so categories do not overlap. A
  // usable seed keeps its ring, so pinned memories stay round their anchors.
  const ring = usable?.ring ?? (order.length <= 1 ? 0 : 6 * Math.sqrt(data.nodes.length) + 10);
  const anchors = new Map<string, Point>(
    order.map((id, i) => {
      const kept = usable?.anchors[id];
      if (kept !== undefined) return [id, { x: kept[0], y: kept[1] }];
      const angle = (2 * Math.PI * i) / Math.max(1, order.length) - Math.PI / 2;
      return [id, { x: ring * Math.cos(angle), y: ring * Math.sin(angle) }];
    }),
  );
  const sizes = new Map<string, number>();
  for (const m of data.nodes) sizes.set(groupOf(m), (sizes.get(groupOf(m)) ?? 0) + 1);

  const graph = new Graph({ type: "undirected", multi: false });
  for (const [id, anchor] of anchors) {
    graph.addNode(`anchor:${id}`, { x: anchor.x, y: anchor.y, fixed: true, size: 1 });
  }
  let released = 0;
  for (const m of data.nodes) {
    const kept = usable?.positions[m.id];
    if (kept !== undefined && kept[2] === groupOf(m)) {
      graph.addNode(m.id, { x: kept[0], y: kept[1], fixed: true, size: 1 });
    } else {
      const anchor = anchors.get(groupOf(m))!;
      const spread = 2 * Math.sqrt(sizes.get(groupOf(m)) ?? 1) + 1;
      const angle = 2 * Math.PI * hashUnit(m.id);
      const radius = spread * Math.sqrt(hashUnit(`${m.id}:r`));
      graph.addNode(m.id, { x: anchor.x + radius * Math.cos(angle), y: anchor.y + radius * Math.sin(angle), size: 1 });
      released += 1;
    }
    graph.addEdge(m.id, `anchor:${groupOf(m)}`, { weight: ANCHOR_WEIGHT });
  }
  const categoryOf = new Map(data.nodes.map((m) => [m.id, groupOf(m)]));
  for (const edge of data.edges) {
    if (!graph.hasNode(edge.source) || !graph.hasNode(edge.target) || edge.source === edge.target) continue;
    const across = categoryOf.get(edge.source) !== categoryOf.get(edge.target) ? CROSS_CATEGORY : 1;
    const weight = (edge.kind === "similar" ? edge.weight : RELATION_WEIGHT) * across;
    if (graph.hasEdge(edge.source, edge.target)) {
      graph.updateEdgeAttribute(edge.source, edge.target, "weight", (w: number | undefined) => Math.max(w ?? 0, weight));
    } else {
      graph.addEdge(edge.source, edge.target, { weight });
    }
  }

  const n = data.nodes.length;
  const cold = n < 300 ? 400 : n < 1500 ? 200 : 100;
  // Warm, with a handful released (an edit or two): place each one by its
  // pinned neighbours instead of running the force layout, whose cost is
  // the whole store's however few nodes move. With more released, a
  // shorter force layout moves only them.
  const few = usable !== null && released <= Math.max(10, n / 50);
  if (few && released > 0) placeByNeighbours(graph);
  const iterations = usable === null ? (options.iterations ?? cold) : few ? 0 : Math.min(cold, 150);
  if (n > 1 && iterations > 0) {
    forceAtlas2.assign(graph, {
      iterations,
      getEdgeWeight: "weight",
      settings: {
        gravity: 0.5,
        scalingRatio: 4,
        slowDown: 4,
        edgeWeightInfluence: 1,
        barnesHutOptimize: n > 800,
      },
    });
  }

  const assigned = assignClusters(data, labels);
  const memories: MemoryNode[] = data.nodes.map((m) => ({
    id: m.id,
    memory: m,
    category: groupOf(m),
    cluster: assigned.get(m.id)!.id,
    position: { x: graph.getNodeAttribute(m.id, "x") as number, y: graph.getNodeAttribute(m.id, "y") as number },
  }));
  const byId = new Map(memories.map((m) => [m.id, m]));

  const group = (id: string, category: string, label: string, members: MemoryNode[]): GroupNode => ({
    id,
    category,
    label,
    count: members.length,
    position: centroid(members.map((m) => m.position)),
    members: members.map((m) => m.id),
  });

  const categories = order
    .map((id) => {
      const members = memories.filter((m) => m.category === id);
      return group(categoryNodeId(id), id, labels.get(id) ?? id, members);
    })
    .filter((c) => c.count > 0);

  const clusterMembers = new Map<string, MemoryNode[]>();
  for (const m of memories) clusterMembers.set(m.cluster, [...(clusterMembers.get(m.cluster) ?? []), m]);
  const clusters = [...clusterMembers].map(([id, members]) => {
    const first = members[0]!;
    return group(id, first.category, assigned.get(first.id)!.label, members);
  });
  clusters.sort((a, b) => a.id.localeCompare(b.id));
  // A category with a single cluster has nothing to split into: the cluster is the category.
  for (const cluster of clusters) {
    if (clusters.filter((c) => c.category === cluster.category).length === 1) {
      cluster.label = labels.get(cluster.category) ?? cluster.category;
    }
  }

  return {
    aggregate: false,
    scope: data.category,
    total: data.total,
    ring,
    anchors: new Map([...anchors].map(([id, p]) => [id, [p.x, p.y] as [number, number]])),
    memories,
    categories,
    clusters,
    edges: data.edges.filter((e) => byId.has(e.source) && byId.has(e.target)),
    categoryEdges: data.categoryEdges,
    minWeight: data.minWeight,
    byId,
    clusterById: new Map(clusters.map((c) => [c.id, c])),
    categoryById: new Map(categories.map((c) => [c.category, c])),
  };
}

/**
 * The categories-only layout of an aggregate answer: one disc per category
 * on a ring ordered by size, then pulled by the category links so related
 * categories sit together. Discs carry their counts; there are no members.
 */
function layoutCategories(data: MapData): MapModel {
  const order = ringOrder(data);
  const ring = order.length <= 1 ? 0 : 6 * Math.sqrt(data.total) + 10;
  const graph = new Graph({ type: "undirected", multi: false });
  order.forEach((id, i) => {
    const angle = (2 * Math.PI * i) / Math.max(1, order.length) - Math.PI / 2;
    graph.addNode(id, { x: ring * Math.cos(angle), y: ring * Math.sin(angle), size: 1 });
  });
  for (const edge of data.categoryEdges) {
    if (graph.hasNode(edge.source) && graph.hasNode(edge.target) && edge.source !== edge.target && !graph.hasEdge(edge.source, edge.target)) {
      graph.addEdge(edge.source, edge.target, { weight: edge.weight });
    }
  }
  if (order.length > 2 && graph.size > 0) {
    forceAtlas2.assign(graph, {
      iterations: 200,
      getEdgeWeight: "weight",
      settings: { gravity: 1, scalingRatio: ring, slowDown: 2, edgeWeightInfluence: 1 },
    });
  }
  separate(graph, order, new Map(data.categories.map((c) => [c.id, c.label])), ring);
  const categories: GroupNode[] = data.categories
    .filter((c) => c.count > 0)
    .map((c) => ({
      id: categoryNodeId(c.id),
      category: c.id,
      label: c.label,
      count: c.count,
      position: { x: graph.getNodeAttribute(c.id, "x") as number, y: graph.getNodeAttribute(c.id, "y") as number },
      members: [],
    }));
  return {
    aggregate: true,
    scope: data.category,
    total: data.total,
    ring,
    anchors: new Map(),
    memories: [],
    categories,
    clusters: [],
    edges: [],
    categoryEdges: data.categoryEdges,
    minWeight: data.minWeight,
    byId: new Map(),
    clusterById: new Map(),
    categoryById: new Map(categories.map((c) => [c.category, c])),
  };
}

/**
 * Puts each free memory at the weighted centre of its pinned neighbours,
 * pulled a third of the way towards its category's anchor and nudged by its
 * id so two new memories with the same neighbours do not coincide. A free
 * memory with no pinned neighbour keeps its seeded spot near the anchor.
 */
function placeByNeighbours(graph: Graph): void {
  graph.forEachNode((node, attributes) => {
    if (attributes.fixed === true) return;
    let x = 0;
    let y = 0;
    let total = 0;
    let anchor: Point | null = null;
    graph.forEachEdge(node, (_edge, edge, source, target) => {
      const other = source === node ? target : source;
      const at = graph.getNodeAttributes(other);
      if (other.startsWith("anchor:")) {
        anchor = { x: at.x as number, y: at.y as number };
        return;
      }
      if (at.fixed !== true) return;
      const weight = (edge.weight as number | undefined) ?? 1;
      x += (at.x as number) * weight;
      y += (at.y as number) * weight;
      total += weight;
    });
    if (total === 0) return;
    const centre = { x: x / total, y: y / total };
    const pull: Point = anchor ?? centre;
    const angle = 2 * Math.PI * hashUnit(node);
    graph.mergeNodeAttributes(node, {
      x: centre.x + (pull.x - centre.x) / 3 + 0.5 * Math.cos(angle),
      y: centre.y + (pull.y - centre.y) / 3 + 0.5 * Math.sin(angle),
    });
  });
}

/**
 * Pushes category discs apart until neither they nor their labels overlap:
 * the force layout pulls related categories together, and discs and labels
 * are drawn at a fixed pixel size whatever the zoom.
 *
 * Each category is a box: its disc, plus its label to the right. Sizes are
 * estimated for a map about 700 px wide showing the whole ring, which is
 * the narrowest the layout is shown at on a laptop; wider maps only have
 * more room. Overlapping boxes move apart along the axis they overlap least.
 */
function separate(graph: Graph, ids: readonly string[], labels: ReadonlyMap<string, string>, ring: number): void {
  if (ids.length < 2 || ring <= 0) return;
  const unit = (2.6 * ring) / 700; // graph units per pixel
  const boxes = ids.map((id) => {
    const text = `${labels.get(id) ?? id} (0000)`;
    return {
      x: graph.getNodeAttribute(id, "x") as number,
      y: graph.getNodeAttribute(id, "y") as number,
      left: 16 * unit,
      right: (20 + 7 * text.length) * unit,
      half: 13 * unit,
    };
  });
  for (let pass = 0; pass < 200; pass += 1) {
    let moved = false;
    for (let i = 0; i < boxes.length; i += 1) {
      for (let j = i + 1; j < boxes.length; j += 1) {
        const [a, b] = [boxes[i]!, boxes[j]!];
        const [first, second] = a.x <= b.x ? [a, b] : [b, a];
        const overlapX = first.x + first.right - (second.x - second.left);
        const overlapY = a.half + b.half - Math.abs(a.y - b.y);
        if (overlapX <= 0 || overlapY <= 0) continue;
        if (overlapY <= overlapX) {
          const [upper, lower] = a.y <= b.y ? [a, b] : [b, a];
          // Coincident rows split by id order, so the result is deterministic.
          upper.y -= overlapY / 2 + 1e-6;
          lower.y += overlapY / 2 + 1e-6;
        } else {
          first.x -= overlapX / 2 + 1e-6;
          second.x += overlapX / 2 + 1e-6;
        }
        moved = true;
      }
    }
    if (!moved) break;
  }
  ids.forEach((id, i) => graph.mergeNodeAttributes(id, { x: boxes[i]!.x, y: boxes[i]!.y }));
}

interface Assignment {
  id: string;
  label: string;
}

/** Which cluster each memory belongs to, and what the cluster is called. */
function assignClusters(data: MapData, labels: Map<string, string>): Map<string, Assignment> {
  const result = new Map<string, Assignment>();
  const byCategory = new Map<string, Memory[]>();
  for (const m of data.nodes) byCategory.set(groupOf(m), [...(byCategory.get(groupOf(m)) ?? []), m]);

  for (const [category, members] of byCategory) {
    const other = { id: clusterNodeId(category, null), label: `${labels.get(category) ?? category} · other` };
    const labelled = members.filter((m) => m.subcategory !== null).length;

    if (labelled / members.length >= SUBLABEL_SHARE) {
      for (const m of members) {
        result.set(m.id, m.subcategory === null ? other : { id: clusterNodeId(category, m.subcategory), label: m.subcategory });
      }
      continue;
    }

    const communities = similarityCommunities(category, members, data.edges);
    // Largest first, so names and ids are stable for a given store.
    const groups = [...communities.values()].filter((g) => g.length > 1).sort((a, b) => b.length - a.length || a[0]!.id.localeCompare(b[0]!.id));
    const used = new Set<string>();
    groups.forEach((group, index) => {
      const assignment = { id: `cluster:${category}::~${index + 1}`, label: nameOf(group, used) ?? `Group ${index + 1}` };
      for (const m of group) result.set(m.id, assignment);
    });
    for (const m of members) if (!result.has(m.id)) result.set(m.id, other);
  }
  return result;
}

function similarityCommunities(category: string, members: Memory[], edges: readonly MapEdge[]): Map<string, Memory[]> {
  const graph = new Graph({ type: "undirected" });
  const ids = new Set(members.map((m) => m.id));
  for (const m of members) graph.addNode(m.id);
  for (const edge of edges) {
    if (edge.kind !== "similar" || !ids.has(edge.source) || !ids.has(edge.target)) continue;
    if (!graph.hasEdge(edge.source, edge.target)) graph.addEdge(edge.source, edge.target, { weight: edge.weight });
  }
  const groups = new Map<string, Memory[]>();
  if (graph.size === 0) return groups;
  const community = louvain(graph, { getEdgeWeight: "weight", rng: seededRandom(category) });
  for (const m of members) {
    // Louvain puts an isolated memory in a community of its own; those are "other".
    if (graph.degree(m.id) === 0) continue;
    const key = String(community[m.id]);
    groups.set(key, [...(groups.get(key) ?? []), m]);
  }
  return groups;
}

/** A community's name: the tag most of its members share (one not already used by a sibling). */
function nameOf(group: Memory[], used: Set<string>): string | null {
  const counts = new Map<string, number>();
  for (const m of group) for (const tag of new Set(m.tags)) counts.set(tag, (counts.get(tag) ?? 0) + 1);
  const ranked = [...counts].filter(([tag, n]) => n >= 2 && !used.has(tag)).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  const best = ranked[0]?.[0];
  if (best === undefined) return null;
  used.add(best);
  return best;
}

/** A small deterministic PRNG (mulberry32) seeded from a string, so communities are stable. */
function seededRandom(seed: string): () => number {
  let state = Math.floor(hashUnit(seed) * 0x100000000);
  return () => {
    state = (state + 0x6d2b79f5) | 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
