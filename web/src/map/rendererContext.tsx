import { createContext, useContext } from "react";
import { loadSigmaRenderer, type RendererLoader } from "./renderer";

/** Which renderer the map uses: Sigma in the app, a recording fake in tests. */
export const RendererContext = createContext<RendererLoader>(loadSigmaRenderer);

export function useRendererLoader(): RendererLoader {
  return useContext(RendererContext);
}
