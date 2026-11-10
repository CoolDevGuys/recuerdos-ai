import { Header } from "./components/Header";
import { LoginScreen } from "./components/LoginScreen";
import { MemoryList } from "./components/MemoryList";
import type { ApiClient } from "./api/client";
import { describeSignInError, useSession } from "./state/session";
import { useMemories } from "./state/memories";

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
          <Memories client={session.client} user={session.identity.user} />
        ) : (
          <p role="alert" className="rounded-lg border border-amber-300 bg-amber-50 p-4 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-200">
            This key cannot read memories (it has no read scope). Sign in with a key issued with{" "}
            <code className="font-mono">--scopes read</code> or more.
          </p>
        )}
      </main>
    </div>
  );
}

function Memories({ client, user }: { client: ApiClient; user: string }) {
  const memories = useMemories(client, user);

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
  return <MemoryList memories={memories.data} />;
}
