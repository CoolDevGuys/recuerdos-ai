import { lazy, Suspense, useCallback, useMemo, useState } from "react";
import type { ApiClient } from "./api/client";
import type { Memory } from "./api/types";
import { AboutView } from "./components/AboutView";
import { AddMemory, type Added } from "./components/AddMemory";
import { BulkEditor } from "./components/BulkEditor";
import { CategoriesView } from "./components/CategoriesView";
import { Header } from "./components/Header";
import { LoginScreen } from "./components/LoginScreen";
import { MemoryEditor } from "./components/MemoryEditor";
import { MemoryList } from "./components/MemoryList";
import { Panel } from "./components/Panel";
import { ProjectsView } from "./components/ProjectsView";
import { ReviewQueue } from "./components/ReviewQueue";
import { TopicsView } from "./components/TopicsView";
import { Toasts } from "./components/Toasts";
import type { Access } from "./domain/access";
import { fileable } from "./domain/categories";
import type { MemoryFilter } from "./domain/memories";
import { topicLabelOf } from "./domain/structureForm";
import { useCategories, useMemories, useMemoryActions, useProjects, useTopics } from "./state/memories";
import { describeSignInError, useSession } from "./state/session";
import { useView, VIEW_PATHS, type View } from "./state/route";
import { useToasts } from "./state/toasts";

// The map pulls in graphology and its layout; the list does not need them.
const MapView = lazy(() => import("./components/map/MapView"));

/** How long the Undo on a forgotten memory stays offered. */
const UNDO_WINDOW_MS = 8000;

export function App() {
  const { session, signIn, signOut } = useSession();

  if (session.status === "checking") {
    return (
      <p role="status" className="p-8 text-center text-sm text-slate-600 dark:text-slate-400">
        Connecting…
      </p>
    );
  }

  if (session.status === "signed-out") {
    return (
      <LoginScreen
        notice={session.notice}
        onSubmit={async (key) => {
          try {
            await signIn(key);
            return null;
          } catch (error) {
            return describeSignInError(error);
          }
        }}
      />
    );
  }

  return (
    <div className="min-h-screen bg-slate-50 dark:bg-slate-950">
      <Header user={session.identity.user} access={session.access} keyless={session.keyless} onSignOut={signOut} />
      <main className="mx-auto max-w-6xl px-4 py-6">
        {session.access.canRead ? (
          <Memories
            client={session.client}
            user={session.identity.user}
            access={session.access}
            understanding={session.identity.understanding === true}
          />
        ) : (
          <p role="alert" className="rounded-lg border border-amber-300 bg-amber-50 p-4 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-200">
            This key cannot read memories (it has no read scope). Sign in with a key issued with{" "}
            <code className="font-mono">--scopes read</code> or more.
          </p>
        )}
      </main>
      <Toasts />
    </div>
  );
}

type Open = { kind: "none" } | { kind: "edit"; memory: Memory } | { kind: "add" };

function Memories({
  client,
  user,
  access,
  understanding,
}: {
  client: ApiClient;
  user: string;
  access: Access;
  /** A model is configured: the review queue offers Suggest. */
  understanding: boolean;
}) {
  const memories = useMemories(client, user);
  const categories = useCategories(client, user);
  const topics = useTopics(client, user);
  const projects = useProjects(client, user);
  // Where the list opens when another tab sends the user to it.
  const [listPreset, setListPreset] = useState<{ key: number; filter: Partial<MemoryFilter> }>({ key: 0, filter: {} });
  const actions = useMemoryActions(client, user);
  const { show } = useToasts();
  const [open, setOpen] = useState<Open>({ kind: "none" });
  const [view, setView] = useView();
  const [highlighted, setHighlighted] = useState<string | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());

  const select = useCallback((ids: readonly string[], on: boolean) => {
    setSelected((current) => {
      const next = new Set(current);
      for (const id of ids) {
        if (on) next.add(id);
        else next.delete(id);
      }
      return next;
    });
  }, []);
  const clearSelection = useCallback(() => setSelected(new Set()), []);

  const close = useCallback(() => setOpen({ kind: "none" }), []);
  const topicList = useMemo(() => (topics.data ?? []).map((t) => ({ id: t.id, label: t.label })), [topics.data]);
  const topicLabel = useMemo(() => {
    const labels = new Map(topicList.map((t) => [t.id, t.label]));
    return (id: string) => labels.get(id) ?? topicLabelOf(id);
  }, [topicList]);
  const projectNames = useMemo(() => (projects.data ?? []).map((p) => p.name), [projects.data]);
  const showInList = (filter: Partial<MemoryFilter>) => {
    setListPreset((current) => ({ key: current.key + 1, filter }));
    setView("list");
  };

  const undoForget = async (memory: Memory) => {
    const outcome = await actions.restore(memory.id);
    if (outcome.kind === "failed") {
      show({ tone: "error", message: `It could not be restored: ${outcome.message}` });
      return;
    }
    setHighlighted(memory.id);
    show({ tone: "success", message: "Memory restored." });
  };

  const onForgotten = (memory: Memory) => {
    close();
    show(
      { tone: "info", message: "Memory forgotten.", action: { label: "Undo", run: () => void undoForget(memory) } },
      UNDO_WINDOW_MS,
    );
  };

  const onSaved = (memory: Memory) => {
    close();
    setHighlighted(memory.id);
    show({ tone: "success", message: "Changes saved." });
  };

  const onAdded = (added: Added) => {
    close();
    if (added.kind === "saved") {
      setHighlighted(added.memory.id);
      show({ tone: "success", message: "Memory saved." });
    } else if (added.kind === "known") {
      setHighlighted(added.memory.id);
      show({ tone: "info", message: "You already have this memory, so nothing new was saved." });
    } else {
      setHighlighted(added.firstId);
      show({ tone: added.tone, message: added.message }, 8000);
    }
  };

  if (memories.isPending) {
    return <p role="status" className="text-sm text-slate-600 dark:text-slate-400">Loading memories…</p>;
  }
  if (memories.isError) {
    return (
      <div role="alert" className="rounded-lg border border-red-300 bg-red-50 p-4 text-sm text-red-900 dark:border-red-800 dark:bg-red-950 dark:text-red-200">
        <p>The memories could not be loaded: {memories.error.message}</p>
        <button type="button" onClick={() => void memories.refetch()} className="mt-2 font-medium underline">
          Try again
        </button>
      </div>
    );
  }

  const categoryList = categories.data ?? [];
  const openMemory = (memory: Memory) => setOpen({ kind: "edit", memory });

  return (
    <>
      <ViewTabs view={view} onChange={setView} reviewCount={memories.data.filter((m) => m.needs_review).length} />
      {view === "list" && (
        <>
          {selected.size > 0 && (
            <div className="mb-4">
              <BulkEditor
                selected={[...selected]}
                topics={topicList}
                projects={projectNames}
                actions={actions}
                onDone={clearSelection}
                onClear={clearSelection}
              />
            </div>
          )}
          <MemoryList
            key={listPreset.key}
            initialFilter={listPreset.filter}
            memories={memories.data}
            topicLabel={topicLabel}
            highlighted={highlighted}
            canWrite={access.canWrite}
            onOpen={openMemory}
            onAdd={() => setOpen({ kind: "add" })}
            selected={selected}
            onSelect={select}
          />
        </>
      )}
      {view === "map" && (
        <Suspense fallback={<p role="status" className="text-sm text-slate-600 dark:text-slate-400">Loading the map…</p>}>
          <MapView
            client={client}
            user={user}
            actions={actions}
            labelOf={topicLabel}
            onOpenMemory={openMemory}
            topics={topicList}
            projects={projectNames}
            canWrite={access.canWrite}
          />
        </Suspense>
      )}
      {view === "review" && (
        <ReviewQueue
          memories={memories.data}
          topics={topicList}
          projects={projectNames}
          canWrite={access.canWrite}
          canSuggest={understanding}
          actions={actions}
          onOpen={openMemory}
        />
      )}
      {view === "projects" && (
        <ProjectsView
          client={client}
          user={user}
          canWrite={access.canWrite}
          actions={actions}
          onShowMemories={(name) => showInList({ scope: `project:${name}` })}
        />
      )}
      {view === "topics" && (
        <TopicsView
          client={client}
          user={user}
          canWrite={access.canWrite}
          actions={actions}
          onShowMemories={(topic) => showInList({ topic })}
        />
      )}
      {view === "categories" && <CategoriesView client={client} user={user} access={access} actions={actions} />}
      {view === "about" && <AboutView client={client} user={user} canWrite={access.canWrite} />}
      {open.kind === "edit" && (
        <Panel title={access.canWrite ? "Edit memory" : "Memory"} onClose={close}>
          <MemoryEditor
            memory={open.memory}
            categories={categoryList}
            canWrite={access.canWrite}
            actions={actions}
            onSaved={onSaved}
            onForgotten={onForgotten}
            topics={topicList}
            projects={projectNames}
          />
        </Panel>
      )}
      {open.kind === "add" && (
        <Panel title="Add a memory" onClose={close}>
          <AddMemory
            categories={fileable(categoryList)}
            topics={topicList}
            projects={projectNames}
            actions={actions}
            onAdded={onAdded}
          />
        </Panel>
      )}
    </>
  );
}

function ViewTabs({ view, onChange, reviewCount }: { view: View; onChange: (view: View) => void; reviewCount: number }) {
  const tab = (value: View, label: string, badge?: number) => (
    <a
      href={VIEW_PATHS[value]}
      aria-current={view === value ? "page" : undefined}
      aria-label={badge !== undefined && badge > 0 ? `${label}, ${badge} to review` : undefined}
      onClick={(event) => {
        event.preventDefault();
        onChange(value);
      }}
      className={
        "shrink-0 whitespace-nowrap border-b-2 px-3 py-2 text-sm font-medium " +
        (view === value
          ? "border-indigo-600 text-indigo-700 dark:text-indigo-300"
          : "border-transparent text-slate-600 hover:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100")
      }
    >
      {label}
      {badge !== undefined && badge > 0 && (
        <span className="ml-1.5 rounded-full bg-amber-100 px-1.5 py-0.5 text-[11px] font-semibold text-amber-900 dark:bg-amber-900/60 dark:text-amber-100">
          {badge}
        </span>
      )}
    </a>
  );
  return (
    <nav aria-label="Views" className="mb-5 flex gap-1 overflow-x-auto border-b border-slate-200 dark:border-slate-800">
      {tab("list", "List")}
      {tab("review", "Needs review", reviewCount)}
      {tab("map", "Map")}
      {tab("projects", "Projects")}
      {tab("topics", "Topics")}
      {tab("categories", "Categories")}
      {tab("about", "About")}
    </nav>
  );
}
