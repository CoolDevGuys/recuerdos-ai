import { ApiError, type ApiClient } from "./client";
import type {
  Category,
  Identity,
  IngestRequest,
  IngestResult,
  Memory,
  MemoryFields,
  MemoryPatch,
} from "./types";

interface Account {
  identity: Identity;
  memories: Memory[];
  forgotten: Memory[];
}

const BUILT_IN: [string, string][] = [
  ["preference.coding", "Coding preference"],
  ["preference.personal", "Personal preference"],
  ["decision", "Decision"],
  ["fact.project", "Project fact"],
  ["fact.person", "Fact about a person"],
  ["experience", "Experience"],
  ["skill", "Skill"],
  ["reference", "Reference"],
];

/**
 * An in-memory daemon for tests: one account per key (`null` = a daemon
 * in `auth.mode = "none"`), enforcing what the real one enforces — scopes,
 * per-user isolation, `If-Match` versions, restore-only-what-was-forgotten
 * — so components are tested against the same failures they meet in
 * production. `failNext` injects one error into the next call.
 */
export class FakeServer {
  private readonly accounts = new Map<string | null, Account>();
  private clock = Date.parse("2026-11-01T00:00:00Z");
  private sequence = 0;
  private failure: ApiError | null = null;
  /** What `ingest` answers; `stored` writes one memory with the submitted text. */
  ingestOutcome: IngestResult["outcome"] = "stored";
  understanding = true;
  /** Requests seen, for assertions about what was (not) sent. */
  readonly calls: { method: string; id?: string; version?: string | null; patch?: MemoryPatch }[] = [];

  addAccount(key: string | null, identity: Identity, memories: Memory[] = []): this {
    this.accounts.set(key, { identity, memories: memories.map(copy), forgotten: [] });
    return this;
  }

  client(key: string | null): ApiClient {
    return new FakeClient(this, key);
  }

  failNext(error: ApiError): void {
    this.failure = error;
  }

  /** Simulates another client (an agent, another tab) editing a memory. */
  editElsewhere(key: string | null, id: string, patch: MemoryPatch): void {
    const memory = this.account(key).memories.find((m) => m.id === id);
    if (memory === undefined) throw new Error(`no memory ${id}`);
    Object.assign(memory, patch, { updated_at: this.tick() });
  }

  memoriesOf(key: string | null): Memory[] {
    return this.account(key).memories.map(copy);
  }

  // --- used by FakeClient

  takeFailure(): void {
    const failure = this.failure;
    this.failure = null;
    if (failure !== null) throw failure;
  }

  account(key: string | null): Account {
    const account = this.accounts.get(key);
    if (account === undefined) throw new ApiError(401, "unauthorized", "invalid API key");
    return account;
  }

  tick(): string {
    this.clock += 1000;
    return new Date(this.clock).toISOString();
  }

  nextId(): string {
    this.sequence += 1;
    return `mem_new_${this.sequence}`;
  }
}

class FakeClient implements ApiClient {
  private readonly server: FakeServer;
  private readonly key: string | null;

  constructor(server: FakeServer, key: string | null) {
    this.server = server;
    this.key = key;
  }

  async ping(): Promise<Identity> {
    this.server.takeFailure();
    return this.server.account(this.key).identity;
  }

  async listMemories(): Promise<Memory[]> {
    return this.readable().memories.map(copy);
  }

  async getMemory(id: string): Promise<Memory> {
    return copy(this.find(this.readable(), id));
  }

  async listCategories(): Promise<Category[]> {
    const account = this.readable();
    const counts = new Map<string, number>();
    for (const m of account.memories) counts.set(m.category, (counts.get(m.category) ?? 0) + 1);
    const builtIn = BUILT_IN.map(([id, label]) => ({ id, label, built_in: true, count: counts.get(id) ?? 0 }));
    const custom = [...counts]
      .filter(([id]) => !BUILT_IN.some(([b]) => b === id))
      .map(([id, count]) => ({ id, label: id, built_in: false, count }));
    return [...builtIn, ...custom];
  }

  async createMemory(fields: MemoryFields): Promise<Memory> {
    const account = this.writable();
    this.server.calls.push({ method: "create" });
    const now = this.server.tick();
    const memory: Memory = {
      id: this.server.nextId(),
      ...fields,
      tags: [...fields.tags],
      confidence: 1,
      created_at: now,
      updated_at: now,
      superseded_by: null,
    };
    account.memories.push(memory);
    return copy(memory);
  }

  async ingest(request: IngestRequest): Promise<IngestResult> {
    const account = this.writable();
    this.server.calls.push({ method: "ingest" });
    if (this.server.ingestOutcome !== "stored") {
      return { outcome: this.server.ingestOutcome, memoryIds: [], understanding: this.server.understanding };
    }
    const memory = await this.createMemory({
      content: request.content,
      category: request.category ?? "fact.project",
      subcategory: null,
      tags: request.tags,
    });
    void account;
    return { outcome: "stored", memoryIds: [memory.id], understanding: this.server.understanding };
  }

  async updateMemory(id: string, patch: MemoryPatch, version: string | null): Promise<Memory> {
    const account = this.writable();
    this.server.calls.push({ method: "update", id, version, patch });
    const memory = this.find(account, id);
    if (version !== null && version !== memory.updated_at) {
      throw new ApiError(412, "precondition_failed", `memory ${id} changed since it was read`);
    }
    if (patch.content !== undefined && patch.content.trim() === "") {
      throw new ApiError(400, "validation_failed", "memory content is empty");
    }
    Object.assign(memory, patch, { updated_at: this.server.tick() });
    return copy(memory);
  }

  async forgetMemory(id: string): Promise<void> {
    const account = this.writable();
    this.server.calls.push({ method: "forget", id });
    const memory = this.find(account, id);
    account.memories = account.memories.filter((m) => m.id !== id);
    account.forgotten.push(memory);
  }

  async restoreMemory(id: string): Promise<Memory> {
    const account = this.writable();
    this.server.calls.push({ method: "restore", id });
    const memory = account.forgotten.find((m) => m.id === id);
    if (memory === undefined) throw new ApiError(404, "not_found", `memory ${id} not found`);
    account.forgotten = account.forgotten.filter((m) => m.id !== id);
    account.memories.push(memory);
    return copy(memory);
  }

  private readable(): Account {
    this.server.takeFailure();
    const account = this.server.account(this.key);
    if (!account.identity.scopes.some((s) => s === "read" || s === "admin")) {
      throw new ApiError(403, "forbidden", "this key lacks the read scope");
    }
    return account;
  }

  private writable(): Account {
    this.server.takeFailure();
    const account = this.server.account(this.key);
    if (!account.identity.scopes.some((s) => s === "write" || s === "admin")) {
      throw new ApiError(403, "forbidden", "this key lacks the write scope");
    }
    return account;
  }

  private find(account: Account, id: string): Memory {
    const memory = account.memories.find((m) => m.id === id);
    if (memory === undefined) throw new ApiError(404, "not_found", `memory ${id} not found`);
    return memory;
  }
}

function copy(memory: Memory): Memory {
  return { ...memory, tags: [...memory.tags] };
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
