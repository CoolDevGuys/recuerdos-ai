import { useId, useState } from "react";
import type { Category } from "../api/types";
import { bulkEditOf, EMPTY_BULK_FORM, type BulkForm } from "../domain/bulk";
import { fileable, memoriesCount } from "../domain/categories";
import type { MemoryActions } from "../state/memories";
import { useToasts } from "../state/toasts";
import { field, hint, label, primaryButton, secondaryButton } from "./styles";

/**
 * One edit applied to every selected memory (plan §4.3): re-file, set or
 * clear the sub-label, add or remove tags. All or nothing on the daemon;
 * shared by the list's selection and the map's lasso.
 */
export function BulkEditor({
  selected,
  categories,
  actions,
  onDone,
  onClear,
}: {
  selected: readonly string[];
  categories: readonly Category[];
  actions: MemoryActions;
  /** After a successful edit; the selection is usually cleared. */
  onDone: () => void;
  onClear: () => void;
}) {
  const id = useId();
  const { show } = useToasts();
  const [form, setForm] = useState<BulkForm>(EMPTY_BULK_FORM);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const set = (patch: Partial<BulkForm>) => {
    setForm((current) => ({ ...current, ...patch }));
    setError(null);
  };

  const apply = async () => {
    const built = bulkEditOf(form, selected.length);
    if ("error" in built) {
      setError(built.error);
      return;
    }
    setBusy(true);
    const outcome = await actions.bulkEdit([...selected], built.edit);
    setBusy(false);
    if (outcome.kind === "failed") {
      setError(`Nothing was changed: ${outcome.message}`);
      return;
    }
    const { changed } = outcome.value;
    show({
      tone: "success",
      message: changed === 0 ? "Those memories already looked like that." : `Updated ${memoriesCount(changed)}.`,
    });
    setForm(EMPTY_BULK_FORM);
    onDone();
  };

  return (
    <section
      aria-labelledby={`${id}-title`}
      className="rounded-lg border border-indigo-200 bg-indigo-50/60 p-4 dark:border-indigo-900 dark:bg-indigo-950/30"
    >
      <div className="flex items-center justify-between gap-2">
        <h2 id={`${id}-title`} className="text-sm font-semibold text-slate-900 dark:text-slate-100">
          {memoriesCount(selected.length)} selected
        </h2>
        <button type="button" onClick={onClear} className="text-sm font-medium text-indigo-700 underline dark:text-indigo-300">
          Clear selection
        </button>
      </div>

      <form
        className="mt-3 grid gap-3 sm:grid-cols-2"
        onSubmit={(event) => {
          event.preventDefault();
          void apply();
        }}
      >
        <div>
          <label htmlFor={`${id}-category`} className={label}>
            Move to category
          </label>
          <select
            id={`${id}-category`}
            value={form.category}
            onChange={(event) => set({ category: event.target.value })}
            className={field}
          >
            <option value="">Leave as it is</option>
            {fileable(categories).map((c) => (
              <option key={c.id} value={c.id}>
                {c.label}
              </option>
            ))}
          </select>
        </div>

        <fieldset>
          <legend className={label}>Sub-label</legend>
          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-slate-800 dark:text-slate-200">
            {(["keep", "set", "clear"] as const).map((choice) => (
              <label key={choice} className="flex items-center gap-1">
                <input
                  type="radio"
                  name={`${id}-sub`}
                  checked={form.subcategory === choice}
                  onChange={() => set({ subcategory: choice })}
                  className="accent-indigo-600"
                />
                {choice === "keep" ? "Leave" : choice === "set" ? "Set to" : "Clear"}
              </label>
            ))}
          </div>
          {form.subcategory === "set" && (
            <input
              aria-label="New sub-label"
              value={form.subcategoryValue}
              onChange={(event) => set({ subcategoryValue: event.target.value })}
              className={field}
            />
          )}
        </fieldset>

        <div>
          <label htmlFor={`${id}-add`} className={label}>
            Add tags
          </label>
          <input id={`${id}-add`} value={form.addTags} onChange={(event) => set({ addTags: event.target.value })} className={field} />
          <p className={hint}>Comma-separated.</p>
        </div>
        <div>
          <label htmlFor={`${id}-remove`} className={label}>
            Remove tags
          </label>
          <input
            id={`${id}-remove`}
            value={form.removeTags}
            onChange={(event) => set({ removeTags: event.target.value })}
            className={field}
          />
        </div>

        {error !== null && (
          <p role="alert" className="text-sm text-red-700 sm:col-span-2 dark:text-red-400">
            {error}
          </p>
        )}
        <div className="flex gap-2 sm:col-span-2">
          <button type="submit" disabled={busy} className={primaryButton}>
            {busy ? "Applying…" : `Apply to ${memoriesCount(selected.length)}`}
          </button>
          <button type="button" onClick={onClear} disabled={busy} className={secondaryButton}>
            Cancel
          </button>
        </div>
      </form>
    </section>
  );
}
