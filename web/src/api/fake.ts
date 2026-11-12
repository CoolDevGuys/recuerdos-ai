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
  ConflictingMemory,
  MemoryStructure,
  RecallHit,
  SavedMemory,
  Project,
  ProjectChange,
  Suggestion,
  Topic,
  AppliesTo,
} from "./types";
import { KINDS, TOPICS } from "../domain/structureForm";

interface Account {
  identity: Identity;
  memories: Memory[];
  forgotten: Memory[];
  /** Custom categories, id → the user's label (or null for the default wording). */
  custom: Map<string, string | null>;
  /** Built-ins the user relabelled. */
  relabelled: Map<string, string>;
  /** The user's own topics, id → label (null = default wording). */
  topics: Map<string, string | null>;
  /** Built-in topics the user relabelled. */
  topicLabels: Map<string, string>;
  projects: Project[];
}

const PROJECT_NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;
const TOPIC_ID = /^[a-z][a-z0-9_-]{0,31}$/;

function topicDefaultLabel(id: string): string {
  const builtIn = TOPICS.find((t) => t.id === id);
  if (builtIn !== undefined) return builtIn.label.replace(/ \(.*\)$/, "");
  const words = id.replace(/[-_]/g, " ");
  return words[0]!.toUpperCase() + words.slice(1);
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
  /** What `suggest` proposes, by memory id. */
  suggestions = new Map<string, Omit<Suggestion, "id">>();
  /** Requests seen, for assertions about what was (not) sent. */
  readonly calls: {
    method: string;
    id?: string;
    version?: string | null;
    patch?: MemoryPatch;
    structure?: MemoryStructure;
  }[] = [];
  /** What the next structured save is refused with, the way the daemon's rules would. */
  rejectNextSave: { code: string; message: string; conflicts?: ConflictingMemory[] } | null = null;

  addAccount(key: string | null, identity: Identity, memories: Memory[] = []): this {
    // Seeded like the daemon's catalog: every custom category already on a memory.
    const custom = new Map<string, string | null>();
    for (const m of memories) if (!isBuiltIn(m.category)) custom.set(m.category, null);
    // And its projects: every project a memory names.
    const projects: Project[] = [];
    for (const m of memories) {
      const name = m.applies_to.project_name;
      if (name !== null && !projects.some((p) => p.name === name)) {
        projects.push({ id: `proj_${name}`, name, aliases: [], memories: 0 });
      }
    }
    this.accounts.set(key, {
      identity: { ...identity, understanding: this.understanding },
      memories: memories.map(copy),
      forgotten: [],
      custom,
      relabelled: new Map(),
      topics: new Map(),
      topicLabels: new Map(),
      projects,
    });
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
    return { ...this.server.account(this.key).identity, understanding: this.server.understanding };
  }

  async listTopics(): Promise<Topic[]> {
    return topicsOf(this.readable());
  }

  async createTopic(id: string, label: string | null): Promise<Topic> {
    const account = this.writable();
    this.server.calls.push({ method: "createTopic", id });
    const normalised = id.trim().toLowerCase();
    if (TOPICS.some((t) => t.id === normalised) || account.topics.has(normalised)) {
      throw new ApiError(409, "conflict", `you already have the topic ${normalised}`);
    }
    if (!TOPIC_ID.test(normalised)) throw new ApiError(400, "validation_failed", `topic id ${id} is not valid`);
    account.topics.set(normalised, blankToNull(label));
    return topicsOf(account).find((t) => t.id === normalised)!;
  }

  async relabelTopic(id: string, label: string | null): Promise<Topic> {
    const account = this.writable();
    this.server.calls.push({ method: "relabelTopic", id });
    const value = blankToNull(label);
    if (TOPICS.some((t) => t.id === id)) {
      if (value === null) account.topicLabels.delete(id);
      else account.topicLabels.set(id, value);
    } else if (account.topics.has(id)) {
      account.topics.set(id, value);
    } else {
      throw new ApiError(422, "unknown_topic", `You have no topic "${id}".`);
    }
    return topicsOf(account).find((t) => t.id === id)!;
  }

  async listProjects(): Promise<Project[]> {
    return projectsOf(this.readable());
  }

  async renameProject(name: string, newName: string): Promise<ProjectChange> {
    const account = this.writable();
    this.server.calls.push({ method: "renameProject", id: name });
    const project = this.project(account, name);
    const next = newName.trim().toLowerCase();
    if (!PROJECT_NAME.test(next)) {
      throw new ApiError(422, "invalid_value", `project name "${newName}" must be lowercase a-z, 0-9, '.', '_' or '-'`);
    }
    if (account.projects.some((p) => p.name === next && p.id !== project.id)) {
      throw new ApiError(409, "conflict", `Another project is already called ${next}; merge them instead.`);
    }
    const previous = project.name;
    if (next !== previous) project.aliases = [...new Set([...project.aliases, previous])].sort();
    project.name = next;
    for (const m of account.memories) {
      if (m.applies_to.project_name === previous) m.applies_to = { target: "project", project_name: next };
    }
    const view = projectsOf(account).find((p) => p.id === project.id)!;
    return { project: view, message: `Renamed ${previous} to ${next}.`, moved: 0 };
  }

  async mergeProject(name: string, into: string): Promise<ProjectChange> {
    const account = this.writable();
    this.server.calls.push({ method: "mergeProject", id: name });
    const from = this.project(account, name);
    const to = this.project(account, into);
    if (from.id === to.id) throw new ApiError(422, "invalid_value", `${from.name} cannot be merged into itself.`);
    let moved = 0;
    const now = this.server.tick();
    for (const m of account.memories) {
      if (m.applies_to.project_name === from.name) {
        m.applies_to = { target: "project", project_name: to.name };
        m.updated_at = now;
        moved += 1;
      }
    }
    to.aliases = [...new Set([...to.aliases, ...from.aliases, from.name])].sort();
    account.projects = account.projects.filter((p) => p.id !== from.id);
    const view = projectsOf(account).find((p) => p.id === to.id)!;
    return { project: view, message: `Merged ${from.name} into ${to.name}: ${moved} moved.`, moved };
  }

  async suggest(ids: string[]): Promise<Suggestion[]> {
    const account = this.readable();
    this.server.calls.push({ method: "suggest" });
    if (!this.server.understanding) {
      throw new ApiError(422, "no_model", "Suggestions need a language model: set [understanding].provider.");
    }
    for (const id of ids) this.find(account, id);
    return ids.flatMap((id) => {
      const suggestion = this.server.suggestions.get(id);
      return suggestion === undefined ? [] : [{ id, ...suggestion }];
    });
  }

  private project(account: Account, name: string): Project {
    const wanted = name.trim().toLowerCase();
    const project = account.projects.find((p) => p.name === wanted || p.id === wanted || p.aliases.includes(wanted));
    if (project === undefined) {
      throw new ApiError(422, "unknown_project", `Unknown project "${name}"; known: ${account.projects.map((p) => p.name).join(", ") || "none yet"}.`);
    }
    return project;
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
    if (edit.appliesTo?.target === "project") this.project(account, edit.appliesTo.project_name ?? "");
    const structural = edit.kind !== undefined || edit.topic !== undefined || edit.appliesTo !== undefined;
    if (structural) {
      // Like the daemon: every memory must be able to take it, or none does.
      const kindFor = (m: Memory) => edit.kind ?? m.kind;
      const targetFor = (m: Memory) => edit.appliesTo?.target ?? m.applies_to.target;
      const refused = unique
        .map((id) => this.find(account, id))
        .filter((m) => !(KINDS.find((k) => k.id === kindFor(m))?.targets.includes(targetFor(m)) ?? true));
      if (refused.length > 0) {
        throw new ApiError(
          422,
          "invalid_value",
          `Nothing was changed: ${refused.length} of the ${unique.length} memories cannot take that.`,
        );
      }
    }
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
      if (edit.kind !== undefined) memory.kind = edit.kind;
      if (edit.topic !== undefined) memory.topic = edit.topic;
      if (edit.appliesTo !== undefined) memory.applies_to = { ...edit.appliesTo };
      if (structural) memory.origin = "user_confirmed";
      if (structural || edit.reviewed === true) memory.needs_review = false;
      memory.updated_at = now;
      return copy(memory);
    });
    return { memories: result, changed: result.length, reembedded: 0 };
  }

  async createMemory(fields: MemoryFields, structure?: MemoryStructure): Promise<SavedMemory> {
    const account = this.writable();
    const refusal = this.server.rejectNextSave;
    if (refusal !== null) {
      this.server.rejectNextSave = null;
      throw new ApiError(422, refusal.code, refusal.message, refusal.conflicts);
    }
    this.server.calls.push({ method: "create", ...(structure !== undefined && { structure }) });
    // Like the daemon: the same sentence again is already known.
    const same = account.memories.find(
      (m) => m.content.trim().toLowerCase() === fields.content.trim().toLowerCase() && !structure?.coexists_with?.includes(m.id),
    );
    if (same !== undefined) {
      return { memory: copy(same), status: "already_known", message: `Already known: "${same.content}".` };
    }
    const now = this.server.tick();
    const appliesTo: AppliesTo = {
      target: structure?.applies_to.target ?? "general",
      project_name: structure?.applies_to.project_name ?? null,
    };
    if (appliesTo.project_name !== null && !account.projects.some((p) => p.name === appliesTo.project_name)) {
      account.projects.push({ id: `proj_${appliesTo.project_name}`, name: appliesTo.project_name, aliases: [], memories: 0 });
    }
    const memory: Memory = {
      id: this.server.nextId(),
      ...fields,
      tags: [...fields.tags],
      confidence: 1,
      created_at: now,
      updated_at: now,
      superseded_by: null,
      kind: structure?.kind ?? "fact",
      topic: structure?.topic ?? "work",
      applies_to: appliesTo,
      origin: structure?.origin ?? "user_said",
      goal_status: structure?.kind === "goal" ? "active" : null,
      needs_review: structure === undefined,
      valid_until: null,
    };
    // A replaced memory leaves the list (it stays in the daemon's history).
    if (structure?.replaces !== undefined) {
      account.memories = account.memories.filter((m) => m.id !== structure.replaces);
    }
    account.memories.push(memory);
    return { memory: copy(memory), status: "stored", message: "Saved." };
  }

  async ingest(request: IngestRequest): Promise<IngestResult> {
    const account = this.writable();
    this.server.calls.push({ method: "ingest" });
    if (this.server.ingestOutcome !== "stored") {
      return { outcome: this.server.ingestOutcome, memoryIds: [], understanding: this.server.understanding };
    }
    const { memory } = await this.createMemory({
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
    if (patch.goal_status !== undefined && patch.goal_status !== "active" && patch.origin === undefined) {
      throw new ApiError(422, "missing_field", "Closing a goal needs origin: who said it was reached.");
    }
    const { applies_to: appliesTo, goal_status: goalStatus, valid_until: validUntil, origin, kind, topic, ...fields } = patch;
    Object.assign(memory, fields);
    if (kind !== undefined) memory.kind = kind;
    if (topic !== undefined) memory.topic = topic;
    if (appliesTo !== undefined) {
      memory.applies_to = { target: appliesTo.target, project_name: appliesTo.project_name ?? null };
      const name = appliesTo.project_name;
      if (name !== undefined && !account.projects.some((p) => p.name === name)) {
        account.projects.push({ id: `proj_${name}`, name, aliases: [], memories: 0 });
      }
    }
    if (goalStatus !== undefined) memory.goal_status = goalStatus;
    if (validUntil !== undefined) memory.valid_until = validUntil;
    if (origin !== undefined && (kind !== undefined || topic !== undefined || appliesTo !== undefined)) memory.origin = origin;
    if (kind !== undefined || topic !== undefined || appliesTo !== undefined) memory.needs_review = false;
    memory.updated_at = this.server.tick();
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
    // Grouped by topic, as the daemon's map (plan D13).
    const inScope = account.memories.filter((m) => category === null || m.topic === category);
    const aggregate = category === null && inScope.length > this.server.mapDetailLimit;
    const nodes = aggregate ? [] : inScope;
    const present = new Set(nodes.map((m) => m.id));
    const categories = new Map<string, { count: number; subs: Map<string, number> }>();
    for (const m of inScope) {
      const entry = categories.get(m.topic) ?? { count: 0, subs: new Map<string, number>() };
      entry.count += 1;
      if (m.subcategory !== null) entry.subs.set(m.subcategory, (entry.subs.get(m.subcategory) ?? 0) + 1);
      categories.set(m.topic, entry);
    }
    const label = (id: string) => topicsOf(account).find((t) => t.id === id)?.label ?? topicDefaultLabel(id);
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
        built_in: TOPICS.some((t) => t.id === id),
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
  return { ...memory, tags: [...memory.tags], applies_to: { ...memory.applies_to } };
}

/** Built-ins in order, then the user's own; counts of active memories. */
function topicsOf(account: Account): Topic[] {
  const counts = new Map<string, number>();
  for (const m of account.memories) counts.set(m.topic, (counts.get(m.topic) ?? 0) + 1);
  const entry = (id: string, label: string | null | undefined, builtIn: boolean): Topic => ({
    id,
    label: label ?? topicDefaultLabel(id),
    default_label: topicDefaultLabel(id),
    built_in: builtIn,
    count: counts.get(id) ?? 0,
  });
  return [
    ...TOPICS.map((t) => entry(t.id, account.topicLabels.get(t.id), true)),
    ...[...account.topics].sort(([a], [b]) => a.localeCompare(b)).map(([id, label]) => entry(id, label, false)),
  ];
}

function projectsOf(account: Account): Project[] {
  return account.projects
    .map((p) => ({ ...p, aliases: [...p.aliases], memories: account.memories.filter((m) => m.applies_to.project_name === p.name).length }))
    .sort((a, b) => a.name.localeCompare(b.name));
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
    kind: "preference",
    topic: "coding",
    applies_to: { target: "me", project_name: null },
    origin: "user_said",
    goal_status: null,
    needs_review: false,
    valid_until: null,
    ...overrides,
  };
}
