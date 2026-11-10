import { describe, expect, it } from "vitest";
import { memory } from "../api/fake";
import { parseTags, patchOf, validate, valuesOf } from "./memoryForm";

const base = memory({ id: "a", content: "Tests use Jest", subcategory: "testing", tags: ["jest"] });

describe("parseTags", () => {
  it("splits on commas and newlines, trims, lowercases and de-duplicates", () => {
    expect(parseTags(" Pnpm, tooling\n#pnpm ,, ")).toEqual(["pnpm", "tooling"]);
  });

  it("is empty for blank input", () => {
    expect(parseTags("  ,  ")).toEqual([]);
  });
});

describe("validate", () => {
  it("accepts a memory as loaded", () => {
    expect(validate(valuesOf(base))).toEqual({});
  });

  it("requires content", () => {
    expect(validate({ ...valuesOf(base), content: "   " }).content).toBe("Write what should be remembered.");
  });

  it("caps content at the server's limit", () => {
    expect(validate({ ...valuesOf(base), content: "x".repeat(4001) }).content).toMatch(/under 4000/);
  });

  it("caps the number of tags", () => {
    const tags = Array.from({ length: 33 }, (_, i) => `t${i}`).join(",");
    expect(validate({ ...valuesOf(base), tags }).tags).toMatch(/At most 32/);
  });

  it("checks only the fields asked for", () => {
    expect(validate({ ...valuesOf(base), category: "" }, ["content"])).toEqual({});
  });
});

describe("patchOf", () => {
  it("is null when nothing changed, whatever the spacing", () => {
    expect(patchOf(base, { ...valuesOf(base), content: "  Tests use Jest  ", tags: "jest," })).toBeNull();
  });

  it("carries only the changed fields", () => {
    expect(patchOf(base, { ...valuesOf(base), content: "Tests use Vitest" })).toEqual({ content: "Tests use Vitest" });
  });

  it("clears the sub-label with null", () => {
    expect(patchOf(base, { ...valuesOf(base), subcategory: " " })).toEqual({ subcategory: null });
  });

  it("sends the full new tag list", () => {
    expect(patchOf(base, { ...valuesOf(base), tags: "jest, vitest" })).toEqual({ tags: ["jest", "vitest"] });
  });
});
