import type { BulkEdit } from "../api/types";
import { MAX_SUBCATEGORY_LENGTH, parseTags } from "./memoryForm";

/** The most memories one bulk edit may touch (the daemon's limit). */
export const MAX_BULK = 500;

/** The bulk editor's fields as typed. `""` means "leave as it is". */
export interface BulkForm {
  kind: string;
  topic: string;
  /** `me`, `general`, `project:<name>` (a known project), or `""`. */
  scope: string;
  /** Confirm each memory's structure as it is: it leaves the review queue. */
  reviewed: boolean;
  /** `keep`, `set` (to `subcategoryValue`) or `clear`. */
  subcategory: "keep" | "set" | "clear";
  subcategoryValue: string;
  addTags: string;
  removeTags: string;
}

export const EMPTY_BULK_FORM: BulkForm = {
  kind: "",
  topic: "",
  scope: "",
  reviewed: false,
  subcategory: "keep",
  subcategoryValue: "",
  addTags: "",
  removeTags: "",
};

/** What to send, or why not: nothing selected, too many, nothing to change, a bad sub-label. */
export function bulkEditOf(form: BulkForm, selected: number): { edit: BulkEdit } | { error: string } {
  if (selected === 0) return { error: "Select some memories first." };
  if (selected > MAX_BULK) return { error: `Edit at most ${MAX_BULK} memories at a time (${selected} are selected).` };
  const edit: BulkEdit = {};
  if (form.kind !== "") edit.kind = form.kind;
  if (form.topic !== "") edit.topic = form.topic;
  if (form.scope.startsWith("project:")) {
    edit.appliesTo = { target: "project", project_name: form.scope.slice("project:".length) };
  } else if (form.scope === "me" || form.scope === "general") {
    edit.appliesTo = { target: form.scope, project_name: null };
  }
  if (form.reviewed) edit.reviewed = true;
  if (form.subcategory === "clear") edit.subcategory = null;
  if (form.subcategory === "set") {
    const value = form.subcategoryValue.trim();
    if (value === "") return { error: "Type the sub-label to set, or choose to clear it." };
    if ([...value].length > MAX_SUBCATEGORY_LENGTH) return { error: `Keep the sub-label under ${MAX_SUBCATEGORY_LENGTH} characters.` };
    edit.subcategory = value;
  }
  const add = parseTags(form.addTags);
  const remove = parseTags(form.removeTags);
  if (add.length > 0) edit.addTags = add;
  if (remove.length > 0) edit.removeTags = remove;
  if (Object.keys(edit).length === 0) return { error: "Choose something to change." };
  return { edit };
}
