import { createContext, useContext } from "react";
import { defaultLayoutRunner, type LayoutRunner } from "./layoutRunner";
import { loadSigmaRenderer, type RendererLoader } from "./renderer";

/** Which renderer the map uses: Sigma in the app, a recording fake in tests. */
export const RendererContext = createContext<RendererLoader>(loadSigmaRenderer);

export function useRendererLoader(): RendererLoader {
  return useContext(RendererContext);
}

/** Where layouts run: a Web Worker in the app, inline in tests. */
export const LayoutContext = createContext<LayoutRunner>(defaultLayoutRunner);

export function useLayoutRunner(): LayoutRunner {
  return useContext(LayoutContext);
}
