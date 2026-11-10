import { ApiError, type ApiClient } from "./client";
import type {
  BulkEdit,
  BulkResult,
  Category,
  CategoryChange,
  CategoryUpdate,
  Identity,
  IngestRequest,
  IngestResult,
  MapData,
  MapEdge,
  Memory,
  MemoryFields,
  MemoryPatch,
  RecallHit,
} from "./types";

interface Account {
  identity: Identity;
  memories: Memory[];
  forgotten: Memory[];
  /** Custom categories, id → the user's label (or null for the default wording). */
  custom: Map<string, string | null>;
  /** Built-ins the user relabelled. */
  relabelled: Map<string, string>;
}

const CUSTOM_ID = /^[a-z][a-z0-9._-]{0,63}$/;

/** The default wording of a category id, as the daemon's `Category::label`. */
export function defaultLabel(id: string): string {
  const builtIn = BUILT_IN.find(([b]) => b === id);
  if (builtIn !== undefined) return builtIn[1];
  const words = id.replace(/[._-]/g, " ").trim();
  return words === "" ? "Memory" : words[0]!.toUpperCase() + words.slice(1);
}

function isBuiltIn(id: string): boolean {
  return BUILT_IN.some(([b]) => b === id);
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
  /** The edges `getMap` reports, between whatever memories exist. */
  mapEdges: MapEdge[] = [];
  /** Above this many memories the whole-store map is categories only, as the daemon's `[ui].map_detail_limit`. */
  mapDetailLimit = Number.POSITIVE_INFINITY;
  /** Requests seen, for assertions about what was (not) sent. */
  readonly calls: { method: string; id?: string; version?: string | null; patch?: MemoryPatch }[] = [];

  addAccount(key: string | null, identity: Identity, memories: Memory[] = []): this {
    // Seeded like the daemon's catalog: every custom category already on a memory.
    const custom = new Map<string, string | null>();
    for (const m of memories) if (!isBuiltIn(m.category)) custom.set(m.category, null);
    this.accounts.set(key, { identity, memories: memories.map(copy), forgotten: [], custom, relabelled: new Map() });
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
    return categoriesOf(this.readable());
  }

  async createCategory(id: string, label: string | null): Promise<Category> {
    const account = this.writable();
    this.server.calls.push({ method: "createCategory", id });
    const normalised = id.trim().toLowerCase();
    if (isBuiltIn(normalised) || account.custom.has(normalised)) {
      throw new ApiError(409, "conflict", `category ${normalised} already exists`);
    }
    if (!CUSTOM_ID.test(normalised)) throw new ApiError(400, "validation_failed", `category id ${normalised} is not valid`);
    account.custom.set(normalised, blankToNull(label));
    return this.categoryNamed(account, normalised);
  }

  async updateCategory(id: string, update: CategoryUpdate): Promise<CategoryChange> {
    const account = this.writable();
    this.server.calls.push({ method: "updateCategory", id });
    this.requireKnown(account, id);
    let current = id;
    let moved = 0;
    if (update.id !== undefined) {
      if (isBuiltIn(id)) throw new ApiError(409, "conflict", `${id} is a built-in category`);
      const to = update.id.trim().toLowerCase();
      if (isBuiltIn(to) || account.custom.has(to)) throw new ApiError(409, "conflict", `category ${to} already exists`);
      if (!CUSTOM_ID.test(to)) throw new ApiError(400, "validation_failed", `category id ${to} is not valid`);
      account.custom.set(to, account.custom.get(id) ?? null);
      moved = this.move(account, id, to);
      account.custom.delete(id);
      current = to;
    }
    if (update.label !== undefined) {
      const label = blankToNull(update.label);
      if (isBuiltIn(current)) {
        if (label === null) account.relabelled.delete(current);
        else account.relabelled.set(current, label);
      } else {
        account.custom.set(current, label);
      }
    }
    return { category: this.categoryNamed(account, current), moved, reembedded: moved };
  }

  async mergeCategory(id: string, into: string): Promise<CategoryChange> {
    const account = this.writable();
    this.server.calls.push({ method: "mergeCategory", id });
    this.requireKnown(account, id);
    this.requireKnown(account, into);
    if (isBuiltIn(id)) throw new ApiError(409, "conflict", `${id} is a built-in category`);
    if (id === into) throw new ApiError(400, "validation_failed", "a category cannot be merged into itself");
    const moved = this.move(account, id, into);
    account.custom.delete(id);
    return { category: this.categoryNamed(account, into), moved, reembedded: moved };
  }

  async deleteCategory(id: string, moveTo: string | null): Promise<CategoryChange> {
    if (moveTo !== null) {
      const change = await this.mergeCategory(id, moveTo);
      return { ...change, category: null };
    }
    const account = this.writable();
    this.server.calls.push({ method: "deleteCategory", id });
    this.requireKnown(account, id);
    if (isBuiltIn(id)) throw new ApiError(409, "conflict", `${id} is a built-in category`);
    const members = account.memories.filter((m) => m.category === id).length;
    if (members > 0) throw new ApiError(409, "conflict", `${id} still holds ${members} memories; move them first`);
    account.custom.delete(id);
    return { category: null, moved: 0, reembedded: 0 };
  }

  async bulkEdit(ids: string[], edit: BulkEdit): Promise<BulkResult> {
    const account = this.writable();
    this.server.calls.push({ method: "bulk" });
    const unique = [...new Set(ids)];
    if (unique.length === 0) throw new ApiError(400, "validation_failed", "no memories were selected");
    const missing = unique.filter((id) => !account.memories.some((m) => m.id === id));
    if (missing.length > 0) {
      throw new ApiError(404, "not_found", `these memories were not found: ${missing.join(", ")}`);
    }
    if (edit.category !== undefined) this.requireKnown(account, edit.category, 400);
    const removed = new Set((edit.removeTags ?? []).map((t) => t.trim().toLowerCase()));
    const added = (edit.addTags ?? []).map((t) => t.trim().toLowerCase()).filter((t) => t !== "");
    const now = this.server.tick();
    const result = unique.map((id) => {
      const memory = this.find(account, id);
      if (edit.category !== undefined) memory.category = edit.category;
      if (edit.subcategory !== undefined) memory.subcategory = edit.subcategory;
      if (removed.size > 0 || added.length > 0) {
        memory.tags = [...new Set([...memory.tags.filter((t) => !removed.has(t)), ...added])];
      }
      memory.updated_at = now;
      return copy(memory);
    });
    return { memories: result, changed: result.length, reembedded: 0 };
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

  async getMap(category: string | null = null): Promise<MapData> {
    const account = this.readable();
    const generation = String(this.server.calls.length);
    this.server.calls.push({ method: "map", ...(category !== null && { id: category }) });
    const inScope = account.memories.filter((m) => category === null || m.category === category);
    const aggregate = category === null && inScope.length > this.server.mapDetailLimit;
    const nodes = aggregate ? [] : inScope;
    const present = new Set(nodes.map((m) => m.id));
    const categories = new Map<string, { count: number; subs: Map<string, number> }>();
    for (const m of inScope) {
      const entry = categories.get(m.category) ?? { count: 0, subs: new Map<string, number>() };
      entry.count += 1;
      if (m.subcategory !== null) entry.subs.set(m.subcategory, (entry.subs.get(m.subcategory) ?? 0) + 1);
      categories.set(m.category, entry);
    }
    const label = (id: string) => account.relabelled.get(id) ?? account.custom.get(id) ?? defaultLabel(id);
    return {
      generation,
      minWeight: 0.15,
      edgesOmitted: false,
      detail: aggregate ? "categories" : "memories",
      total: inScope.length,
      category,
      nodes: nodes.map(copy),
      edges: this.server.mapEdges.filter((e) => present.has(e.source) && present.has(e.target)),
      categories: [...categories].map(([id, entry]) => ({
        id,
        label: label(id),
        built_in: BUILT_IN.some(([b]) => b === id),
        count: entry.count,
        subcategories: [...entry.subs].map(([name, count]) => ({ name, count })),
      })),
      categoryEdges: [],
    };
  }

  async recall(query: string, limit: number): Promise<RecallHit[]> {
    const account = this.readable();
    this.server.calls.push({ method: "recall" });
    const words = query.toLowerCase().split(/\W+/).filter((w) => w.length > 2);
    return account.memories
      .map((m) => ({ m, hits: words.filter((w) => m.content.toLowerCase().includes(w)).length }))
      .filter(({ hits }) => hits > 0)
      .sort((a, b) => b.hits - a.hits)
      .slice(0, limit)
      .map(({ m, hits }, i) => ({
        memory: copy(m),
        relevance: Math.min(1, hits / Math.max(1, words.length)),
        matched: { vector: i + 1, keyword: i + 1, graph: null, rerank: null },
      }));
  }

  private requireKnown(account: Account, id: string, status: 400 | 404 = 404): void {
    if (!isBuiltIn(id) && !account.custom.has(id)) {
      throw new ApiError(status, status === 404 ? "not_found" : "validation_failed", `unknown category ${id}`);
    }
  }

  private move(account: Account, from: string, to: string): number {
    const now = this.server.tick();
    let moved = 0;
    for (const m of account.memories) {
      if (m.category === from) {
        m.category = to;
        m.updated_at = now;
        moved += 1;
      }
    }
    return moved;
  }

  private categoryNamed(account: Account, id: string): Category {
    const category = categoriesOf(account).find((c) => c.id === id);
    if (category === undefined) throw new ApiError(404, "not_found", `category ${id} not found`);
    return category;
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

function blankToNull(label: string | null | undefined): string | null {
  const trimmed = label?.trim() ?? "";
  return trimmed === "" ? null : trimmed;
}

/** The category list as the daemon builds it: built-ins, custom, then leftovers. */
function categoriesOf(account: Account): Category[] {
  const counts = new Map<string, number>();
  for (const m of account.memories) counts.set(m.category, (counts.get(m.category) ?? 0) + 1);
  const entry = (id: string, label: string | null, known: boolean): Category => ({
    id,
    label: label ?? defaultLabel(id),
    default_label: defaultLabel(id),
    built_in: isBuiltIn(id),
    known,
    count: counts.get(id) ?? 0,
  });
  const builtIn = BUILT_IN.map(([id]) => entry(id, account.relabelled.get(id) ?? null, true));
  const custom = [...account.custom].sort(([a], [b]) => a.localeCompare(b)).map(([id, label]) => entry(id, label, true));
  const leftovers = [...counts.keys()]
    .filter((id) => !isBuiltIn(id) && !account.custom.has(id))
    .sort()
    .map((id) => entry(id, null, false));
  return [...builtIn, ...custom, ...leftovers];
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
