import { describe, expect, it } from "vitest";
import { bulkEditOf, EMPTY_BULK_FORM, MAX_BULK } from "./bulk";

describe("bulkEditOf", () => {
  it("sends only what the user chose to change", () => {
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, category: "skill", addTags: "#Rust, cli" }, 2)).toEqual({
      edit: { category: "skill", addTags: ["rust", "cli"] },
    });
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
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, category: "skill" }, 0)).toHaveProperty("error");
    expect(bulkEditOf({ ...EMPTY_BULK_FORM, category: "skill" }, MAX_BULK + 1)).toHaveProperty("error");
    expect(bulkEditOf(EMPTY_BULK_FORM, 3)).toEqual({ error: "Choose something to change." });
  });
});
