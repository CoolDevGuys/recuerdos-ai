import { describe, expect, it } from "vitest";
import { contrast, distance, VISIONS } from "../../test/colourVision";
import { KINDS } from "../structureForm";
import { KIND_STYLES, TOPIC_COLOR, categoryStyle, glyphImage, kindStyle, withAlpha } from "./palette";

describe("map styles", () => {
  it("gives every kind its own colour and glyph", () => {
    const kinds = KINDS.map((k) => k.id);
    const styles = kinds.map(kindStyle);
    expect(new Set(kinds).size).toBe(11);
    expect(new Set(styles.map((s) => s.color)).size).toBe(kinds.length);
    expect(new Set(styles.map((s) => s.glyph)).size).toBe(kinds.length);
    expect(Object.keys(KIND_STYLES).sort()).toEqual([...kinds].sort());
  });

  it("draws a topic group neutral, with its initial", () => {
    expect(categoryStyle("coding")).toEqual({ color: TOPIC_COLOR, glyph: "letter" });
    expect(decodeURIComponent(glyphImage("letter", "Health"))).toContain(">H</text>");
  });

  it("shows a kind it does not know as neutral rather than failing", () => {
    expect(kindStyle("opinion").color).toBe(TOPIC_COLOR);
  });

  it("escapes a label's initial inside the SVG", () => {
    expect(decodeURIComponent(glyphImage("letter", "<script>"))).not.toContain("<script>");
  });

  it("builds glyphs as SVG data URLs", () => {
    for (const style of Object.values(KIND_STYLES)) {
      expect(glyphImage(style.glyph, "x")).toMatch(/^data:image\/svg\+xml;charset=utf-8,/);
    }
  });

  it("converts a hex colour to rgba", () => {
    expect(withAlpha("#ff8000", 0.5)).toBe("rgba(255,128,0,0.5)");
    expect(withAlpha("#000000", 2)).toBe("rgba(0,0,0,1)");
  });
});

describe("the 11-kind palette for every viewer", () => {
  const kinds = Object.entries(KIND_STYLES);

  it("keeps 3:1 against the light and the dark canvas", () => {
    for (const color of [...kinds.map(([, s]) => s.color), TOPIC_COLOR]) {
      expect(contrast(color, "#ffffff"), `${color} on white`).toBeGreaterThanOrEqual(3);
      expect(contrast(color, "#020617"), `${color} on slate-950`).toBeGreaterThanOrEqual(3);
    }
  });

  it.each(VISIONS)("keeps every pair of kinds apart with %s (ΔE ≥ 14)", (vision) => {
    for (let i = 0; i < kinds.length; i += 1) {
      for (let j = i + 1; j < kinds.length; j += 1) {
        const [[a, sa], [b, sb]] = [kinds[i]!, kinds[j]!];
        expect(distance(sa.color, sb.color, vision), `${a} / ${b}`).toBeGreaterThanOrEqual(14);
      }
    }
  });
});
