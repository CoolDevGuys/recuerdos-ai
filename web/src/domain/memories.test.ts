import { describe, expect, it } from "vitest";
import { memory } from "../api/fake";
import { categoryLabel, countBy, filterMemories, sortMemories } from "./memories";

const pnpm = memory({ id: "a", content: "The user prefers pnpm", tags: ["package-management"], updated_at: "2026-08-31T00:00:00Z" });
const tdd = memory({ id: "b", content: "Testing follows TDD", subcategory: "testing", updated_at: "2026-08-11T00:00:00Z" });
const stack = memory({
  id: "c",
  content: "Frontend uses Next.js",
  category: "fact.project",
  kind: "convention",
  topic: "work",
  applies_to: { target: "project", project_name: "quill" },
  updated_at: "2026-09-01T00:00:00Z",
  created_at: "2026-07-01T00:00:00Z",
});
const all = [pnpm, tdd, stack];

const none = { text: "", topic: null, kind: null, scope: null, status: "current" } as const;

describe("filterMemories", () => {
  it("returns everything current with no filter", () => {
    expect(filterMemories(all, none)).toEqual(all);
  });

  it("matches content case-insensitively", () => {
    expect(filterMemories(all, { ...none, text: "NEXT" })).toEqual([stack]);
  });

  it("matches the sub-label and tags as well as content", () => {
    expect(filterMemories(all, { ...none, text: "testing" })).toEqual([tdd]);
    expect(filterMemories(all, { ...none, text: "package" })).toEqual([pnpm]);
  });

  it("narrows by topic, kind and who it is for", () => {
    expect(filterMemories(all, { ...none, topic: "work" })).toEqual([stack]);
    expect(filterMemories(all, { ...none, kind: "convention" })).toEqual([stack]);
    expect(filterMemories(all, { ...none, scope: "project:quill" })).toEqual([stack]);
    expect(filterMemories(all, { ...none, scope: "me" })).toEqual([pnpm, tdd]);
  });

  it("combines text and structure", () => {
    expect(filterMemories(all, { ...none, text: "pnpm", topic: "work" })).toEqual([]);
  });

  it("keeps closed goals and expired memories out of the current list, and lists them on request", () => {
    const now = Date.parse("2026-11-12T00:00:00Z");
    const done = memory({ id: "g", content: "Run a half marathon", kind: "goal", goal_status: "achieved" });
    const past = memory({ id: "p", content: "Dentist on 1 November", kind: "plan", valid_until: "2026-11-01T23:59:59Z" });
    const review = memory({ id: "r", content: "A guess", needs_review: true });
    const store = [...all, done, past, review];

    expect(filterMemories(store, none, now).map((m) => m.id)).toEqual(["a", "b", "c", "r"]);
    expect(filterMemories(store, { ...none, status: "closed" }, now)).toEqual([done]);
    expect(filterMemories(store, { ...none, status: "expired" }, now)).toEqual([past]);
    expect(filterMemories(store, { ...none, status: "review" }, now)).toEqual([review]);
    expect(filterMemories(store, { ...none, status: "all" }, now)).toHaveLength(6);
  });
});

describe("sortMemories", () => {
  it("puts the most recently updated first", () => {
    expect(sortMemories(all, "updated").map((m) => m.id)).toEqual(["c", "a", "b"]);
  });

  it("puts the most recently created first", () => {
    expect(sortMemories(all, "created").map((m) => m.id)[2]).toBe("c");
  });

  it("groups by topic", () => {
    expect(sortMemories(all, "topic").map((m) => m.topic)).toEqual(["coding", "coding", "work"]);
  });

  it("does not reorder its input", () => {
    const input = [...all];
    sortMemories(input, "topic");
    expect(input).toEqual(all);
  });
});

describe("countBy", () => {
  it("counts each value, largest first", () => {
    expect(countBy(all, (m) => m.topic)).toEqual([
      { id: "coding", count: 2 },
      { id: "work", count: 1 },
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
