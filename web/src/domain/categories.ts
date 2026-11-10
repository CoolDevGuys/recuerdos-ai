import type { Category } from "../api/types";

// Mirror src/memories/domain/category.rs, so a mistake is explained before
// the round trip; the daemon stays the authority.
export const MAX_CATEGORY_ID_LENGTH = 64;
export const MAX_CATEGORY_LABEL_LENGTH = 64;

const ID_SHAPE = /^[a-z][a-z0-9._-]*$/;

/** A category id as the daemon will store it: trimmed and lowercased. */
export function normaliseCategoryId(raw: string): string {
  return raw.trim().toLowerCase();
}

/** Why `raw` cannot be a new category id, or `undefined` when it can. */
export function validateCategoryId(raw: string, existing: readonly Category[], current: string | null = null): string | undefined {
  const id = normaliseCategoryId(raw);
  if (id === "") return "Give the category an id, like health.sleep.";
  if ([...id].length > MAX_CATEGORY_ID_LENGTH) return `Keep the id under ${MAX_CATEGORY_ID_LENGTH} characters.`;
  if (!ID_SHAPE.test(id)) return "Start with a letter and use only a–z, 0–9, dots, dashes and underscores.";
  if (id === current) return undefined;
  const clash = existing.find((c) => c.id === id && c.known);
  if (clash !== undefined) {
    return clash.built_in
      ? `${id} is a built-in category. Relabel it instead.`
      : `You already have a category ${id}.`;
  }
  return undefined;
}

export function validateCategoryLabel(raw: string): string | undefined {
  const label = raw.trim();
  if ([...label].length > MAX_CATEGORY_LABEL_LENGTH) return `Keep the label under ${MAX_CATEGORY_LABEL_LENGTH} characters.`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(label)) return "A label is one line of text.";
  return undefined;
}

/** Where a category's memories may go: every other category the user has. */
export function destinationsFor(categories: readonly Category[], id: string): Category[] {
  return categories.filter((c) => c.known && c.id !== id);
}

/** The categories a memory may be filed under: those in the user's catalog. */
export function fileable(categories: readonly Category[]): Category[] {
  return categories.filter((c) => c.known);
}

export function memoriesCount(n: number): string {
  return `${n} ${n === 1 ? "memory" : "memories"}`;
}
