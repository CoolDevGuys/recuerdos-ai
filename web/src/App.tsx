import { lazy, Suspense, useCallback, useMemo, useState } from "react";
import type { ApiClient } from "./api/client";
import type { Memory } from "./api/types";
import { AddMemory, type Added } from "./components/AddMemory";
import { BulkEditor } from "./components/BulkEditor";
import { CategoriesView } from "./components/CategoriesView";
import { Header } from "./components/Header";
import { LoginScreen } from "./components/LoginScreen";
import { MemoryEditor } from "./components/MemoryEditor";
import { MemoryList } from "./components/MemoryList";
import { Panel } from "./components/Panel";
import { Toasts } from "./components/Toasts";
import type { Access } from "./domain/access";
import { fileable } from "./domain/categories";
import { categoryLabel } from "./domain/memories";
import { useCategories, useMemories, useMemoryActions } from "./state/memories";
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
          <Memories client={session.client} user={session.identity.user} access={session.access} />
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

function Memories({ client, user, access }: { client: ApiClient; user: string; access: Access }) {
  const memories = useMemories(client, user);
  const categories = useCategories(client, user);
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
  const labelOf = useMemo(() => {
    const labels = new Map((categories.data ?? []).map((c) => [c.id, c.label]));
    return (id: string) => labels.get(id) ?? categoryLabel(id);
  }, [categories.data]);

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
      <ViewTabs view={view} onChange={setView} />
      {view === "list" && (
        <>
          {selected.size > 0 && (
            <div className="mb-4">
              <BulkEditor
                selected={[...selected]}
                categories={categoryList}
                actions={actions}
                onDone={clearSelection}
                onClear={clearSelection}
              />
            </div>
          )}
          <MemoryList
            memories={memories.data}
            labelOf={labelOf}
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
            labelOf={labelOf}
            onOpenMemory={openMemory}
            categories={categoryList}
            canWrite={access.canWrite}
          />
        </Suspense>
      )}
      {view === "categories" && <CategoriesView client={client} user={user} access={access} actions={actions} />}
      {open.kind === "edit" && (
        <Panel title={access.canWrite ? "Edit memory" : "Memory"} onClose={close}>
          <MemoryEditor
            memory={open.memory}
            categories={categoryList}
            canWrite={access.canWrite}
            actions={actions}
            onSaved={onSaved}
            onForgotten={onForgotten}
          />
        </Panel>
      )}
      {open.kind === "add" && (
        <Panel title="Add a memory" onClose={close}>
          <AddMemory categories={fileable(categoryList)} actions={actions} onAdded={onAdded} />
        </Panel>
      )}
    </>
  );
}

function ViewTabs({ view, onChange }: { view: View; onChange: (view: View) => void }) {
  const tab = (value: View, label: string) => (
    <a
      href={VIEW_PATHS[value]}
      aria-current={view === value ? "page" : undefined}
      onClick={(event) => {
        event.preventDefault();
        onChange(value);
      }}
      className={
        "border-b-2 px-3 py-2 text-sm font-medium " +
        (view === value
          ? "border-indigo-600 text-indigo-700 dark:text-indigo-300"
          : "border-transparent text-slate-600 hover:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100")
      }
    >
      {label}
    </a>
  );
  return (
    <nav aria-label="Views" className="mb-5 flex gap-2 border-b border-slate-200 dark:border-slate-800">
      {tab("list", "List")}
      {tab("map", "Map")}
      {tab("categories", "Categories")}
    </nav>
  );
}
