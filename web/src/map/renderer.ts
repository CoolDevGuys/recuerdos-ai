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
  /** Move the camera by a fraction of the view's width and height (keyboard panning). */
  pan(dx: number, dy: number): void;
  /**
   * In lasso mode a drag draws a selection outline instead of panning. Shift
   * plus drag lassos in either mode.
   */
  setLassoMode(on: boolean): void;
  /** Where the camera is now, to start the next renderer of the same map there. */
  camera(): CameraView | null;
  destroy(): void;
}

/**
 * A camera position in graph coordinates: the point at the view's centre and
 * how much of the graph the view spans across. Unlike Sigma's own camera
 * state it does not depend on the map's extent, so it still means the same
 * place after an edit moves or removes a memory at the edge.
 */
export interface CameraView {
  x: number;
  y: number;
  span: number;
}

export interface RendererCallbacks {
  onOpenMemory(id: string): void;
  /** A category disc was clicked on an aggregate map: open that category's own map. */
  onOpenCategory(category: string): void;
  onLevelChange(level: Level): void;
  /**
   * The memories a lasso enclosed: those drawn inside it, plus every member
   * of a category or cluster disc drawn inside it.
   */
  onLasso(ids: string[]): void;
}

export type RendererFactory = (
  container: HTMLElement,
  model: MapModel,
  view: ViewState,
  callbacks: RendererCallbacks,
  /** Start here instead of fitting the whole map: the same map, redrawn after a change. */
  camera?: CameraView | null,
) => MapRenderer;

export type RendererLoader = () => Promise<RendererFactory>;

export const loadSigmaRenderer: RendererLoader = () => import("./sigmaRenderer").then((m) => m.createSigmaRenderer);
