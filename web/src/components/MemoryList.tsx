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

/** A read-only, filterable table of memories (plan §5.4, "List view"). */
export function MemoryList({ memories }: { memories: readonly Memory[] }) {
  const searchId = useId();
  const categoryId = useId();
  const sortId = useId();
  const [filter, setFilter] = useState<MemoryFilter>(NO_FILTER);
  const [sort, setSort] = useState<SortKey>("updated");

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
                {categoryLabel(category.id)} ({category.count})
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
            <li key={memory.id} className="px-4 py-3">
              <p className="text-sm text-slate-900 dark:text-slate-100">{memory.content}</p>
              <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-slate-600 dark:text-slate-400">
                <span className="font-medium text-slate-700 dark:text-slate-300">
                  {categoryLabel(memory.category)}
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
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
