import { useId, useState } from "react";
import { bulkEditOf, EMPTY_BULK_FORM, type BulkForm } from "../domain/bulk";
import { memoriesCount } from "../domain/categories";
import { KINDS, TOPICS } from "../domain/structureForm";
import type { MemoryActions } from "../state/memories";
import { useToasts } from "../state/toasts";
import { field, hint, label, primaryButton, secondaryButton } from "./styles";

/**
 * One edit applied to every selected memory (plan §4.3, and the review queue
 * of memory-structure-plan §8): what they are, what they are about, who they
 * are for, the sub-label and tags — or "these are right as they are". All or
 * nothing on the daemon, under the same rules as a save; shared by the
 * list's selection, the map's lasso and the review queue.
 */
export function BulkEditor({
  selected,
  topics = TOPICS,
  projects = [],
  actions,
  onDone,
  onClear,
}: {
  selected: readonly string[];
  topics?: readonly { id: string; label: string }[];
  /** Known project names: a bulk edit files only under an existing project. */
  projects?: readonly string[];
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
          <label htmlFor={`${id}-kind`} className={label}>
            What they are
          </label>
          <select id={`${id}-kind`} value={form.kind} onChange={(event) => set({ kind: event.target.value })} className={field}>
            <option value="">Leave as it is</option>
            {KINDS.map((k) => (
              <option key={k.id} value={k.id}>
                {k.label}
              </option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor={`${id}-topic`} className={label}>
            About
          </label>
          <select id={`${id}-topic`} value={form.topic} onChange={(event) => set({ topic: event.target.value })} className={field}>
            <option value="">Leave as it is</option>
            {topics.map((t) => (
              <option key={t.id} value={t.id}>
                {t.label}
              </option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor={`${id}-scope`} className={label}>
            Applies to
          </label>
          <select id={`${id}-scope`} value={form.scope} onChange={(event) => set({ scope: event.target.value })} className={field}>
            <option value="">Leave as it is</option>
            <option value="me">Me, everywhere</option>
            <option value="general">General</option>
            {projects.map((name) => (
              <option key={name} value={`project:${name}`}>
                Project {name}
              </option>
            ))}
          </select>
          <p className={hint}>A memory a kind does not fit (a preference of a project) stops the whole edit.</p>
        </div>
        <label className="flex items-start gap-2 self-end text-sm text-slate-800 dark:text-slate-200">
          <input
            type="checkbox"
            checked={form.reviewed}
            onChange={(event) => set({ reviewed: event.target.checked })}
            className="mt-0.5 h-4 w-4 accent-indigo-600"
          />
          <span>
            Mark as reviewed
            <span className={`block ${hint}`}>They leave the Needs review queue.</span>
          </span>
        </label>

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
