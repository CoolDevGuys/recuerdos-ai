import { ApiError, type ApiClient } from "./client";
import type { Identity, Memory } from "./types";

/**
 * An in-memory daemon for tests: one user per key, each with their own
 * memories. A key it does not know is refused the way the daemon refuses
 * it, so tests exercise the same error path the real client takes.
 */
export class InMemoryApiClient implements ApiClient {
  private readonly users: Map<string | null, { identity: Identity; memories: Memory[] }>;
  private readonly key: string | null;

  constructor(
    users: Map<string | null, { identity: Identity; memories: Memory[] }>,
    key: string | null,
  ) {
    this.users = users;
    this.key = key;
  }

  async ping(): Promise<Identity> {
    return this.account().identity;
  }

  async listMemories(): Promise<Memory[]> {
    const account = this.account();
    if (!account.identity.scopes.some((s) => s === "read" || s === "admin")) {
      throw new ApiError(403, "forbidden", "this key lacks the read scope");
    }
    return account.memories.map((m) => ({ ...m, tags: [...m.tags] }));
  }

  private account() {
    const account = this.users.get(this.key);
    if (account === undefined) throw new ApiError(401, "unauthorized", "invalid API key");
    return account;
  }
}

/** A memory with sensible defaults, for building fixtures. */
export function memory(overrides: Partial<Memory> & Pick<Memory, "id" | "content">): Memory {
  return {
    category: "preference.coding",
    subcategory: null,
    tags: [],
    confidence: 1,
    created_at: "2026-08-11T10:00:00Z",
    updated_at: "2026-08-11T10:00:00Z",
    superseded_by: null,
    ...overrides,
  };
}
