import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { ApiError, NetworkError, type ApiClient } from "../api/client";
import type { Identity } from "../api/types";
import { accessFor, type Access } from "../domain/access";
import type { KeyStore } from "./keyStore";
import { clearLayoutSeeds } from "../map/layoutCache";

export type ClientFactory = (key: string | null) => ApiClient;

export type Session =
  | { status: "checking" }
  | { status: "signed-out"; notice: string | null }
  | {
      status: "signed-in";
      client: ApiClient;
      identity: Identity;
      access: Access;
      /** The daemon runs with `auth.mode = "none"`: no key was needed. */
      keyless: boolean;
    };

interface SessionContextValue {
  session: Session;
  /** Resolves on success; rejects with the reason the key was refused. */
  signIn(key: string): Promise<void>;
  signOut(): void;
}

const SessionContext = createContext<SessionContextValue | null>(null);

export function SessionProvider({
  createClient,
  keyStore,
  children,
}: {
  createClient: ClientFactory;
  keyStore: KeyStore;
  children: ReactNode;
}) {
  const [session, setSession] = useState<Session>({ status: "checking" });

  useEffect(() => {
    let cancelled = false;
    restore(createClient, keyStore).then((restored) => {
      if (!cancelled) setSession(restored);
    });
    return () => {
      cancelled = true;
    };
  }, [createClient, keyStore]);

  const signIn = useCallback(
    async (rawKey: string) => {
      const key = rawKey.trim();
      const client = createClient(key);
      const identity = await client.ping();
      keyStore.write(key);
      setSession(signedIn(client, identity, false));
    },
    [createClient, keyStore],
  );

  const signOut = useCallback(() => {
    keyStore.clear();
    clearLayoutSeeds();
    setSession({ status: "signed-out", notice: null });
  }, [keyStore]);

  const value = useMemo(() => ({ session, signIn, signOut }), [session, signIn, signOut]);
  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>;
}

export function useSession(): SessionContextValue {
  const value = useContext(SessionContext);
  if (value === null) throw new Error("useSession must be used inside a SessionProvider");
  return value;
}

function signedIn(client: ApiClient, identity: Identity, keyless: boolean): Session {
  return { status: "signed-in", client, identity, access: accessFor(identity.scopes), keyless };
}

/**
 * On load: a key kept from earlier in this tab, else a keyless probe (a
 * daemon in `auth.mode = "none"` answers it), else the sign-in screen.
 */
async function restore(createClient: ClientFactory, keyStore: KeyStore): Promise<Session> {
  const stored = keyStore.read();
  if (stored !== null) {
    const client = createClient(stored);
    try {
      return signedIn(client, await client.ping(), false);
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        keyStore.clear();
        return { status: "signed-out", notice: "Your key is no longer accepted. Sign in again." };
      }
      return { status: "signed-out", notice: describeSignInError(error) };
    }
  }

  const keyless = createClient(null);
  try {
    return signedIn(keyless, await keyless.ping(), true);
  } catch (error) {
    if (error instanceof NetworkError) return { status: "signed-out", notice: describeSignInError(error) };
    return { status: "signed-out", notice: null };
  }
}

/** A specific, actionable message for a failed sign-in. */
export function describeSignInError(error: unknown): string {
  if (error instanceof NetworkError) {
    return "The memory server could not be reached. Check that it is running, then try again.";
  }
  if (error instanceof ApiError) {
    if (error.status === 401) return "This key was not accepted. It may be mistyped or revoked.";
    if (error.status === 429) return "Too many attempts. Wait a minute, then try again.";
    return `The server refused the request: ${error.message}`;
  }
  return "Something went wrong while signing in. Try again.";
}
