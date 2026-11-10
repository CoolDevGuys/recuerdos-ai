import { describe, expect, it } from "vitest";
import { LEVELS, LEVEL_RATIO, levelAt, quantize, zoomMix } from "./zoom";

describe("semantic zoom", () => {
  it("shows categories zoomed out and memories zoomed in", () => {
    expect(zoomMix(1)).toEqual({ cluster: 0, memory: 0 });
    expect(zoomMix(0.05)).toEqual({ cluster: 1, memory: 1 });
  });

  it("dissolves categories into clusters before clusters into memories", () => {
    const middle = zoomMix(0.47);
    expect(middle.cluster).toBe(1);
    expect(middle.memory).toBe(0);
  });

  it("moves monotonically as the camera closes in", () => {
    let previous = zoomMix(1.2);
    for (let ratio = 1.2; ratio > 0.05; ratio -= 0.01) {
      const mix = zoomMix(ratio);
      expect(mix.cluster).toBeGreaterThanOrEqual(previous.cluster);
      expect(mix.memory).toBeGreaterThanOrEqual(previous.memory);
      previous = mix;
    }
  });

  it("names the level each preset ratio shows", () => {
    for (const level of LEVELS) expect(levelAt(LEVEL_RATIO[level])).toBe(level);
  });

  it("quantizes the mix into a bounded number of steps", () => {
    expect(quantize({ cluster: 0.512, memory: 0.049 })).toEqual({ cluster: 0.5, memory: 0.05 });
  });
});
