import { useId, useMemo, useState } from "react";
import type { Memory } from "../api/types";
import {
  NO_FILTER,
  categoryCounts,
  categoryLabel,
  filterMemories,
  sortMemories,
  type MemoryFilter,
  type SortKey,
} from "../domain/memories";

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });

/** A filterable list of memories (plan §5.4, "List view"); a row opens it. */
export function MemoryList({
  memories,
  labelOf = categoryLabel,
  highlighted = null,
  canWrite = false,
  onOpen,
  onAdd,
  selected,
  onSelect,
}: {
  memories: readonly Memory[];
  labelOf?: (id: string) => string;
  /** A memory just saved or restored, marked so the eye finds it. */
  highlighted?: string | null;
  canWrite?: boolean;
  onOpen?: (memory: Memory) => void;
  onAdd?: () => void;
  /** The bulk selection; with `onSelect`, the list offers a "Select" mode. */
  selected?: ReadonlySet<string>;
  /** Adds (`on`) or removes these ids from the selection. */
  onSelect?: (ids: readonly string[], on: boolean) => void;
}) {
  const searchId = useId();
  const categoryId = useId();
  const sortId = useId();
  const [filter, setFilter] = useState<MemoryFilter>(NO_FILTER);
  const [sort, setSort] = useState<SortKey>("updated");
  const [selecting, setSelecting] = useState(false);
  const canSelect = canWrite && onSelect !== undefined && selected !== undefined;

  const categories = useMemo(() => categoryCounts(memories), [memories]);
  const visible = useMemo(
    () => sortMemories(filterMemories(memories, filter), sort),
    [memories, filter, sort],
  );

  const fieldClass =
    "mt-1 block w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100";
  const labelClass = "block text-xs font-medium text-slate-700 dark:text-slate-300";

  return (
    <section aria-labelledby={`${searchId}-heading`}>
      <h2 id={`${searchId}-heading`} className="sr-only">
        Memories
      </h2>
      {canWrite && (onAdd !== undefined || canSelect) && (
        <div className="mb-4 flex justify-end gap-2">
          {canSelect && (
            <button
              type="button"
              aria-pressed={selecting}
              onClick={() => {
                if (selecting) onSelect(memories.map((m) => m.id), false);
                setSelecting(!selecting);
              }}
              className="rounded-md border border-slate-300 bg-white px-4 py-2 text-sm font-medium text-slate-800 hover:bg-slate-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-600 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100 dark:hover:bg-slate-800"
            >
              {selecting ? "Done selecting" : "Select"}
            </button>
          )}
          {onAdd !== undefined && (
          <button
            type="button"
            onClick={onAdd}
            className="rounded-md bg-indigo-600 px-4 py-2 text-sm font-medium text-white hover:bg-indigo-500 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-600"
          >
            + Add memory
          </button>
          )}
        </div>
      )}
      <div className="grid gap-3 sm:grid-cols-[1fr_14rem_10rem]">
        <div>
          <label htmlFor={searchId} className={labelClass}>
            Filter
          </label>
          <input
            id={searchId}
            type="search"
            placeholder="Words in the memory, its sub-label or tags"
            value={filter.text}
            onChange={(event) => setFilter({ ...filter, text: event.target.value })}
            className={fieldClass}
          />
        </div>
        <div>
          <label htmlFor={categoryId} className={labelClass}>
            Category
          </label>
          <select
            id={categoryId}
            value={filter.category ?? ""}
            onChange={(event) => setFilter({ ...filter, category: event.target.value === "" ? null : event.target.value })}
            className={fieldClass}
          >
            <option value="">All categories ({memories.length})</option>
            {categories.map((category) => (
              <option key={category.id} value={category.id}>
                {labelOf(category.id)} ({category.count})
              </option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor={sortId} className={labelClass}>
            Sort by
          </label>
          <select id={sortId} value={sort} onChange={(event) => setSort(event.target.value as SortKey)} className={fieldClass}>
            <option value="updated">Recently updated</option>
            <option value="created">Recently created</option>
            <option value="category">Category</option>
          </select>
        </div>
      </div>

      {selecting && canSelect && visible.length > 0 && (
        <label className="mt-4 flex items-center gap-2 text-sm text-slate-800 dark:text-slate-200">
          <input
            type="checkbox"
            checked={visible.every((m) => selected.has(m.id))}
            onChange={(event) => onSelect(visible.map((m) => m.id), event.target.checked)}
            className="h-4 w-4 accent-indigo-600"
          />
          Select all {visible.length} shown
        </label>
      )}

      <p role="status" className="mt-4 text-sm text-slate-600 dark:text-slate-400">
        {visible.length === memories.length
          ? `${memories.length} ${memories.length === 1 ? "memory" : "memories"}`
          : `${visible.length} of ${memories.length} memories`}
      </p>

      {visible.length === 0 ? (
        <p className="mt-6 rounded-lg border border-dashed border-slate-300 p-8 text-center text-sm text-slate-600 dark:border-slate-700 dark:text-slate-400">
          {memories.length === 0 ? "No memories stored yet." : "No memory matches this filter."}
        </p>
      ) : (
        <ul className="mt-2 divide-y divide-slate-200 rounded-lg border border-slate-200 bg-white dark:divide-slate-800 dark:border-slate-800 dark:bg-slate-900">
          {visible.map((memory) => (
            <li
              key={memory.id}
              className={
                memory.id === highlighted
                  ? "border-l-4 border-indigo-500 bg-indigo-50/60 dark:bg-indigo-950/40"
                  : "border-l-4 border-transparent"
              }
            >
              {selecting && canSelect ? (
                <label className="flex cursor-pointer gap-3 px-4 py-3 hover:bg-slate-50 dark:hover:bg-slate-800/60">
                  <input
                    type="checkbox"
                    checked={selected.has(memory.id)}
                    onChange={(event) => onSelect([memory.id], event.target.checked)}
                    className="mt-0.5 h-4 w-4 shrink-0 accent-indigo-600"
                  />
                  <span className="min-w-0">
                    <span className="block text-sm text-slate-900 dark:text-slate-100">{memory.content}</span>
                    <span className="mt-1 block text-xs text-slate-600 dark:text-slate-400">
                      {labelOf(memory.category)}
                      {memory.subcategory !== null && ` · ${memory.subcategory}`}
                      {memory.tags.length > 0 && ` · ${memory.tags.map((tag) => `#${tag}`).join(" ")}`}
                    </span>
                  </span>
                </label>
              ) : (
              <button
                type="button"
                onClick={() => onOpen?.(memory)}
                disabled={onOpen === undefined}
                className="block w-full px-4 py-3 text-left hover:bg-slate-50 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-indigo-600 disabled:cursor-default disabled:hover:bg-transparent dark:hover:bg-slate-800/60"
              >
              <p className="text-sm text-slate-900 dark:text-slate-100">
                {memory.id === highlighted && <span className="sr-only">Just changed: </span>}
                {memory.content}
              </p>
              <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-slate-600 dark:text-slate-400">
                <span className="font-medium text-slate-700 dark:text-slate-300">
                  {labelOf(memory.category)}
                  {memory.subcategory !== null && ` · ${memory.subcategory}`}
                </span>
                {memory.tags.length > 0 && (
                  <span>
                    <span className="sr-only">Tags: </span>
                    {memory.tags.map((tag) => `#${tag}`).join(" ")}
                  </span>
                )}
                <span>
                  Updated <time dateTime={memory.updated_at}>{dateFormat.format(new Date(memory.updated_at))}</time>
                </span>
              </p>
              </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
