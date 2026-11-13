import { describe, expect, it } from "vitest";
import { bulkEditOf, EMPTY_BULK_FORM, MAX_BULK } from "./bulk";

describe("bulkEditOf", () => {
  it("sends only what the user chose to change", () => {
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, kind: "reference", addTags: "#Rust, cli" }, 2)).toEqual({
      edit: { kind: "reference", addTags: ["rust", "cli"] },
    });
  });

  it("files under a scope, or confirms the structure as it is", () => {
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, topic: "tech", scope: "project:quill" }, 1)).toEqual({
      edit: { topic: "tech", appliesTo: { target: "project", project_name: "quill" } },
    });
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, scope: "general" }, 1)).toEqual({
      edit: { appliesTo: { target: "general", project_name: null } },
    });
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, reviewed: true }, 1)).toEqual({ edit: { reviewed: true } });
  });

  it("clears or sets the sub-label explicitly", () => {
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, subcategory: "clear" }, 1)).toEqual({ edit: { subcategory: null } });
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, subcategory: "set", subcategoryValue: " tooling " }, 1)).toEqual({
      edit: { subcategory: "tooling" },
    });
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, subcategory: "set" }, 1)).toEqual({
      error: "Type the sub-label to set, or choose to clear it.",
    });
  });

  it("explains an empty selection, an oversized one, and a no-op", () => {
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, kind: "fact" }, 0)).toHaveProperty("error");
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, kind: "fact" }, MAX_BULK + 1)).toHaveProperty("error");
    expect(bulkEditOf(EMPTY_BULK_FORM, 3)).toEqual({ error: "Choose something to change." });
  });
});
