import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeServer, memory } from "../api/fake";
import type { Memory } from "../api/types";
import { memoryKeyStore, type KeyStore } from "../state/keyStore";
import { SessionProvider } from "../state/session";
import { ToastProvider } from "../state/toasts";

export const ALEX_KEY = "ra_live_1f4c8a209d4e5f6a7b8c9d0e1f2a3b4c5d6e7f80";
export const READER_KEY = "ra_live_0000000000000000000000000000000000000001";
export const UNKNOWN_KEY = "ra_live_ffffffffffffffffffffffffffffffffffffffff";

export const ALEX_MEMORIES: Memory[] = [
  memory({ id: "a", content: "The user prefers pnpm", tags: ["package-management"], updated_at: "2026-08-31T00:00:00Z" }),
  memory({ id: "b", content: "Testing follows TDD", subcategory: "testing", updated_at: "2026-08-12T00:00:00Z" }),
  memory({ id: "c", content: "Frontend uses Next.js", category: "fact.project", updated_at: "2026-08-11T00:00:00Z" }),
];

export function standardServer(): FakeServer {
  return new FakeServer()
    .addAccount(ALEX_KEY, { user: "alex", scopes: ["read", "write"] }, ALEX_MEMORIES)
    .addAccount(READER_KEY, { user: "reader", scopes: ["read"] }, ALEX_MEMORIES);
}

export function renderApp({
  server = standardServer(),
  keyStore = memoryKeyStore(),
}: { server?: FakeServer; keyStore?: KeyStore } = {}) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <SessionProvider createClient={(key) => server.client(key)} keyStore={keyStore}>
        <ToastProvider>
          <App />
        </ToastProvider>
      </SessionProvider>
    </QueryClientProvider>,
  );
  return { server, keyStore };
}

export async function signIn(key: string) {
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText("API key"), key);
  await user.click(screen.getByRole("button", { name: "Sign in" }));
  return user;
}

/** Signed in as alex with the list loaded. */
export async function signedInAsAlex(server = standardServer()) {
  renderApp({ server, keyStore: memoryKeyStore(ALEX_KEY) });
  await screen.findByText("The user prefers pnpm");
  return { server, user: userEvent.setup() };
}
