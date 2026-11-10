import type { Memory, MemoryFields, MemoryPatch } from "../api/types";

// Limits mirror src/memories/domain/memory.rs, so a mistake is explained
// before the round trip; the daemon stays the authority.
export const MAX_CONTENT_LENGTH = 4000;
export const MAX_TAGS = 32;
export const MAX_TAG_LENGTH = 64;
export const MAX_SUBCATEGORY_LENGTH = 64;

/** The editor's fields as typed: tags are one comma-separated string. */
export interface FormValues {
  content: string;
  category: string;
  subcategory: string;
  tags: string;
}

export type FormErrors = Partial<Record<keyof FormValues, string>>;

export function valuesOf(memory: Memory): FormValues {
  return {
    content: memory.content,
    category: memory.category,
    subcategory: memory.subcategory ?? "",
    tags: memory.tags.join(", "),
  };
}

export function emptyValues(category: string): FormValues {
  return { content: "", category, subcategory: "", tags: "" };
}

/** Comma- or newline-separated, trimmed, lowercased, de-duplicated, `#` dropped. */
export function parseTags(raw: string): string[] {
  const tags: string[] = [];
  for (const part of raw.split(/[,\n]/)) {
    const tag = part.trim().replace(/^#+/, "").toLowerCase();
    if (tag !== "" && !tags.includes(tag)) tags.push(tag);
  }
  return tags;
}

export function validateField(field: keyof FormValues, values: FormValues): string | undefined {
  switch (field) {
    case "content": {
      const length = [...values.content.trim()].length;
      if (length === 0) return "Write what should be remembered.";
      if (length > MAX_CONTENT_LENGTH) {
        return `Keep it under ${MAX_CONTENT_LENGTH} characters (it is ${length}). Split a long note into several memories.`;
      }
      return undefined;
    }
    case "category":
      return values.category.trim() === "" ? "Choose a category." : undefined;
    case "subcategory":
      return [...values.subcategory.trim()].length > MAX_SUBCATEGORY_LENGTH
        ? `Keep the sub-label under ${MAX_SUBCATEGORY_LENGTH} characters.`
        : undefined;
    case "tags": {
      const tags = parseTags(values.tags);
      if (tags.length > MAX_TAGS) return `At most ${MAX_TAGS} tags (there are ${tags.length}).`;
      const long = tags.find((tag) => [...tag].length > MAX_TAG_LENGTH);
      return long === undefined ? undefined : `“${long.slice(0, 20)}…” is longer than ${MAX_TAG_LENGTH} characters.`;
    }
  }
}

export function validate(values: FormValues, fields: readonly (keyof FormValues)[] = FIELDS): FormErrors {
  const errors: FormErrors = {};
  for (const field of fields) {
    const error = validateField(field, values);
    if (error !== undefined) errors[field] = error;
  }
  return errors;
}

export const FIELDS: readonly (keyof FormValues)[] = ["content", "category", "subcategory", "tags"];

export function fieldsOf(values: FormValues): MemoryFields {
  const subcategory = values.subcategory.trim();
  return {
    content: values.content.trim(),
    category: values.category,
    subcategory: subcategory === "" ? null : subcategory,
    tags: parseTags(values.tags),
  };
}

/** Only what changed between `memory` and `values`; `null` when nothing did. */
export function patchOf(memory: Memory, values: FormValues): MemoryPatch | null {
  const next = fieldsOf(values);
  const patch: MemoryPatch = {};
  if (next.content !== memory.content) patch.content = next.content;
  if (next.category !== memory.category) patch.category = next.category;
  if (next.subcategory !== memory.subcategory) patch.subcategory = next.subcategory;
  if (next.tags.join("\u0000") !== memory.tags.join("\u0000")) patch.tags = next.tags;
  return Object.keys(patch).length === 0 ? null : patch;
}
