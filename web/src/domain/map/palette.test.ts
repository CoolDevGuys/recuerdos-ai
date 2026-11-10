import { describe, expect, it } from "vitest";
import { categoryStyle, glyphImage, withAlpha } from "./palette";

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
