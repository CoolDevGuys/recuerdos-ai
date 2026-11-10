import { useId, useState } from "react";
import type { ApiClient } from "../api/client";
import type { Category } from "../api/types";
import type { Access } from "../domain/access";
import {
  destinationsFor,
  memoriesCount,
  normaliseCategoryId,
  validateCategoryId,
  validateCategoryLabel,
} from "../domain/categories";
import { useCategories, type MemoryActions } from "../state/memories";
import { useToasts } from "../state/toasts";
import { FieldError } from "./FieldError";
import { Panel } from "./Panel";
import { dangerButton, field, hint, label, primaryButton, secondaryButton } from "./styles";

type Action = { kind: "edit" | "merge" | "delete"; category: Category };

/**
 * The categories manager (plan §4.2, D4): every category with its count;
 * create a custom one; relabel any; rename, merge or delete a custom one.
 * A relabel is display-only; anything that moves memories says how many.
 */
export function CategoriesView({
  client,
  user,
  access,
  actions,
}: {
  client: ApiClient;
  user: string;
  access: Access;
  actions: MemoryActions;
}) {
  const headingId = useId();
  const categories = useCategories(client, user);
  const [action, setAction] = useState<Action | null>(null);
  const { show } = useToasts();

  if (categories.isPending) return <p role="status" className="text-sm text-slate-600 dark:text-slate-400">Loading categories…</p>;
  if (categories.isError) {
    return (
      <p role="alert" className="text-sm text-red-700 dark:text-red-400">
        The categories could not be loaded: {categories.error.message}
      </p>
    );
  }

  const list = categories.data;
  const close = () => setAction(null);
  const done = (message: string) => {
    close();
    show({ tone: "success", message });
  };

  const adopt = async (category: Category) => {
    const outcome = await actions.createCategory(category.id, null);
    if (outcome.kind === "failed") show({ tone: "error", message: outcome.message });
    else show({ tone: "success", message: `${category.id} is one of your categories again.` });
  };

  const cell = "px-3 py-2 align-top";
  const rowButton =
    "rounded px-2 py-1 text-sm font-medium text-indigo-700 hover:bg-indigo-50 focus-visible:outline-2 focus-visible:outline-indigo-600 dark:text-indigo-300 dark:hover:bg-indigo-950";

  return (
    <section aria-labelledby={headingId} className="space-y-6">
      <h2 id={headingId} className="sr-only">
        Categories
      </h2>
      {access.canWrite && <CreateCategory categories={list} actions={actions} onCreated={(c) => show({ tone: "success", message: `Created ${c.label}.` })} />}

      <div className="overflow-x-auto rounded-lg border border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-900">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-slate-200 text-xs text-slate-600 dark:border-slate-800 dark:text-slate-400">
            <tr>
              <th scope="col" className={cell}>
                Category
              </th>
              <th scope="col" className={cell}>
                Id
              </th>
              <th scope="col" className={`${cell} text-right`}>
                Memories
              </th>
              {access.canWrite && (
                <th scope="col" className={cell}>
                  <span className="sr-only">Actions</span>
                </th>
              )}
            </tr>
          </thead>
          <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
            {list.map((category) => (
              <tr key={category.id}>
                <td className={cell}>
                  <span className="font-medium text-slate-900 dark:text-slate-100">{category.label}</span>
                  {category.label !== category.default_label && (
                    <span className="block text-xs text-slate-500">default: {category.default_label}</span>
                  )}
                  {category.built_in && (
                    <span className="ml-2 rounded bg-slate-100 px-1.5 py-0.5 text-xs text-slate-600 dark:bg-slate-800 dark:text-slate-300">
                      built-in
                    </span>
                  )}
                  {!category.known && (
                    <span className="ml-2 rounded bg-amber-100 px-1.5 py-0.5 text-xs text-amber-900 dark:bg-amber-950 dark:text-amber-200">
                      not in your categories
                    </span>
                  )}
                </td>
                <td className={cell}>
                  <code className="font-mono text-xs text-slate-700 dark:text-slate-300">{category.id}</code>
                </td>
                <td className={`${cell} text-right tabular-nums text-slate-800 dark:text-slate-200`}>{category.count}</td>
                {access.canWrite && (
                  <td className={`${cell} whitespace-nowrap text-right`}>
                    {category.known ? (
                      <>
                        <button
                          type="button"
                          className={rowButton}
                          aria-label={`${category.built_in ? "Relabel" : "Edit"} ${category.label}`}
                          onClick={() => setAction({ kind: "edit", category })}
                        >
                          {category.built_in ? "Relabel" : "Edit"}
                        </button>
                        {!category.built_in && (
                          <>
                            <button
                              type="button"
                              className={rowButton}
                              aria-label={`Merge ${category.label}`}
                              onClick={() => setAction({ kind: "merge", category })}
                            >
                              Merge…
                            </button>
                            <button
                              type="button"
                              className={rowButton}
                              aria-label={`Delete ${category.label}`}
                              onClick={() => setAction({ kind: "delete", category })}
                            >
                              Delete…
                            </button>
                          </>
                        )}
                      </>
                    ) : (
                      <button type="button" className={rowButton} onClick={() => void adopt(category)}>
                        Add to my categories
                      </button>
                    )}
                  </td>
                )}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className={hint}>
        A label only changes what you see. Renaming or merging a custom category moves its memories, and their search
        index is refreshed to match.
      </p>

      {action?.kind === "edit" && (
        <Panel title={action.category.built_in ? "Relabel category" : "Edit category"} onClose={close}>
          <EditCategory category={action.category} categories={list} actions={actions} onDone={done} />
        </Panel>
      )}
      {action?.kind === "merge" && (
        <Panel title="Merge category" onClose={close}>
          <MoveCategory mode="merge" category={action.category} categories={list} actions={actions} onDone={done} onCancel={close} />
        </Panel>
      )}
      {action?.kind === "delete" && (
        <Panel title="Delete category" onClose={close}>
          <MoveCategory mode="delete" category={action.category} categories={list} actions={actions} onDone={done} onCancel={close} />
        </Panel>
      )}
    </section>
  );
}

function CreateCategory({
  categories,
  actions,
  onCreated,
}: {
  categories: readonly Category[];
  actions: MemoryActions;
  onCreated: (category: Category) => void;
}) {
  const id = useId();
  const [rawId, setRawId] = useState("");
  const [rawLabel, setRawLabel] = useState("");
  const [errors, setErrors] = useState<{ id?: string | undefined; label?: string | undefined; form?: string }>({});
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    const next = { id: validateCategoryId(rawId, categories), label: validateCategoryLabel(rawLabel) };
    setErrors(next);
    if (next.id !== undefined || next.label !== undefined) return;
    setBusy(true);
    const outcome = await actions.createCategory(normaliseCategoryId(rawId), rawLabel.trim() === "" ? null : rawLabel.trim());
    setBusy(false);
    if (outcome.kind === "failed") {
      setErrors({ form: outcome.message });
      return;
    }
    setRawId("");
    setRawLabel("");
    onCreated(outcome.value);
  };

  return (
    <form
      aria-labelledby={`${id}-title`}
      className="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
      noValidate
    >
      <h3 id={`${id}-title`} className="text-sm font-semibold text-slate-900 dark:text-slate-100">
        New category
      </h3>
      <div className="mt-2 grid gap-3 sm:grid-cols-[1fr_1fr_auto] sm:items-start">
        <div>
          <label htmlFor={`${id}-id`} className={label}>
            Id
          </label>
          <input
            id={`${id}-id`}
            value={rawId}
            placeholder="health.sleep"
            onChange={(event) => setRawId(event.target.value)}
            aria-invalid={errors.id !== undefined}
            aria-describedby={`${id}-id-error`}
            className={field}
          />
          <FieldError id={`${id}-id-error`} message={errors.id} />
        </div>
        <div>
          <label htmlFor={`${id}-label`} className={label}>
            Label <span className="font-normal text-slate-500">(optional)</span>
          </label>
          <input
            id={`${id}-label`}
            value={rawLabel}
            placeholder="Sleep log"
            onChange={(event) => setRawLabel(event.target.value)}
            aria-invalid={errors.label !== undefined}
            aria-describedby={`${id}-label-error`}
            className={field}
          />
          <FieldError id={`${id}-label-error`} message={errors.label} />
        </div>
        <button type="submit" disabled={busy} className={`${primaryButton} sm:mt-6`}>
          {busy ? "Creating…" : "Create"}
        </button>
      </div>
      {errors.form !== undefined && (
        <p role="alert" className="mt-2 text-sm text-red-700 dark:text-red-400">
          {errors.form}
        </p>
      )}
    </form>
  );
}

function EditCategory({
  category,
  categories,
  actions,
  onDone,
}: {
  category: Category;
  categories: readonly Category[];
  actions: MemoryActions;
  onDone: (message: string) => void;
}) {
  const id = useId();
  const relabelled = category.label !== category.default_label;
  const [rawLabel, setRawLabel] = useState(relabelled ? category.label : "");
  const [rawId, setRawId] = useState(category.id);
  const [errors, setErrors] = useState<{ id?: string | undefined; label?: string | undefined; form?: string }>({});
  const [busy, setBusy] = useState(false);

  const newId = normaliseCategoryId(rawId);
  const renaming = !category.built_in && newId !== category.id;

  const submit = async () => {
    const next = {
      label: validateCategoryLabel(rawLabel),
      id: category.built_in ? undefined : validateCategoryId(rawId, categories, category.id),
    };
    setErrors(next);
    if (next.id !== undefined || next.label !== undefined) return;
    const label = rawLabel.trim() === "" ? null : rawLabel.trim();
    const labelChanged = label !== (relabelled ? category.label : null);
    if (!labelChanged && !renaming) {
      onDone("Nothing to change.");
      return;
    }
    setBusy(true);
    const outcome = await actions.updateCategory(category.id, {
      ...(labelChanged && { label }),
      ...(renaming && { id: newId }),
    });
    setBusy(false);
    if (outcome.kind === "failed") {
      setErrors({ form: outcome.message });
      return;
    }
    const moved = outcome.value.moved;
    onDone(renaming ? `Renamed to ${newId}; ${memoriesCount(moved)} moved.` : "Label saved.");
  };

  return (
    <form
      className="space-y-4"
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <div>
        <label htmlFor={`${id}-label`} className={label}>
          Label
        </label>
        <input
          id={`${id}-label`}
          value={rawLabel}
          placeholder={category.default_label}
          onChange={(event) => setRawLabel(event.target.value)}
          aria-invalid={errors.label !== undefined}
          aria-describedby={`${id}-label-hint ${id}-label-error`}
          className={field}
        />
        <p id={`${id}-label-hint`} className={hint}>
          Leave blank for the default, “{category.default_label}”. Only changes what you see.
        </p>
        <FieldError id={`${id}-label-error`} message={errors.label} />
      </div>
      {!category.built_in && (
        <div>
          <label htmlFor={`${id}-id`} className={label}>
            Id
          </label>
          <input
            id={`${id}-id`}
            value={rawId}
            onChange={(event) => setRawId(event.target.value)}
            aria-invalid={errors.id !== undefined}
            aria-describedby={`${id}-id-hint ${id}-id-error`}
            className={field}
          />
          <p id={`${id}-id-hint`} className={hint}>
            {renaming
              ? `Saving moves its ${memoriesCount(category.count)} to ${newId}. Agents filtering by ${category.id} will need the new id.`
              : "What agents filter by. Changing it moves every memory filed here."}
          </p>
          <FieldError id={`${id}-id-error`} message={errors.id} />
        </div>
      )}
      {errors.form !== undefined && (
        <p role="alert" className="text-sm text-red-700 dark:text-red-400">
          {errors.form}
        </p>
      )}
      <button type="submit" disabled={busy} className={primaryButton}>
        {busy ? "Saving…" : "Save"}
      </button>
    </form>
  );
}

function MoveCategory({
  mode,
  category,
  categories,
  actions,
  onDone,
  onCancel,
}: {
  mode: "merge" | "delete";
  category: Category;
  categories: readonly Category[];
  actions: MemoryActions;
  onDone: (message: string) => void;
  onCancel: () => void;
}) {
  const id = useId();
  const destinations = destinationsFor(categories, category.id);
  const needsDestination = mode === "merge" || category.count > 0;
  const [into, setInto] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const target = destinations.find((c) => c.id === into);

  const submit = async () => {
    if (needsDestination && target === undefined) {
      setError(`Choose where its ${memoriesCount(category.count)} should go.`);
      return;
    }
    setBusy(true);
    const outcome =
      mode === "merge"
        ? await actions.mergeCategory(category.id, into)
        : await actions.deleteCategory(category.id, needsDestination ? into : null);
    setBusy(false);
    if (outcome.kind === "failed") {
      setError(outcome.message);
      return;
    }
    onDone(
      outcome.value.moved > 0
        ? `${category.label} deleted; ${memoriesCount(outcome.value.moved)} moved to ${target?.label ?? into}.`
        : `${category.label} deleted.`,
    );
  };

  return (
    <form
      className="space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <p className="text-sm text-slate-800 dark:text-slate-200">
        {mode === "merge"
          ? `Every memory in ${category.label} (${memoriesCount(category.count)}) moves to the category you choose, then ${category.label} is deleted.`
          : category.count > 0
            ? `${category.label} still holds ${memoriesCount(category.count)}. Choose where they go; then it is deleted.`
            : `${category.label} holds no memories. Delete it?`}
      </p>
      {needsDestination && (
        <div>
          <label htmlFor={`${id}-into`} className={label}>
            Move its memories to
          </label>
          <select id={`${id}-into`} value={into} onChange={(event) => setInto(event.target.value)} className={field}>
            <option value="">Choose a category…</option>
            {destinations.map((c) => (
              <option key={c.id} value={c.id}>
                {c.label}
              </option>
            ))}
          </select>
        </div>
      )}
      {error !== null && (
        <p role="alert" className="text-sm text-red-700 dark:text-red-400">
          {error}
        </p>
      )}
      <div className="flex gap-2">
        <button type="submit" disabled={busy} className={mode === "merge" ? primaryButton : dangerButton}>
          {busy
            ? "Working…"
            : mode === "merge"
              ? `Merge into ${target?.label ?? "…"}`
              : needsDestination
                ? `Move and delete ${category.label}`
                : `Delete ${category.label}`}
        </button>
        <button type="button" onClick={onCancel} disabled={busy} className={secondaryButton}>
          Cancel
        </button>
      </div>
    </form>
  );
}
