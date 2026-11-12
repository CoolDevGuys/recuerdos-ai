import type { Memory } from "../api/types";

/**
 * Which memories the list shows. `current` is what recall would still use:
 * not a closed goal, not past its end. The others are the review queue
 * (filed by a guess), closed goals and expired memories, kept in history.
 */
export type Status = "current" | "review" | "closed" | "expired" | "all";

export interface MemoryFilter {
  /** Free text, matched case-insensitively against content, sub-label and tags. */
  text: string;
  /** A topic id, or `null` for all. */
  topic: string | null;
  /** A kind, or `null` for all. */
  kind: string | null;
  /** Who it is for: `me`, `general`, `project:<name>`, or `null` for all. */
  scope: string | null;
  status: Status;
}

export const NO_FILTER: MemoryFilter = { text: "", topic: null, kind: null, scope: null, status: "current" };

/** `me`, `general` or `project:<name>`: the scope a memory is filed for. */
export function scopeOf(memory: Memory): string {
  return memory.applies_to.target === "project" ? `project:${memory.applies_to.project_name ?? ""}` : memory.applies_to.target;
}

export function isClosedGoal(memory: Memory): boolean {
  return memory.goal_status === "achieved" || memory.goal_status === "abandoned";
}

export function isExpired(memory: Memory, now: number = Date.now()): boolean {
  return memory.valid_until !== null && Date.parse(memory.valid_until) < now;
}

function hasStatus(memory: Memory, status: Status, now: number): boolean {
  switch (status) {
    case "all":
      return true;
    case "review":
      return memory.needs_review;
    case "closed":
      return isClosedGoal(memory);
    case "expired":
      return isExpired(memory, now);
    case "current":
      return !isClosedGoal(memory) && !isExpired(memory, now);
  }
}

export function filterMemories(memories: readonly Memory[], filter: MemoryFilter, now: number = Date.now()): Memory[] {
  const needle = filter.text.trim().toLowerCase();
  return memories.filter((memory) => {
    if (!hasStatus(memory, filter.status, now)) return false;
    if (filter.topic !== null && memory.topic !== filter.topic) return false;
    if (filter.kind !== null && memory.kind !== filter.kind) return false;
    if (filter.scope !== null && scopeOf(memory) !== filter.scope) return false;
    if (needle === "") return true;
    return (
      memory.content.toLowerCase().includes(needle) ||
      (memory.subcategory?.toLowerCase().includes(needle) ?? false) ||
      memory.tags.some((tag) => tag.toLowerCase().includes(needle))
    );
  });
}

export type SortKey = "updated" | "created" | "topic";

export function sortMemories(memories: readonly Memory[], key: SortKey): Memory[] {
  const sorted = [...memories];
  switch (key) {
    case "updated":
      return sorted.sort((a, b) => b.updated_at.localeCompare(a.updated_at) || a.id.localeCompare(b.id));
    case "created":
      return sorted.sort((a, b) => b.created_at.localeCompare(a.created_at) || a.id.localeCompare(b.id));
    case "topic":
      return sorted.sort(
        (a, b) =>
          a.topic.localeCompare(b.topic) ||
          a.kind.localeCompare(b.kind) ||
          (a.subcategory ?? "").localeCompare(b.subcategory ?? "") ||
          b.updated_at.localeCompare(a.updated_at),
      );
  }
}

/** How many memories have each value of `key`, largest first, ties alphabetical. */
export function countBy(memories: readonly Memory[], key: (memory: Memory) => string): { id: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const memory of memories) counts.set(key(memory), (counts.get(key(memory)) ?? 0) + 1);
  return [...counts]
    .map(([id, count]) => ({ id, count }))
    .sort((a, b) => b.count - a.count || a.id.localeCompare(b.id));
}

const BUILT_IN_LABELS: Record<string, string> = {
  "preference.coding": "Coding preference",
  "preference.personal": "Personal preference",
  decision: "Decision",
  "fact.project": "Project fact",
  "fact.person": "Fact about a person",
  experience: "Experience",
  skill: "Skill",
  reference: "Reference",
};

/**
 * A category as words. Mirrors the daemon's enriched-header wording
 * (src/memories/domain/document_text.rs) until labels come from
 * `GET /v1/categories` (plan §4.2).
 */
export function categoryLabel(id: string): string {
  const builtIn = BUILT_IN_LABELS[id];
  if (builtIn !== undefined) return builtIn;
  const words = id.replace(/[._-]/g, " ").trim();
  return words === "" ? "Memory" : words.charAt(0).toUpperCase() + words.slice(1);
}
