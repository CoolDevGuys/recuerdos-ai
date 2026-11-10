import type { ViewState } from "../domain/map/appearance";
import type { MapModel } from "../domain/map/layout";
import type { Level } from "../domain/map/zoom";
import type { MapRenderer, RendererCallbacks, RendererLoader } from "../map/renderer";

/** Records what the map asked of its renderer, and lets a test play the user's clicks on the canvas. */
export class FakeRenderer implements MapRenderer {
  model: MapModel | null = null;
  views: ViewState[] = [];
  focused: string[][] = [];
  zooms: string[] = [];
  destroyed = false;
  lassoModes: boolean[] = [];
  callbacks: RendererCallbacks | null = null;

  update(view: ViewState): void {
    this.views.push(view);
  }
  focus(ids: readonly string[]): void {
    this.focused.push([...ids]);
  }
  zoomTo(level: Level): void {
    this.zooms.push(level);
  }
  zoomIn(): void {
    this.zooms.push("in");
  }
  zoomOut(): void {
    this.zooms.push("out");
  }
  reset(): void {
    this.zooms.push("reset");
  }
  pans: [number, number][] = [];
  pan(dx: number, dy: number): void {
    this.pans.push([dx, dy]);
  }
  setLassoMode(on: boolean): void {
    this.lassoModes.push(on);
  }
  destroy(): void {
    this.destroyed = true;
  }

  /** Simulates the user drawing a lasso around these memories. */
  lasso(ids: string[]): void {
    this.callbacks?.onLasso(ids);
  }

  get lastView(): ViewState | undefined {
    return this.views.at(-1);
  }

  /** Simulates a click on a memory node. */
  clickMemory(id: string): void {
    this.callbacks?.onOpenMemory(id);
  }

  /** Simulates a click on a category disc of an aggregate map. */
  clickCategory(category: string): void {
    this.callbacks?.onOpenCategory(category);
  }

  /** Simulates the camera reaching a level. */
  reachLevel(level: Level): void {
    this.callbacks?.onLevelChange(level);
  }
}

export function fakeRendererLoader(renderer: FakeRenderer): RendererLoader {
  return async () => (_container, model, view, callbacks) => {
    renderer.model = model;
    renderer.views.push(view);
    renderer.callbacks = callbacks;
    return renderer;
  };
}
