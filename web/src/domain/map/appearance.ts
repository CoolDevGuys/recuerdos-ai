import type { MapEdge } from "../../api/types";
import { lerpPoint, type Point } from "./geometry";
import type { GroupNode, MapModel, MemoryNode } from "./layout";
import { categoryStyle, withAlpha } from "./palette";
import type { ZoomMix } from "./zoom";

/**
 * What every node and edge looks like for a given zoom and view state —
 * the whole of the map's drawing logic, as pure functions the renderer
 * calls from its reducers (plan §5.2). Kept out of the renderer so it is
 * testable without WebGL.
 */
export interface ViewState {
  hiddenCategories: ReadonlySet<string>;
  showSimilar: boolean;
  showRelations: boolean;
  /** Similarity edges below this weight are hidden. */
  minWeight: number;
  /** Memories to emphasise (search hits, a hovered neighbourhood); `null` = none. */
  highlighted: ReadonlySet<string> | null;
  /** A memory under the pointer. */
  hovered: string | null;
}

export interface Look {
  x: number;
  y: number;
  size: number;
  color: string;
  label: string | null;
  hidden: boolean;
  forceLabel: boolean;
  zIndex: number;
}

export interface EdgeLook {
  color: string;
  size: number;
  hidden: boolean;
  type: "line" | "arrow";
}

const DIMMED = "rgba(148,163,184,0.25)";
const LABEL_LENGTH = 48;
const VISIBLE = 0.04;

function truncate(text: string, length: number): string {
  return text.length <= length ? text : `${text.slice(0, length - 1).trimEnd()}…`;
}

/** Groups step back while a search or hover singles out memories. */
function groupDimming(view: ViewState): number {
  return view.highlighted === null ? 1 : 0.35;
}

export function categoryLook(node: GroupNode, mix: ZoomMix, view: ViewState): Look {
  const alpha = (1 - mix.cluster) * groupDimming(view);
  const { color } = categoryStyle(node.category);
  return {
    ...node.position,
    size: 8 + 2.5 * Math.sqrt(node.count),
    color: withAlpha(color, alpha),
    label: `${node.label} (${node.count})`,
    hidden: alpha < VISIBLE || view.hiddenCategories.has(node.category),
    forceLabel: true,
    zIndex: 3,
  };
}

export function clusterLook(node: GroupNode, model: MapModel, mix: ZoomMix, view: ViewState): Look {
  const alpha = mix.cluster * (1 - mix.memory) * groupDimming(view);
  const category = model.categoryById.get(node.category);
  const from = category?.position ?? node.position;
  const { color } = categoryStyle(node.category);
  return {
    ...lerpPoint(from, node.position, mix.cluster),
    size: 5 + 2 * Math.sqrt(node.count),
    color: withAlpha(color, alpha * 0.9),
    label: `${node.label} (${node.count})`,
    hidden: alpha < VISIBLE || view.hiddenCategories.has(node.category),
    forceLabel: alpha > 0.5,
    zIndex: 2,
  };
}

/** Where a memory is drawn: sliding out of its cluster's centre as it dissolves. */
export function memoryPosition(node: MemoryNode, model: MapModel, mix: ZoomMix): Point {
  const cluster = model.clusterById.get(node.cluster);
  const category = model.categoryById.get(node.category);
  const groupCentre = cluster && category ? lerpPoint(category.position, cluster.position, mix.cluster) : node.position;
  return lerpPoint(groupCentre, node.position, mix.memory);
}

export function memoryLook(node: MemoryNode, model: MapModel, mix: ZoomMix, view: ViewState): Look {
  const { color } = categoryStyle(node.category);
  // A highlighted memory shows at any zoom, in its true place, so a search
  // over the whole map shows where its answers sit among the categories.
  if (view.highlighted?.has(node.id) === true && !view.hiddenCategories.has(node.category)) {
    return {
      ...node.position,
      size: 5 + 3 * node.memory.confidence,
      color: withAlpha(color, 1),
      label: truncate(node.memory.content, LABEL_LENGTH),
      hidden: false,
      forceLabel: true,
      zIndex: 4,
    };
  }
  const emphasised = view.highlighted === null || view.highlighted.has(node.id) || view.hovered === node.id;
  const alpha = mix.memory;
  return {
    ...memoryPosition(node, model, mix),
    size: 3 + 3 * node.memory.confidence,
    color: emphasised ? withAlpha(color, alpha) : DIMMED,
    label: emphasised ? truncate(node.memory.content, LABEL_LENGTH) : null,
    hidden: alpha < VISIBLE || view.hiddenCategories.has(node.category),
    forceLabel: view.hovered === node.id || (view.highlighted?.has(node.id) ?? false),
    zIndex: emphasised ? 1 : 0,
  };
}

export function edgeLook(edge: MapEdge, model: MapModel, mix: ZoomMix, view: ViewState): EdgeLook {
  const source = model.byId.get(edge.source);
  const target = model.byId.get(edge.target);
  const filtered =
    source === undefined ||
    target === undefined ||
    view.hiddenCategories.has(source.category) ||
    view.hiddenCategories.has(target.category);
  const touchesFocus =
    view.highlighted === null ||
    view.highlighted.has(edge.source) ||
    view.highlighted.has(edge.target) ||
    view.hovered === edge.source ||
    view.hovered === edge.target;

  if (edge.kind === "relation") {
    return {
      color: withAlpha("#d97706", mix.memory * (touchesFocus ? 0.55 : 0.08)),
      size: 1,
      hidden: filtered || !view.showRelations || mix.memory < 0.3,
      type: "arrow",
    };
  }
  return {
    color: withAlpha("#94a3b8", mix.memory * (touchesFocus ? 0.12 + 0.45 * edge.weight : 0.05)),
    size: 0.3 + 1.4 * edge.weight,
    hidden: filtered || !view.showSimilar || edge.weight < view.minWeight || mix.memory < 0.3,
    type: "line",
  };
}

export function categoryEdgeLook(
  edge: { source: string; target: string; weight: number },
  mix: ZoomMix,
  view: ViewState,
): EdgeLook {
  return {
    color: withAlpha("#94a3b8", (1 - mix.cluster) * 0.7),
    size: 1 + 6 * Math.min(1, edge.weight),
    hidden: mix.cluster > 0.9 || view.hiddenCategories.has(edge.source) || view.hiddenCategories.has(edge.target),
    type: "line",
  };
}

/** The memories adjacent to `id` over the edges currently shown — the hover neighbourhood. */
export function neighbourhood(id: string, model: MapModel, view: ViewState): Set<string> {
  const result = new Set([id]);
  for (const edge of model.edges) {
    if (edge.kind === "similar" && (!view.showSimilar || edge.weight < view.minWeight)) continue;
    if (edge.kind === "relation" && !view.showRelations) continue;
    if (edge.source === id) result.add(edge.target);
    if (edge.target === id) result.add(edge.source);
  }
  return result;
}
