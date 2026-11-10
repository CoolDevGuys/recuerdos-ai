import type { MapData } from "../api/types";
import { layoutMap, type LayoutSeed, type MapModel } from "../domain/map/layout";

/** Computes a map's layout, wherever it runs. */
export type LayoutRunner = (data: MapData, seed: LayoutSeed | null) => Promise<MapModel>;

/** On the calling thread: tests, and browsers without module workers. */
export const inlineLayout: LayoutRunner = async (data, seed) => layoutMap(data, { seed });

/**
 * In a Web Worker. Each layout gets a fresh worker, ended when it answers;
 * starting one costs a few milliseconds against layouts that can take
 * seconds. If the worker cannot start or fails, the layout runs inline
 * rather than leave the map blank.
 */
export const workerLayout: LayoutRunner = (data, seed) =>
  new Promise<MapModel>((resolve, reject) => {
    let worker: Worker;
    try {
      worker = new Worker(new URL("./layout.worker.ts", import.meta.url), { type: "module" });
    } catch (error) {
      reject(error instanceof Error ? error : new Error(String(error)));
      return;
    }
    worker.onmessage = (event: MessageEvent<{ model?: MapModel; error?: string }>) => {
      worker.terminate();
      if (event.data.model !== undefined) resolve(event.data.model);
      else reject(new Error(event.data.error ?? "layout failed"));
    };
    worker.onerror = (event) => {
      worker.terminate();
      reject(new Error(event.message || "layout worker failed"));
    };
    worker.postMessage({ data, seed });
  }).catch(() => inlineLayout(data, seed));

export const defaultLayoutRunner: LayoutRunner = typeof Worker === "undefined" ? inlineLayout : workerLayout;
