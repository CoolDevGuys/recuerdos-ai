import { describe, expect, it } from "vitest";
import type { Category } from "../api/types";
import { destinationsFor, fileable, memoriesCount, validateCategoryId, validateCategoryLabel } from "./categories";

function category(id: string, overrides: Partial<Category> = {}): Category {
  return { id, label: id, default_label: id, built_in: false, known: true, count: 0, ...overrides };
}

const existing = [category("decision", { built_in: true }), category("homelab"), category("leftover", { known: false })];

describe("validateCategoryId", () => {
  it("accepts a well-formed new id, in any case", () => {
    expect(validateCategoryId("  Health.Sleep ", existing)).toBeUndefined();
  });

  it.each([
    ["", "Give the category an id"],
    ["9lives", "Start with a letter"],
    ["has space", "Start with a letter"],
    ["x".repeat(65), "under 64"],
  ])("refuses %j", (raw, message) => {
    expect(validateCategoryId(raw, existing)).toContain(message);
  });

  it("refuses a clash, saying a built-in is relabelled instead", () => {
    expect(validateCategoryId("Decision", existing)).toContain("Relabel it instead");
    expect(validateCategoryId("homelab", existing)).toContain("already have");
  });

  it("allows keeping the current id, and adopting a leftover", () => {
    expect(validateCategoryId("homelab", existing, "homelab")).toBeUndefined();
    expect(validateCategoryId("leftover", existing)).toBeUndefined();
  });
});

describe("validateCategoryLabel", () => {
  it("allows blank (the default wording) and refuses overlong or multi-line labels", () => {
    expect(validateCategoryLabel("  ")).toBeUndefined();
    expect(validateCategoryLabel("x".repeat(65))).toContain("under 64");
    expect(validateCategoryLabel("a\nb")).toContain("one line");
  });
});

describe("destinations and fileable categories", () => {
  it("never offers the category itself or a leftover", () => {
    expect(destinationsFor(existing, "homelab").map((c) => c.id)).toEqual(["decision"]);
    expect(fileable(existing).map((c) => c.id)).toEqual(["decision", "homelab"]);
  });

  it("counts in words", () => {
    expect(memoriesCount(1)).toBe("1 memory");
    expect(memoriesCount(3)).toBe("3 memories");
  });
});
