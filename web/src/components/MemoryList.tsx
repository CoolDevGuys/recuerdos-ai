import { useId, useMemo, useRef, useState } from "react";
import type { Memory } from "../api/types";
import {
  NO_FILTER,
  countBy,
  filterMemories,
  isClosedGoal,
  isExpired,
  scopeOf,
  sortMemories,
  type MemoryFilter,
  type SortKey,
  type Status,
} from "../domain/memories";
import { describeStructure, kindKeyLabel, kindKeyOf, topicLabelOf } from "../domain/structureForm";
import { secondaryButton } from "./styles";

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });

/**
 * Rows rendered at once. Filtering, sorting and "select all" still work on
 * every memory; only the rows in the page are put in the DOM, which is what
 * made a store of thousands slow to open and to type a filter into.
 */
export const PAGE_SIZE = 50;

/** A filterable list of memories (plan §5.4, "List view"); a row opens it. */
const STATUSES: { id: Status; label: string }[] = [
  { id: "current", label: "Current" },
  { id: "review", label: "Needs review" },
  { id: "closed", label: "Closed goals" },
  { id: "expired", label: "Expired" },
  { id: "all", label: "Everything" },
];

export function MemoryList({
  memories,
  topicLabel = topicLabelOf,
  initialFilter,
  highlighted = null,
  canWrite = false,
  onOpen,
  onAdd,
  selected,
  onSelect,
  pageSize = PAGE_SIZE,
}: {
  memories: readonly Memory[];
  /** A topic's words: the user's label (`GET /v1/topics`). */
  topicLabel?: (id: string) => string;
  /** Where the list opens: a project's memories, the review queue… */
  initialFilter?: Partial<MemoryFilter>;
  /** A memory just saved or restored, marked so the eye finds it. */
  highlighted?: string | null;
  canWrite?: boolean;
  onOpen?: (memory: Memory) => void;
  onAdd?: () => void;
  /** The bulk selection; with `onSelect`, the list offers a "Select" mode. */
  selected?: ReadonlySet<string>;
  /** Adds (`on`) or removes these ids from the selection. */
  onSelect?: (ids: readonly string[], on: boolean) => void;
  pageSize?: number;
}) {
  const searchId = useId();
  const filterId = useId();
  const sortId = useId();
  const [filter, setFilter] = useState<MemoryFilter>(() => ({ ...NO_FILTER, ...initialFilter }));
  const [sort, setSort] = useState<SortKey>("updated");
  const [selecting, setSelecting] = useState(false);
  const [page, setPage] = useState(0);
  const listTop = useRef<HTMLParagraphElement>(null);
  const canSelect = canWrite && onSelect !== undefined && selected !== undefined;

  const topics = useMemo(() => countBy(memories, (m) => m.topic), [memories]);
  const kinds = useMemo(() => countBy(memories, kindKeyOf), [memories]);
  const scopes = useMemo(() => countBy(memories, scopeOf), [memories]);
  const scopeLabel = (scope: string) =>
    scope === "me" ? "You (everywhere)" : scope === "general" ? "General" : `Project ${scope.slice("project:".length)}`;
  const visible = useMemo(
    () => sortMemories(filterMemories(memories, filter), sort),
    [memories, filter, sort],
  );

  const pageCount = Math.max(1, Math.ceil(visible.length / pageSize));
  // Clamped rather than reset, so forgetting the last memory on the last
  // page lands on the page before it instead of an empty one.
  const current = Math.min(page, pageCount - 1);
  const first = current * pageSize;
  const rows = visible.slice(first, first + pageSize);
  const filtered = visible.length !== memories.length;

  // A memory just saved or restored is shown where it now sorts: jump to its
  // page, once per highlight (adjusting state during render, not in an effect).
  const [seenHighlight, setSeenHighlight] = useState(highlighted);
  if (highlighted !== seenHighlight) {
    setSeenHighlight(highlighted);
    const index = highlighted === null ? -1 : visible.findIndex((m) => m.id === highlighted);
    if (index >= 0) setPage(Math.floor(index / pageSize));
  }

  const goTo = (next: number) => {
    setPage(next);
    listTop.current?.scrollIntoView?.({ block: "nearest" });
  };
  const changeFilter = (next: MemoryFilter) => {
    setFilter(next);
    setPage(0);
  };

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
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-[1fr_10rem_10rem_10rem_10rem_9rem]">
        <div className="sm:col-span-2 lg:col-span-1">
          <label htmlFor={searchId} className={labelClass}>
            Filter
          </label>
          <input
            id={searchId}
            type="search"
            placeholder="Words in the memory, its sub-label or tags"
            value={filter.text}
            onChange={(event) => changeFilter({ ...filter, text: event.target.value })}
            className={fieldClass}
          />
        </div>
        <div>
          <label htmlFor={`${filterId}-topic`} className={labelClass}>
            Topic
          </label>
          <select
            id={`${filterId}-topic`}
            value={filter.topic ?? ""}
            onChange={(event) => changeFilter({ ...filter, topic: event.target.value === "" ? null : event.target.value })}
            className={fieldClass}
          >
            <option value="">All topics</option>
            {topics.map((topic) => (
              <option key={topic.id} value={topic.id}>
                {topicLabel(topic.id)} ({topic.count})
              </option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor={`${filterId}-kind`} className={labelClass}>
            Kind
          </label>
          <select
            id={`${filterId}-kind`}
            value={filter.kind ?? ""}
            onChange={(event) => changeFilter({ ...filter, kind: event.target.value === "" ? null : event.target.value })}
            className={fieldClass}
          >
            <option value="">All kinds</option>
            {kinds.map((kind) => (
              <option key={kind.id} value={kind.id}>
                {kindKeyLabel(kind.id)} ({kind.count})
              </option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor={`${filterId}-scope`} className={labelClass}>
            Applies to
          </label>
          <select
            id={`${filterId}-scope`}
            value={filter.scope ?? ""}
            onChange={(event) => changeFilter({ ...filter, scope: event.target.value === "" ? null : event.target.value })}
            className={fieldClass}
          >
            <option value="">Anyone or anything</option>
            {scopes.map((scope) => (
              <option key={scope.id} value={scope.id}>
                {scopeLabel(scope.id)} ({scope.count})
              </option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor={`${filterId}-status`} className={labelClass}>
            Show
          </label>
          <select
            id={`${filterId}-status`}
            value={filter.status}
            onChange={(event) => changeFilter({ ...filter, status: event.target.value as Status })}
            className={fieldClass}
          >
            {STATUSES.map((status) => (
              <option key={status.id} value={status.id}>
                {status.label}
              </option>
            ))}
          </select>
        </div>
        <div>
          <label htmlFor={sortId} className={labelClass}>
            Sort by
          </label>
          <select id={sortId} value={sort} onChange={(event) => {
              setSort(event.target.value as SortKey);
              setPage(0);
            }} className={fieldClass}>
            <option value="updated">Recently updated</option>
            <option value="created">Recently created</option>
            <option value="topic">Topic</option>
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
          {/* Every memory the filter keeps, on every page: bulk edits are for many. */}
          Select all {visible.length}
          {filtered ? " matching" : ""}
        </label>
      )}

      <p ref={listTop} role="status" className="mt-4 scroll-mt-4 text-sm text-slate-600 dark:text-slate-400">
        {filtered
          ? `${visible.length} of ${memories.length} memories`
          : `${memories.length} ${memories.length === 1 ? "memory" : "memories"}`}
        {pageCount > 1 && ` · showing ${first + 1}–${first + rows.length}`}
      </p>

      {visible.length === 0 ? (
        <p className="mt-6 rounded-lg border border-dashed border-slate-300 p-8 text-center text-sm text-slate-600 dark:border-slate-700 dark:text-slate-400">
          {memories.length === 0 ? "No memories stored yet." : "No memory matches this filter."}
        </p>
      ) : (
        <ul className="mt-2 divide-y divide-slate-200 rounded-lg border border-slate-200 bg-white dark:divide-slate-800 dark:border-slate-800 dark:bg-slate-900">
          {rows.map((memory) => (
            <li
              key={memory.id}
              className={
                memory.id === highlighted
                  ? "border-l-4 border-indigo-500 bg-indigo-50/60 dark:bg-indigo-950/40"
                  : "border-l-4 border-transparent"
              }
            >
              {selecting && canSelect ? (
                // The memory's text inside is the label; the rule cannot see text from an expression.
                // oxlint-disable-next-line jsx-a11y/label-has-associated-control
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
                      {describeStructure(memory, topicLabel)}
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
                  {describeStructure(memory, topicLabel)}
                  {memory.subcategory !== null && ` · ${memory.subcategory}`}
                </span>
                <StatusBadges memory={memory} />
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

      {pageCount > 1 && (
        <nav aria-label="Pages" className="mt-3 flex items-center justify-between gap-3">
          <button type="button" onClick={() => goTo(current - 1)} disabled={current === 0} className={secondaryButton}>
            ← Previous
          </button>
          <span className="text-sm text-slate-600 dark:text-slate-400">
            Page {current + 1} of {pageCount}
          </span>
          <button
            type="button"
            onClick={() => goTo(current + 1)}
            disabled={current === pageCount - 1}
            className={secondaryButton}
          >
            Next →
          </button>
        </nav>
      )}
    </section>
  );
}

/** What makes a memory more than current: a guess to review, a closed goal, an end that passed. */
function StatusBadges({ memory }: { memory: Memory }) {
  const badge = "rounded px-1.5 py-0.5 text-[11px] font-medium";
  return (
    <>
      {memory.needs_review && (
        <span className={`${badge} bg-amber-100 text-amber-900 dark:bg-amber-900/50 dark:text-amber-100`}>Needs review</span>
      )}
      {memory.kind === "goal" && memory.goal_status === "active" && (
        <span className={`${badge} bg-indigo-100 text-indigo-900 dark:bg-indigo-900/50 dark:text-indigo-100`}>Open goal</span>
      )}
      {isClosedGoal(memory) && (
        <span className={`${badge} bg-slate-200 text-slate-800 dark:bg-slate-700 dark:text-slate-100`}>
          Goal {memory.goal_status}
        </span>
      )}
      {isExpired(memory) && (
        <span className={`${badge} bg-slate-200 text-slate-800 dark:bg-slate-700 dark:text-slate-100`}>Expired</span>
      )}
    </>
  );
}
