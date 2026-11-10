import { smoothstep } from "./geometry";

/**
 * Semantic zoom (web-ui-plan.md §5.2). The camera ratio (1 = the whole map
 * fits; smaller = closer) decides what is drawn:
 *
 *   ratio ≥ 0.85            categories
 *   0.85 → 0.55             categories dissolve into their clusters
 *   0.55 → 0.40             clusters
 *   0.40 → 0.20             clusters dissolve into memories
 *   ratio ≤ 0.20            memories
 *
 * Two numbers drive every node's appearance: `cluster` (0 = categories,
 * 1 = clusters) and `memory` (0 = clusters, 1 = memories).
 */
export type Level = "categories" | "clusters" | "memories";

export const LEVELS: readonly Level[] = ["categories", "clusters", "memories"];

const CLUSTER_WINDOW: [number, number] = [0.85, 0.55];
const MEMORY_WINDOW: [number, number] = [0.4, 0.2];

/** The camera ratio that shows a level fully. */
export const LEVEL_RATIO: Record<Level, number> = { categories: 1, clusters: 0.47, memories: 0.12 };

export interface ZoomMix {
  cluster: number;
  memory: number;
}

export function zoomMix(ratio: number): ZoomMix {
  return {
    cluster: smoothstep(CLUSTER_WINDOW[0], CLUSTER_WINDOW[1], ratio),
    memory: smoothstep(MEMORY_WINDOW[0], MEMORY_WINDOW[1], ratio),
  };
}

/** The level the eye is on: whichever dominates at this ratio. */
export function levelAt(ratio: number): Level {
  const { cluster, memory } = zoomMix(ratio);
  if (memory >= 0.5) return "memories";
  if (cluster >= 0.5) return "clusters";
  return "categories";
}

/** Rounds a mix so the renderer re-styles at most ~20 times per transition. */
export function quantize(mix: ZoomMix, steps = 20): ZoomMix {
  return { cluster: Math.round(mix.cluster * steps) / steps, memory: Math.round(mix.memory * steps) / steps };
}
