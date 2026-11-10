import { useCallback, useMemo, useState } from "react";
import type { ApiClient } from "./api/client";
import type { Memory } from "./api/types";
import { AddMemory, type Added } from "./components/AddMemory";
import { Header } from "./components/Header";
import { LoginScreen } from "./components/LoginScreen";
import { MemoryEditor } from "./components/MemoryEditor";
import { MemoryList } from "./components/MemoryList";
import { Panel } from "./components/Panel";
import { Toasts } from "./components/Toasts";
import type { Access } from "./domain/access";
import { categoryLabel } from "./domain/memories";
import { useCategories, useMemories, useMemoryActions } from "./state/memories";
import { describeSignInError, useSession } from "./state/session";
import { useToasts } from "./state/toasts";

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
  const [highlighted, setHighlighted] = useState<string | null>(null);

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

  return (
    <>
      <MemoryList
        memories={memories.data}
        labelOf={labelOf}
        highlighted={highlighted}
        canWrite={access.canWrite}
        onOpen={(memory) => setOpen({ kind: "edit", memory })}
        onAdd={() => setOpen({ kind: "add" })}
      />
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
          <AddMemory categories={categoryList} actions={actions} onAdded={onAdded} />
        </Panel>
      )}
    </>
  );
}
