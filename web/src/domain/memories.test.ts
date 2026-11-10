import { describe, expect, it } from "vitest";
import { memory } from "../api/fake";
import { categoryCounts, categoryLabel, filterMemories, sortMemories } from "./memories";

const pnpm = memory({ id: "a", content: "The user prefers pnpm", tags: ["package-management"], updated_at: "2026-08-31T00:00:00Z" });
const tdd = memory({ id: "b", content: "Testing follows TDD", subcategory: "testing", updated_at: "2026-08-11T00:00:00Z" });
const stack = memory({ id: "c", content: "Frontend uses Next.js", category: "fact.project", updated_at: "2026-09-01T00:00:00Z", created_at: "2026-07-01T00:00:00Z" });
const all = [pnpm, tdd, stack];

describe("filterMemories", () => {
  it("returns everything with no filter", () => {
    expect(filterMemories(all, { text: "", category: null })).toEqual(all);
  });

  it("matches content case-insensitively", () => {
    expect(filterMemories(all, { text: "NEXT", category: null })).toEqual([stack]);
  });

  it("matches the sub-label and tags as well as content", () => {
    expect(filterMemories(all, { text: "testing", category: null })).toEqual([tdd]);
    expect(filterMemories(all, { text: "package", category: null })).toEqual([pnpm]);
  });

  it("narrows to one category", () => {
    expect(filterMemories(all, { text: "", category: "fact.project" })).toEqual([stack]);
  });

  it("combines text and category", () => {
    expect(filterMemories(all, { text: "pnpm", category: "fact.project" })).toEqual([]);
  });
});

describe("sortMemories", () => {
  it("puts the most recently updated first", () => {
    expect(sortMemories(all, "updated").map((m) => m.id)).toEqual(["c", "a", "b"]);
  });

  it("puts the most recently created first", () => {
    expect(sortMemories(all, "created").map((m) => m.id)[2]).toBe("c");
  });

  it("groups by category", () => {
    expect(sortMemories(all, "category").map((m) => m.category)).toEqual([
      "fact.project",
      "preference.coding",
      "preference.coding",
    ]);
  });

  it("does not reorder its input", () => {
    const input = [...all];
    sortMemories(input, "category");
    expect(input).toEqual(all);
  });
});

describe("categoryCounts", () => {
  it("counts each category, largest first", () => {
    expect(categoryCounts(all)).toEqual([
      { id: "preference.coding", count: 2 },
      { id: "fact.project", count: 1 },
    ]);
  });
});

describe("categoryLabel", () => {
  it("names built-in categories in words", () => {
    expect(categoryLabel("preference.coding")).toBe("Coding preference");
  });

  it("turns a custom id into words", () => {
    expect(categoryLabel("health.sleep_log")).toBe("Health sleep log");
  });
});
