import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { ApiClient } from "../../api/client";
import type { Category, Memory, RecallHit } from "../../api/types";
import { memoriesCount } from "../../domain/categories";
import type { ViewState } from "../../domain/map/appearance";
import { layoutMap } from "../../domain/map/layout";
import { categoryStyle, glyphImage } from "../../domain/map/palette";
import { LEVELS, type Level } from "../../domain/map/zoom";
import { useRendererLoader } from "../../map/rendererContext";
import type { MapRenderer } from "../../map/renderer";
import { useMap, type MemoryActions } from "../../state/memories";
import { BulkEditor } from "../BulkEditor";
import { RecallPanel } from "./RecallPanel";

const LEVEL_NAMES: Record<Level, string> = { categories: "Categories", clusters: "Clusters", memories: "Memories" };

/**
 * The mental map (plan §5): categories when zoomed out, dissolving into
 * sub-label clusters and then into individual memories as you zoom in.
 * Presentation and wiring only — layout and looks are in domain/map, the
 * drawing in the lazily loaded renderer.
 */
export default function MapView({
  client,
  user,
  actions,
  labelOf,
  onOpenMemory,
  categories = [],
  canWrite = false,
}: {
  client: ApiClient;
  user: string;
  actions: MemoryActions;
  labelOf: (id: string) => string;
  onOpenMemory: (memory: Memory) => void;
  /** For the bulk editor a lasso opens. */
  categories?: readonly Category[];
  canWrite?: boolean;
}) {
  const sliderId = useId();
  const map = useMap(client, user);
  const loadRenderer = useRendererLoader();
  const container = useRef<HTMLDivElement>(null);
  const renderer = useRef<MapRenderer | null>(null);
  const [rendererFailed, setRendererFailed] = useState(false);
  const [level, setLevel] = useState<Level>("categories");
  const [lassoMode, setLassoMode] = useState(false);
  const [selected, setSelected] = useState<readonly string[]>([]);
  const [lassoNote, setLassoNote] = useState<string | null>(null);
  const [recallHits, setRecallHits] = useState<ReadonlySet<string> | null>(null);
  const [view, setView] = useState<ViewState>({
    hiddenCategories: new Set(),
    showSimilar: true,
    showRelations: true,
    minWeight: 0,
    highlighted: null,
    hovered: null,
  });

  const model = useMemo(() => (map.data === undefined ? null : layoutMap(map.data)), [map.data]);
  const relationCount = model?.edges.filter((e) => e.kind === "relation").length ?? 0;
  const similarCount = model?.edges.filter((e) => e.kind === "similar").length ?? 0;
  const floor = Math.max(view.minWeight, model?.minWeight ?? 0);

  // Latest values for the renderer's callbacks, which are bound once.
  const latest = useRef({ model, onOpenMemory, view });
  latest.current = { model, onOpenMemory, view };

  useEffect(() => {
    if (model === null || container.current === null || model.memories.length === 0) return;
    let cancelled = false;
    let created: MapRenderer | null = null;
    loadRenderer()
      .then((create) => {
        if (cancelled || container.current === null) return;
        created = create(container.current, model, latest.current.view, {
          onOpenMemory: (id) => {
            const node = latest.current.model?.byId.get(id);
            if (node !== undefined) latest.current.onOpenMemory(node.memory);
          },
          onLevelChange: setLevel,
          onLasso: (ids) => {
            setSelected(ids);
            setLassoNote(ids.length === 0 ? "The lasso caught no memories. Draw around the dots or discs you want." : null);
          },
        });
        renderer.current = created;
        setRendererFailed(false);
      })
      .catch(() => setRendererFailed(true));
    return () => {
      cancelled = true;
      created?.destroy();
      renderer.current = null;
    };
  }, [model, loadRenderer]);

  // A search's answers take precedence; otherwise the lassoed selection is what stands out.
  const highlighted = recallHits ?? (selected.length > 0 ? new Set(selected) : null);
  const shown: ViewState = useMemo(
    () => ({ ...view, highlighted, labelHighlighted: recallHits !== null, minWeight: floor }),
    [view, highlighted, recallHits, floor],
  );
  latest.current.view = shown;

  useEffect(() => {
    renderer.current?.update(shown);
  }, [shown]);

  useEffect(() => {
    renderer.current?.setLassoMode(lassoMode);
  }, [lassoMode, model]);

  const clearSelection = () => {
    setSelected([]);
    setLassoNote(null);
  };

  const toggleCategory = (id: string) =>
    setView((current) => {
      const hidden = new Set(current.hiddenCategories);
      if (hidden.has(id)) hidden.delete(id);
      else hidden.add(id);
      return { ...current, hiddenCategories: hidden };
    });

  const showRecall = (hits: RecallHit[] | null) => {
    setRecallHits(hits === null ? null : new Set(hits.map((h) => h.memory.id)));
    if (hits !== null && hits.length > 0) renderer.current?.focus(hits.map((h) => h.memory.id));
    if (hits === null) renderer.current?.reset();
  };

  if (map.isPending) return <p role="status" className="text-sm text-slate-600 dark:text-slate-400">Drawing your map…</p>;
  if (map.isError) {
    return (
      <div role="alert" className="rounded-lg border border-red-300 bg-red-50 p-4 text-sm text-red-900 dark:border-red-800 dark:bg-red-950 dark:text-red-200">
        <p>The map could not be loaded: {map.error.message}</p>
        <button type="button" onClick={() => void map.refetch()} className="mt-2 font-medium underline">
          Try again
        </button>
      </div>
    );
  }
  if (model === null || model.memories.length === 0) {
    return (
      <p className="rounded-lg border border-dashed border-slate-300 p-8 text-center text-sm text-slate-600 dark:border-slate-700 dark:text-slate-400">
        No memories stored yet, so there is nothing to map.
      </p>
    );
  }

  const control =
    "rounded-md border border-slate-300 bg-white px-2.5 py-1 text-sm text-slate-800 hover:bg-slate-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100 dark:hover:bg-slate-800";

  return (
    <div className="grid gap-4 lg:grid-cols-[1fr_20rem]">
      <section aria-label="Mental map" className="min-w-0">
        <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
          <div role="group" aria-label="Zoom level" className="flex overflow-hidden rounded-md border border-slate-300 dark:border-slate-600">
            {LEVELS.map((l) => (
              <button
                key={l}
                type="button"
                aria-pressed={level === l}
                onClick={() => renderer.current?.zoomTo(l)}
                className={
                  "px-3 py-1 text-sm " +
                  (level === l
                    ? "bg-indigo-600 font-medium text-white"
                    : "bg-white text-slate-700 hover:bg-slate-50 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800")
                }
              >
                {LEVEL_NAMES[l]}
              </button>
            ))}
          </div>
          <div className="flex gap-1">
            <button type="button" aria-label="Zoom in" onClick={() => renderer.current?.zoomIn()} className={control}>
              +
            </button>
            <button type="button" aria-label="Zoom out" onClick={() => renderer.current?.zoomOut()} className={control}>
              −
            </button>
            <button type="button" onClick={() => renderer.current?.reset()} className={control}>
              Fit
            </button>
            {canWrite && (
              <button
                type="button"
                aria-pressed={lassoMode}
                onClick={() => setLassoMode((on) => !on)}
                className={control + (lassoMode ? " !border-indigo-600 !bg-indigo-600 !text-white" : "")}
              >
                Lasso select
              </button>
            )}
          </div>
        </div>

        <div
          ref={container}
          role="img"
          aria-label={`Map of ${model.memories.length} memories in ${model.categories.length} categories, showing ${LEVEL_NAMES[level].toLowerCase()}. The list view has the same memories as text.`}
          className="relative h-[62vh] min-h-80 w-full overflow-hidden rounded-lg border border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-950"
        />
        {rendererFailed && (
          <p role="alert" className="mt-2 text-sm text-red-700 dark:text-red-400">
            <span aria-hidden="true">⚠ </span>The map could not start in this browser (it needs WebGL). The list view shows the same memories.
          </p>
        )}
        <p className="mt-2 text-xs text-slate-600 dark:text-slate-400">
          {model.memories.length} memories · {similarCount} similarity links
          {relationCount > 0 && ` · ${relationCount} stated relations`}
          {map.data.edgesOmitted && " · too many memories to compare, links omitted"}. Scroll or pinch to zoom; click a
          category to open it, a memory to edit it.
          {canWrite && " To select memories for a bulk edit, turn on Lasso select (or hold Shift) and draw around them."}
        </p>
        {lassoNote !== null && (
          <p role="status" className="mt-1 text-sm text-amber-800 dark:text-amber-300">
            {lassoNote}
          </p>
        )}
      </section>

      <aside className="space-y-6">
        {canWrite && selected.length > 0 && (
          <div>
            <p className="sr-only" role="status">
              {memoriesCount(selected.length)} selected on the map.
            </p>
            <BulkEditor
              selected={selected}
              categories={categories}
              actions={actions}
              onDone={clearSelection}
              onClear={clearSelection}
            />
          </div>
        )}
        <RecallPanel
          recall={(query) => actions.recall(query)}
          labelOf={labelOf}
          onResults={showRecall}
          onPick={(hit) => {
            renderer.current?.focus([hit.memory.id]);
            onOpenMemory(hit.memory);
          }}
        />

        <section aria-labelledby={`${sliderId}-legend`}>
          <h2 id={`${sliderId}-legend`} className="text-sm font-semibold text-slate-900 dark:text-slate-100">
            Categories
          </h2>
          <ul className="mt-2 space-y-1">
            {model.categories.map((category) => {
              const style = categoryStyle(category.category);
              const shown = !view.hiddenCategories.has(category.category);
              return (
                <li key={category.id}>
                  <button
                    type="button"
                    aria-pressed={shown}
                    onClick={() => toggleCategory(category.category)}
                    className={
                      "flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm hover:bg-slate-100 dark:hover:bg-slate-800 " +
                      (shown ? "text-slate-900 dark:text-slate-100" : "text-slate-400 line-through dark:text-slate-500")
                    }
                  >
                    <span
                      aria-hidden="true"
                      className="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-full"
                      style={{ backgroundColor: shown ? style.color : "#94a3b8" }}
                    >
                      <img src={glyphImage(style.glyph, category.label)} alt="" className="h-3.5 w-3.5" />
                    </span>
                    <span className="flex-1">{category.label}</span>
                    <span className="text-xs text-slate-500">{category.count}</span>
                    <span className="sr-only">{shown ? "(shown)" : "(hidden)"}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        </section>

        <section aria-labelledby={`${sliderId}-links`}>
          <h2 id={`${sliderId}-links`} className="text-sm font-semibold text-slate-900 dark:text-slate-100">
            Links
          </h2>
          <div className="mt-2 space-y-2 text-sm text-slate-800 dark:text-slate-200">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={view.showSimilar}
                onChange={(event) => setView((current) => ({ ...current, showSimilar: event.target.checked }))}
                className="h-4 w-4 accent-indigo-600"
              />
              <span>
                Similar meaning <span className="text-slate-500">(solid lines)</span>
              </span>
            </label>
            {relationCount > 0 && (
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={view.showRelations}
                  onChange={(event) => setView((current) => ({ ...current, showRelations: event.target.checked }))}
                  className="h-4 w-4 accent-indigo-600"
                />
                <span>
                  Stated relations <span className="text-slate-500">(arrows)</span>
                </span>
              </label>
            )}
            <div>
              <label htmlFor={sliderId} className="block">
                Minimum similarity: {Math.round(floor * 100)}%
              </label>
              <input
                id={sliderId}
                type="range"
                min={Math.round(model.minWeight * 100)}
                max={95}
                step={5}
                value={Math.round(floor * 100)}
                onChange={(event) => setView((current) => ({ ...current, minWeight: Number(event.target.value) / 100 }))}
                className="mt-1 w-full accent-indigo-600"
              />
            </div>
          </div>
        </section>
      </aside>
    </div>
  );
}
