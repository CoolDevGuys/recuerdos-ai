import { describe, expect, it } from "vitest";
import { contrast, distance, VISIONS } from "../../test/colourVision";
import { BUILT_IN_STYLES, CUSTOM_COLORS, categoryStyle, glyphImage, withAlpha } from "./palette";

describe("category styles", () => {
  it("gives every built-in its own colour and glyph", () => {
    const ids = ["preference.coding", "preference.personal", "decision", "fact.project", "fact.person", "experience", "skill", "reference"];
    const styles = ids.map(categoryStyle);
    expect(new Set(styles.map((s) => s.color)).size).toBe(ids.length);
    expect(new Set(styles.map((s) => s.glyph)).size).toBe(ids.length);
  });

  it("gives a custom category a stable colour and its initial as glyph", () => {
    expect(categoryStyle("health.sleep")).toEqual(categoryStyle("health.sleep"));
    expect(categoryStyle("health.sleep").glyph).toBe("letter");
    expect(decodeURIComponent(glyphImage("letter", "Health sleep"))).toContain(">H</text>");
  });

  it("escapes a label's initial inside the SVG", () => {
    expect(decodeURIComponent(glyphImage("letter", "<script>"))).not.toContain("<script>");
  });

  it("builds glyphs as SVG data URLs", () => {
    expect(glyphImage("code", "x")).toMatch(/^data:image\/svg\+xml;charset=utf-8,/);
  });

  it("converts a hex colour to rgba", () => {
    expect(withAlpha("#ff8000", 0.5)).toBe("rgba(255,128,0,0.5)");
    expect(withAlpha("#000000", 2)).toBe("rgba(0,0,0,1)");
  });
});

describe("the palette for every viewer", () => {
  const builtIns = Object.entries(BUILT_IN_STYLES);

  it("keeps 3:1 against the light and the dark canvas", () => {
    for (const color of [...builtIns.map(([, s]) => s.color), ...CUSTOM_COLORS]) {
      expect(contrast(color, "#ffffff"), `${color} on white`).toBeGreaterThanOrEqual(3);
      expect(contrast(color, "#020617"), `${color} on slate-950`).toBeGreaterThanOrEqual(3);
    }
  });

  it.each(VISIONS)("keeps the built-ins apart with %s", (vision) => {
    for (let i = 0; i < builtIns.length; i += 1) {
      for (let j = i + 1; j < builtIns.length; j += 1) {
        const [[a, sa], [b, sb]] = [builtIns[i]!, builtIns[j]!];
        expect(distance(sa.color, sb.color, vision), `${a} / ${b}`).toBeGreaterThanOrEqual(14);
      }
    }
  });

  it("keeps custom colours apart from each other and from the built-ins", () => {
    const all = [...builtIns.map(([, s]) => s.color), ...CUSTOM_COLORS];
    const pairs = CUSTOM_COLORS.flatMap((custom) => all.filter((other) => other !== custom).map((other) => [custom, other] as const));
    for (const vision of VISIONS) {
      for (const [custom, other] of pairs) {
        expect(distance(custom, other, vision), `${custom} / ${other}, ${vision}`).toBeGreaterThanOrEqual(8);
      }
    }
  });
});

