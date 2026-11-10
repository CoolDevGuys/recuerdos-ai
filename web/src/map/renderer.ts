import type { ViewState } from "../domain/map/appearance";
import type { MapModel } from "../domain/map/layout";
import type { Level } from "../domain/map/zoom";

/**
 * The map's drawing surface, as the React side sees it. The real one wraps
 * Sigma (WebGL, ./sigmaRenderer.ts) and is loaded lazily; tests substitute a
 * recording fake, since jsdom has no WebGL.
 */
export interface MapRenderer {
  /** Re-style for a changed view (filters, highlight). */
  update(view: ViewState): void;
  /** Move the camera so these memories fill the view. */
  focus(ids: readonly string[]): void;
  zoomTo(level: Level): void;
  zoomIn(): void;
  zoomOut(): void;
  reset(): void;
  destroy(): void;
}

export interface RendererCallbacks {
  onOpenMemory(id: string): void;
  onLevelChange(level: Level): void;
}

export type RendererFactory = (
  container: HTMLElement,
  model: MapModel,
  view: ViewState,
  callbacks: RendererCallbacks,
) => MapRenderer;

export type RendererLoader = () => Promise<RendererFactory>;

export const loadSigmaRenderer: RendererLoader = () => import("./sigmaRenderer").then((m) => m.createSigmaRenderer);
