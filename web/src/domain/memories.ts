import type { Memory } from "../api/types";

export interface MemoryFilter {
  /** Free text, matched case-insensitively against content, sub-label and tags. */
  text: string;
  /** A category id, or `null` for all. */
  category: string | null;
}

export const NO_FILTER: MemoryFilter = { text: "", category: null };

export function filterMemories(memories: readonly Memory[], filter: MemoryFilter): Memory[] {
  const needle = filter.text.trim().toLowerCase();
  return memories.filter((memory) => {
    if (filter.category !== null && memory.category !== filter.category) return false;
    if (needle === "") return true;
    return (
      memory.content.toLowerCase().includes(needle) ||
      (memory.subcategory?.toLowerCase().includes(needle) ?? false) ||
      memory.tags.some((tag) => tag.toLowerCase().includes(needle))
    );
  });
}

export type SortKey = "updated" | "created" | "category";

export function sortMemories(memories: readonly Memory[], key: SortKey): Memory[] {
  const sorted = [...memories];
  switch (key) {
    case "updated":
      return sorted.sort((a, b) => b.updated_at.localeCompare(a.updated_at) || a.id.localeCompare(b.id));
    case "created":
      return sorted.sort((a, b) => b.created_at.localeCompare(a.created_at) || a.id.localeCompare(b.id));
    case "category":
      return sorted.sort(
        (a, b) =>
          a.category.localeCompare(b.category) ||
          (a.subcategory ?? "").localeCompare(b.subcategory ?? "") ||
          b.updated_at.localeCompare(a.updated_at),
      );
  }
}

export interface CategoryCount {
  id: string;
  count: number;
}

/** Every category present, largest first, ties alphabetical. */
export function categoryCounts(memories: readonly Memory[]): CategoryCount[] {
  const counts = new Map<string, number>();
  for (const memory of memories) counts.set(memory.category, (counts.get(memory.category) ?? 0) + 1);
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
