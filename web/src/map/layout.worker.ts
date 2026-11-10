/// <reference lib="webworker" />
import type { MapData } from "../api/types";
import { layoutMap, type LayoutSeed } from "../domain/map/layout";

/** Lays a map out off the main thread, so a large store never freezes the page. */
self.onmessage = (event: MessageEvent<{ data: MapData; seed: LayoutSeed | null }>) => {
  try {
    self.postMessage({ model: layoutMap(event.data.data, { seed: event.data.seed }) });
  } catch (error) {
    self.postMessage({ error: error instanceof Error ? error.message : String(error) });
  }
};
