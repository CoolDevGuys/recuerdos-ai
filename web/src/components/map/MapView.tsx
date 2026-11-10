import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import type { ApiClient } from "../../api/client";
import type { Category, Memory, RecallHit } from "../../api/types";
import { memoriesCount } from "../../domain/categories";
import type { ViewState } from "../../domain/map/appearance";
import { seedOf, type MapModel } from "../../domain/map/layout";
import { categoryStyle, glyphImage } from "../../domain/map/palette";
import { LEVELS, type Level } from "../../domain/map/zoom";
import { readLayoutSeed, writeLayoutSeed } from "../../map/layoutCache";
import { useLayoutRunner, useRendererLoader } from "../../map/rendererContext";
import type { MapRenderer } from "../../map/renderer";
import { useMap, type MemoryActions } from "../../state/memories";
import { BulkEditor } from "../BulkEditor";
import { RecallPanel } from "./RecallPanel";

const LEVEL_NAMES: Record<Level, string> = { categories: "Categories", clusters: "Clusters", memories: "Memories" };

/**
 * The mental map (plan §5): categories when zoomed out, dissolving into
 * sub-label clusters and then into individual memories as you zoom in.
 * Presentation and wiring only — layout and looks are in domain/map, the
 * drawing in the lazily loaded renderer, the layout itself in a worker.
 *
 * A store above the daemon's detail limit comes back as categories only;
 * the map then shows the category discs, and opening one shows that
 * category's own map.
 *
 * Keyboard, with the map focused: `+` / `-` zoom, `0` fits, `1` `2` `3`
 * jump to a level, the arrow keys pan, Escape clears a selection.
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
  const [scope, setScope] = useState<string | null>(null);
  const map = useMap(client, user, scope);
  const loadRenderer = useRendererLoader();
  const runLayout = useLayoutRunner();
  const [model, setModel] = useState<MapModel | null>(null);
  const [layoutFailed, setLayoutFailed] = useState(false);
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

  // Lay out each answer off the main thread, starting from the last layout of the same scope.
  useEffect(() => {
    const data = map.data;
    if (data === undefined) return;
    let cancelled = false;
    const seed = data.detail === "memories" ? readLayoutSeed(user, data.category) : null;
    runLayout(data, seed)
      .then((laid) => {
        if (cancelled) return;
        setModel(laid);
        setLayoutFailed(false);
        if (!laid.aggregate && laid.memories.length > 0) writeLayoutSeed(user, laid.scope, seedOf(laid));
      })
      .catch(() => {
        if (!cancelled) setLayoutFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [map.data, user, runLayout]);
  const relationCount = model?.edges.filter((e) => e.kind === "relation").length ?? 0;
  const similarCount = model?.edges.filter((e) => e.kind === "similar").length ?? 0;
  const floor = Math.max(view.minWeight, model?.minWeight ?? 0);

  // Latest values for the renderer's callbacks, which are bound once.
  const latest = useRef({ model, onOpenMemory, view });
  latest.current = { model, onOpenMemory, view };

  useEffect(() => {
    if (model === null || container.current === null) return;
    if (model.memories.length === 0 && model.categories.length === 0) return;
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
          onOpenCategory: (category) => openCategory(category),
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
  const highlighted = useMemo(() => recallHits ?? (selected.length > 0 ? new Set(selected) : null), [recallHits, selected]);
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

  function openCategory(category: string | null) {
    clearSelection();
    setLassoMode(false);
    setRecallHits(null);
    setLevel("categories");
    setScope(category);
  }

  const onMapKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const r = renderer.current;
    if (r === null || event.altKey || event.ctrlKey || event.metaKey) return;
    const step = 0.2;
    const actions: Record<string, () => void> = {
      "+": () => r.zoomIn(),
      "=": () => r.zoomIn(),
      "-": () => r.zoomOut(),
      _: () => r.zoomOut(),
      "0": () => r.reset(),
      ArrowLeft: () => r.pan(-step, 0),
      ArrowRight: () => r.pan(step, 0),
      ArrowUp: () => r.pan(0, -step),
      ArrowDown: () => r.pan(0, step),
      Escape: () => {
        clearSelection();
        setLassoMode(false);
      },
    };
    if (model?.aggregate !== true) {
      LEVELS.forEach((l, i) => {
        actions[String(i + 1)] = () => r.zoomTo(l);
      });
    }
    const action = actions[event.key];
    if (action === undefined) return;
    event.preventDefault();
    action();
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
  if (layoutFailed) {
    return (
      <p role="alert" className="text-sm text-red-700 dark:text-red-400">
        <span aria-hidden="true">⚠ </span>The map could not be laid out. The list view shows the same memories.
      </p>
    );
  }
  if (model === null || model.scope !== scope) {
    return (
      <p role="status" className="text-sm text-slate-600 dark:text-slate-400">
        Laying out {map.data.detail === "categories" ? "your categories" : memoriesCount(map.data.total)}…
      </p>
    );
  }
  if (model.total === 0) {
    if (scope !== null) {
      return (
        <div className="space-y-2">
          <p className="text-sm text-slate-600 dark:text-slate-400">This category has no memories any more.</p>
          <button type="button" onClick={() => openCategory(null)} className="text-sm font-medium text-indigo-700 underline dark:text-indigo-300">
            Back to all categories
          </button>
        </div>
      );
    }
    return (
      <p className="rounded-lg border border-dashed border-slate-300 p-8 text-center text-sm text-slate-600 dark:border-slate-700 dark:text-slate-400">
        No memories stored yet, so there is nothing to map.
      </p>
    );
  }

  const aggregate = model.aggregate;
  const scopeLabel = scope === null ? null : (model.categories[0]?.label ?? labelOf(scope));
  const control =
    "rounded-md border border-slate-300 bg-white px-2.5 py-1 text-sm text-slate-800 hover:bg-slate-50 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100 dark:hover:bg-slate-800";

  return (
    <div className="grid gap-4 lg:grid-cols-[1fr_20rem]">
      <section aria-label="Mental map" className="min-w-0">
        {scope !== null && (
          <div className="mb-2 flex flex-wrap items-baseline gap-x-3 gap-y-1">
            <button type="button" onClick={() => openCategory(null)} className="text-sm font-medium text-indigo-700 hover:underline dark:text-indigo-300">
              <span aria-hidden="true">← </span>All categories
            </button>
            <h2 className="text-base font-semibold text-slate-900 dark:text-slate-100">
              {scopeLabel} <span className="font-normal text-slate-600 dark:text-slate-400">· {memoriesCount(model.total)}</span>
            </h2>
          </div>
        )}
        {aggregate && (
          <p className="mb-2 rounded-md border border-indigo-200 bg-indigo-50 px-3 py-2 text-sm text-indigo-950 dark:border-indigo-900 dark:bg-indigo-950 dark:text-indigo-100">
            You have {memoriesCount(model.total)}, more than the map draws at once. It shows your categories; open one to see
            its memories.
          </p>
        )}
        <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
          {!aggregate && (
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
          )}
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
            {canWrite && !aggregate && (
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

        {/* An application: the keys below drive the camera, so screen readers should pass them through. */}
        {/* oxlint-disable-next-line jsx-a11y/no-noninteractive-element-interactions */}
        <div
          ref={container}
          role="application"
          aria-roledescription="map"
          tabIndex={0}
          onKeyDown={onMapKey}
          aria-label={
            aggregate
              ? `Map of ${model.total} memories in ${model.categories.length} categories, showing categories only. The categories list beside it opens each one.`
              : `Map of ${model.memories.length} memories in ${model.categories.length} categories, showing ${LEVEL_NAMES[level].toLowerCase()}. The list view has the same memories as text.`
          }
          aria-describedby={`${sliderId}-keys`}
          className="relative h-[62vh] min-h-80 w-full overflow-hidden rounded-lg border border-slate-200 bg-white outline-none focus-visible:ring-2 focus-visible:ring-indigo-600 focus-visible:ring-offset-2 dark:border-slate-800 dark:bg-slate-950 dark:focus-visible:ring-offset-slate-950"
        />
        {rendererFailed && (
          <p role="alert" className="mt-2 text-sm text-red-700 dark:text-red-400">
            <span aria-hidden="true">⚠ </span>The map could not start in this browser (it needs WebGL). The list view shows the same memories.
          </p>
        )}
        {aggregate ? (
          <p className="mt-2 text-xs text-slate-600 dark:text-slate-400">
            {model.categories.length} categories · {memoriesCount(model.total)}. Click a category, or pick it from the list,
            to open its map.
          </p>
        ) : (
          <p className="mt-2 text-xs text-slate-600 dark:text-slate-400">
            {memoriesCount(model.memories.length)} · {similarCount} similarity links
            {relationCount > 0 && ` · ${relationCount} stated relations`}
            {map.data.edgesOmitted && " · too many memories to compare, links omitted"}. Scroll or pinch to zoom; click a
            category to open it, a memory to edit it.
            {canWrite && " To select memories for a bulk edit, turn on Lasso select (or hold Shift) and draw around them."}
          </p>
        )}
        <p id={`${sliderId}-keys`} className="mt-1 text-xs text-slate-600 dark:text-slate-400">
          Keyboard: focus the map, then <kbd>+</kbd> and <kbd>−</kbd> zoom, <kbd>0</kbd> fits
          {!aggregate && (
            <>
              , <kbd>1</kbd> <kbd>2</kbd> <kbd>3</kbd> jump to a level
            </>
          )}
          , the arrow keys pan{canWrite && !aggregate && ", Escape clears a selection"}.
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
                    {...(aggregate
                      ? { "aria-label": `Open ${category.label}, ${memoriesCount(category.count)}` }
                      : { "aria-pressed": shown })}
                    onClick={() => (aggregate ? openCategory(category.category) : toggleCategory(category.category))}
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
                    {!aggregate && <span className="sr-only">{shown ? "(shown)" : "(hidden)"}</span>}
                  </button>
                </li>
              );
            })}
          </ul>
        </section>

        {!aggregate && (
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
        )}
      </aside>
    </div>
  );
}
