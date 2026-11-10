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

export function categoryNodeId(category: string): string {
  return `category:${category}`;
}

export function clusterNodeId(category: string, subcategory: string | null): string {
  return `cluster:${category}::${subcategory ?? ""}`;
}

export function layoutMap(data: MapData, iterations?: number): MapModel {
  const labels = new Map(data.categories.map((c) => [c.id, c.label]));
  const order = [...data.categories].sort((a, b) => b.count - a.count || a.id.localeCompare(b.id)).map((c) => c.id);
  // Memories may name a category the summary missed (it should not happen); give them a slot too.
  for (const m of data.nodes) if (!order.includes(m.category)) order.push(m.category);

  // Ring of anchors, sized to the store so categories do not overlap.
  const ring = order.length <= 1 ? 0 : 6 * Math.sqrt(data.nodes.length) + 10;
  const anchors = new Map<string, Point>(
    order.map((id, i) => {
      const angle = (2 * Math.PI * i) / Math.max(1, order.length) - Math.PI / 2;
      return [id, { x: ring * Math.cos(angle), y: ring * Math.sin(angle) }];
    }),
  );
  const sizes = new Map<string, number>();
  for (const m of data.nodes) sizes.set(m.category, (sizes.get(m.category) ?? 0) + 1);

  const graph = new Graph({ type: "undirected", multi: false });
  for (const [id, anchor] of anchors) {
    graph.addNode(`anchor:${id}`, { x: anchor.x, y: anchor.y, fixed: true, size: 1 });
  }
  for (const m of data.nodes) {
    const anchor = anchors.get(m.category)!;
    const spread = 2 * Math.sqrt(sizes.get(m.category) ?? 1) + 1;
    const angle = 2 * Math.PI * hashUnit(m.id);
    const radius = spread * Math.sqrt(hashUnit(`${m.id}:r`));
    graph.addNode(m.id, { x: anchor.x + radius * Math.cos(angle), y: anchor.y + radius * Math.sin(angle), size: 1 });
    graph.addEdge(m.id, `anchor:${m.category}`, { weight: ANCHOR_WEIGHT });
  }
  const categoryOf = new Map(data.nodes.map((m) => [m.id, m.category]));
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
  if (n > 1) {
    forceAtlas2.assign(graph, {
      iterations: iterations ?? (n < 300 ? 400 : n < 1500 ? 200 : 100),
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
    category: m.category,
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

interface Assignment {
  id: string;
  label: string;
}

/** Which cluster each memory belongs to, and what the cluster is called. */
function assignClusters(data: MapData, labels: Map<string, string>): Map<string, Assignment> {
  const result = new Map<string, Assignment>();
  const byCategory = new Map<string, Memory[]>();
  for (const m of data.nodes) byCategory.set(m.category, [...(byCategory.get(m.category) ?? []), m]);

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
