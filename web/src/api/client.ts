import type {
  ApiErrorBody,
  BulkEdit,
  BulkResult,
  Category,
  CategoryChange,
  CategoryUpdate,
  ConflictingMemory,
  Identity,
  IngestOutcome,
  IngestRequest,
  IngestResult,
  MapData,
  MapEdge,
  Memory,
  MemoryFields,
  MemoryPatch,
  MemoryStructure,
  Project,
  ProjectChange,
  RecallHit,
  SavedMemory,
  Scope,
  Suggestion,
  Target,
  Topic,
  AppliesTo,
  GoalStatus,
} from "./types";

/**
 * Everything the UI asks of the daemon. Components never call `fetch`:
 * they go through an `ApiClient`, which tests replace with
 * `InMemoryApiClient` (./fake.ts).
 */
export interface ApiClient {
  ping(): Promise<Identity>;
  listMemories(): Promise<Memory[]>;
  getMemory(id: string): Promise<Memory>;
  listCategories(): Promise<Category[]>;
  /** Saves exactly these fields and this structure, no pipeline. */
  /** Saves as written; a contradiction is a 422 `conflict` `ApiError`. */
  createMemory(fields: MemoryFields, structure: MemoryStructure): Promise<SavedMemory>;
  /** Runs the text through the understanding pipeline and waits for it. */
  ingest(request: IngestRequest): Promise<IngestResult>;
  /**
   * Applies `patch`. With `version` (the `updated_at` the editor started
   * from) the edit is refused with a 412 `ApiError` if the memory changed
   * since; `null` overwrites unconditionally.
   */
  updateMemory(id: string, patch: MemoryPatch, version: string | null): Promise<Memory>;
  forgetMemory(id: string): Promise<void>;
  restoreMemory(id: string): Promise<Memory>;
  /** The whole store's map, or one category's with `category`. */
  getMap(category?: string | null): Promise<MapData>;
  /** What recall returns for `query`, as an agent would see it. */
  recall(query: string, limit: number): Promise<RecallHit[]>;
  /** Creates a custom category. */
  createCategory(id: string, label: string | null): Promise<Category>;
  /** Relabels any category and/or renames a custom one (re-filing its memories). */
  updateCategory(id: string, update: CategoryUpdate): Promise<CategoryChange>;
  /** Moves every memory of custom category `id` into `into`, then deletes `id`. */
  mergeCategory(id: string, into: string): Promise<CategoryChange>;
  /** Deletes a custom category; one with memories needs `moveTo`. */
  deleteCategory(id: string, moveTo: string | null): Promise<CategoryChange>;
  /** Applies one edit to many memories, all or nothing. */
  bulkEdit(ids: string[], edit: BulkEdit): Promise<BulkResult>;
  /** Every topic, with how many memories each holds. */
  listTopics(): Promise<Topic[]>;
  createTopic(id: string, label: string | null): Promise<Topic>;
  /** `null` goes back to the default wording. */
  relabelTopic(id: string, label: string | null): Promise<Topic>;
  listProjects(): Promise<Project[]>;
  renameProject(name: string, newName: string): Promise<ProjectChange>;
  /** Folds project `name` into `into`. */
  mergeProject(name: string, into: string): Promise<ProjectChange>;
  /** What each memory may really be; needs a model (`Identity.understanding`). */
  suggest(ids: string[]): Promise<Suggestion[]>;
}

/** A failed request, carrying the daemon's own error code and message. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  /** For a `conflict`: the stored memories the save contradicts. */
  readonly conflicts: ConflictingMemory[];

  constructor(status: number, code: string, message: string, conflicts: ConflictingMemory[] = []) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.conflicts = conflicts;
  }
}

/** The daemon could not be reached at all (down, network). */
export class NetworkError extends Error {
  constructor(cause: unknown) {
    super("The memory server could not be reached.", { cause });
    this.name = "NetworkError";
  }
}

const SCOPES: readonly Scope[] = ["read", "write", "admin"];
const OUTCOMES: readonly IngestOutcome[] = ["stored", "nothing_durable", "already_known", "changed_without_storing"];
/** Recorded as the source of memories this UI writes. */
const CLIENT_NAME = "web-ui";

interface RequestOptions {
  method?: "GET" | "POST" | "PATCH" | "DELETE";
  body?: unknown;
  headers?: Record<string, string>;
}

/**
 * The real client: same-origin requests to `/v1`, with the key as a bearer
 * token. `key` is `null` on a daemon in `auth.mode = "none"`, which needs
 * no credential.
 */
export class HttpApiClient implements ApiClient {
  private readonly key: string | null;
  private readonly fetcher: typeof fetch;

  constructor(key: string | null, fetcher: typeof fetch = fetch.bind(globalThis)) {
    this.key = key;
    this.fetcher = fetcher;
  }

  async ping(): Promise<Identity> {
    return parseIdentity(await this.request("/v1/ping"));
  }

  async listMemories(): Promise<Memory[]> {
    return parseMemories(await this.request("/v1/memories/export?format=json"));
  }

  async getMemory(id: string): Promise<Memory> {
    return parseMemory(await this.request(`/v1/memories/${encodeURIComponent(id)}`));
  }

  async listCategories(): Promise<Category[]> {
    return parseCategories(await this.request("/v1/categories"));
  }

  async createMemory(fields: MemoryFields, structure: MemoryStructure): Promise<SavedMemory> {
    // The structured save takes no category (it is derived from kind and
    // topic) and calls the sub-label a subtopic.
    const body = await this.request("/v1/memories:direct", {
      method: "POST",
      body: {
        content: fields.content,
        ...(fields.subcategory !== null && { subtopic: fields.subcategory }),
        tags: fields.tags,
        ...structure,
        client: CLIENT_NAME,
      },
    });
    const reply = isObject(body) ? body : {};
    return {
      memory: parseMemory(body),
      status: reply.status === "already_known" ? "already_known" : "stored",
      message: typeof reply.message === "string" ? reply.message : "",
    };
  }

  async ingest(request: IngestRequest): Promise<IngestResult> {
    const body = await this.request("/v1/memories", {
      method: "POST",
      body: {
        content: request.content,
        ...(request.category !== null && { category: request.category }),
        tags: request.tags,
        client: CLIENT_NAME,
        wait: true,
      },
    });
    return parseIngestResult(body);
  }

  async updateMemory(id: string, patch: MemoryPatch, version: string | null): Promise<Memory> {
    const body = await this.request(`/v1/memories/${encodeURIComponent(id)}`, {
      method: "PATCH",
      body: patch,
      ...(version !== null && { headers: { "If-Match": `"${version}"` } }),
    });
    return parseMemory(body);
  }

  async forgetMemory(id: string): Promise<void> {
    await this.request(`/v1/memories/${encodeURIComponent(id)}`, { method: "DELETE" });
  }

  async restoreMemory(id: string): Promise<Memory> {
    return parseMemory(await this.request(`/v1/memories/${encodeURIComponent(id)}/restore`, { method: "POST" }));
  }

  async getMap(category: string | null = null): Promise<MapData> {
    const query = category === null ? "" : `?topic=${encodeURIComponent(category)}`;
    return parseMap(await this.request(`/v1/map${query}`));
  }

  async recall(query: string, limit: number): Promise<RecallHit[]> {
    // `scope: "all"`: a person searching their own store wants every project's
    // memories ranked alike, not demoted the way an agent working in one is.
    const body = await this.request("/v1/memories/search", {
      method: "POST",
      body: { query, limit, scope: "all" },
    });
    if (!isObject(body) || !Array.isArray(body.results)) throw malformed("recall result");
    return body.results.map(parseRecallHit);
  }

  async createCategory(id: string, label: string | null): Promise<Category> {
    const body = await this.request("/v1/categories", { method: "POST", body: { id, ...(label !== null && { label }) } });
    return parseCategory(body);
  }

  async updateCategory(id: string, update: CategoryUpdate): Promise<CategoryChange> {
    const body = await this.request(`/v1/categories/${encodeURIComponent(id)}`, { method: "PATCH", body: update });
    return parseCategoryChange(body);
  }

  async mergeCategory(id: string, into: string): Promise<CategoryChange> {
    const body = await this.request(`/v1/categories/${encodeURIComponent(id)}/merge`, { method: "POST", body: { into } });
    return parseCategoryChange(body);
  }

  async deleteCategory(id: string, moveTo: string | null): Promise<CategoryChange> {
    const query = moveTo === null ? "" : `?move_to=${encodeURIComponent(moveTo)}`;
    const body = await this.request(`/v1/categories/${encodeURIComponent(id)}${query}`, { method: "DELETE" });
    return parseCategoryChange(body);
  }

  async listTopics(): Promise<Topic[]> {
    const body = await this.request("/v1/topics");
    if (!isObject(body) || !Array.isArray(body.topics)) throw malformed("topic list");
    return body.topics.map(parseTopic);
  }

  async createTopic(id: string, label: string | null): Promise<Topic> {
    return parseTopic(await this.request("/v1/topics", { method: "POST", body: { id, ...(label !== null && { label }) } }));
  }

  async relabelTopic(id: string, label: string | null): Promise<Topic> {
    return parseTopic(await this.request(`/v1/topics/${encodeURIComponent(id)}`, { method: "PATCH", body: { label } }));
  }

  async listProjects(): Promise<Project[]> {
    const body = await this.request("/v1/projects");
    if (!isObject(body) || !Array.isArray(body.projects)) throw malformed("project list");
    return body.projects.map(parseProject);
  }

  async renameProject(name: string, newName: string): Promise<ProjectChange> {
    return parseProjectChange(
      await this.request(`/v1/projects/${encodeURIComponent(name)}`, { method: "PATCH", body: { name: newName } }),
    );
  }

  async mergeProject(name: string, into: string): Promise<ProjectChange> {
    return parseProjectChange(
      await this.request(`/v1/projects/${encodeURIComponent(name)}/merge`, { method: "POST", body: { into } }),
    );
  }

  async suggest(ids: string[]): Promise<Suggestion[]> {
    const body = await this.request("/v1/memories:suggest", { method: "POST", body: { ids } });
    if (!isObject(body) || !Array.isArray(body.suggestions)) throw malformed("suggestions");
    return body.suggestions.flatMap((item: unknown) =>
      isObject(item) && typeof item.id === "string" && typeof item.kind === "string" && typeof item.topic === "string"
        ? [
            {
              id: item.id,
              kind: item.kind,
              topic: item.topic,
              applies_to: parseAppliesTo(item.applies_to),
              reason: typeof item.reason === "string" ? item.reason : "",
            },
          ]
        : [],
    );
  }

  async bulkEdit(ids: string[], edit: BulkEdit): Promise<BulkResult> {
    const set: Record<string, unknown> = {};
    if (edit.category !== undefined) set.category = edit.category;
    if (edit.subcategory !== undefined) set.subcategory = edit.subcategory;
    if (edit.kind !== undefined) set.kind = edit.kind;
    if (edit.topic !== undefined) set.topic = edit.topic;
    if (edit.appliesTo !== undefined) {
      set.applies_to = {
        target: edit.appliesTo.target,
        ...(edit.appliesTo.project_name !== null && { project_name: edit.appliesTo.project_name }),
      };
    }
    if (edit.reviewed === true) set.reviewed = true;
    const body = await this.request("/v1/memories:bulk", {
      method: "POST",
      body: { ids, set, add_tags: edit.addTags ?? [], remove_tags: edit.removeTags ?? [] },
    });
    if (!isObject(body) || !Array.isArray(body.memories)) throw malformed("bulk edit result");
    return {
      memories: body.memories.map(parseMemory),
      changed: typeof body.changed === "number" ? body.changed : 0,
      reembedded: typeof body.reembedded === "number" ? body.reembedded : 0,
    };
  }

  private async request(path: string, options: RequestOptions = {}): Promise<unknown> {
    const headers: Record<string, string> = {
      Accept: "application/json",
      "X-Recuerdos-Client": CLIENT_NAME,
      ...options.headers,
    };
    if (this.key !== null) headers.Authorization = `Bearer ${this.key}`;
    if (options.body !== undefined) headers["Content-Type"] = "application/json";

    let response: Response;
    try {
      response = await this.fetcher(path, {
        method: options.method ?? "GET",
        headers,
        credentials: "omit",
        ...(options.body !== undefined && { body: JSON.stringify(options.body) }),
      });
    } catch (cause) {
      throw new NetworkError(cause);
    }

    if (response.status === 204) return null;
    const body: unknown = await response.json().catch(() => null);
    if (!response.ok) throw toApiError(response.status, body);
    return body;
  }
}

function toApiError(status: number, body: unknown): ApiError {
  if (isErrorBody(body)) return new ApiError(status, body.error.code, body.error.message, conflictsIn(body.error));
  return new ApiError(status, "unknown", `The server answered ${status}.`);
}

/** The stored memories a `conflict` refusal names, from its `details`. */
function conflictsIn(error: unknown): ConflictingMemory[] {
  if (!isObject(error) || !isObject(error.details) || !Array.isArray(error.details.conflicts)) return [];
  return error.details.conflicts.flatMap((item: unknown) =>
    isObject(item) && typeof item.id === "string" && typeof item.content === "string"
      ? [{ id: item.id, content: item.content }]
      : [],
  );
}

function isErrorBody(body: unknown): body is ApiErrorBody {
  if (!isObject(body) || !("error" in body)) return false;
  const error = body.error;
  return isObject(error) && typeof error.code === "string" && typeof error.message === "string";
}

// Parsed at the boundary: past this file, every value has the shape its
// type claims.

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function malformed(what: string): ApiError {
  return new ApiError(0, "malformed_response", `The server sent an unexpected ${what}.`);
}

export function parseIdentity(body: unknown): Identity {
  if (!isObject(body) || typeof body.user !== "string" || !Array.isArray(body.scopes)) {
    throw malformed("identity");
  }
  return {
    user: body.user,
    scopes: body.scopes.filter((s): s is Scope => SCOPES.includes(s as Scope)),
    understanding: body.understanding === true,
  };
}

const TARGETS: readonly Target[] = ["me", "project", "general"];
const GOAL_STATUSES: readonly GoalStatus[] = ["active", "achieved", "abandoned"];

export function parseAppliesTo(value: unknown): AppliesTo {
  if (!isObject(value)) return { target: "general", project_name: null };
  const target = TARGETS.includes(value.target as Target) ? (value.target as Target) : "general";
  return {
    target,
    project_name: target === "project" && typeof value.project_name === "string" ? value.project_name : null,
  };
}

export function parseTopic(item: unknown): Topic {
  if (!isObject(item) || typeof item.id !== "string" || typeof item.label !== "string") throw malformed("topic");
  return {
    id: item.id,
    label: item.label,
    default_label: typeof item.default_label === "string" ? item.default_label : item.label,
    built_in: item.built_in === true,
    count: typeof item.count === "number" ? item.count : 0,
  };
}

export function parseProject(item: unknown): Project {
  if (!isObject(item) || typeof item.id !== "string" || typeof item.name !== "string") throw malformed("project");
  return {
    id: item.id,
    name: item.name,
    aliases: Array.isArray(item.aliases) ? item.aliases.filter((a): a is string => typeof a === "string") : [],
    memories: typeof item.memories === "number" ? item.memories : 0,
  };
}

function parseProjectChange(body: unknown): ProjectChange {
  if (!isObject(body)) throw malformed("project change");
  return {
    project: parseProject(body.project),
    message: typeof body.message === "string" ? body.message : "",
    moved: typeof body.moved === "number" ? body.moved : 0,
  };
}

export function parseMemories(body: unknown): Memory[] {
  if (!isObject(body) || !Array.isArray(body.memories)) throw malformed("memory list");
  return body.memories.map(parseMemory);
}

export function parseMemory(item: unknown): Memory {
  if (
    !isObject(item) ||
    typeof item.id !== "string" ||
    typeof item.content !== "string" ||
    typeof item.category !== "string" ||
    typeof item.created_at !== "string" ||
    typeof item.updated_at !== "string"
  ) {
    throw malformed("memory");
  }
  return {
    id: item.id,
    content: item.content,
    category: item.category,
    subcategory: typeof item.subcategory === "string" ? item.subcategory : null,
    tags: Array.isArray(item.tags) ? item.tags.filter((t): t is string => typeof t === "string") : [],
    confidence: typeof item.confidence === "number" ? item.confidence : 1,
    created_at: item.created_at,
    updated_at: item.updated_at,
    superseded_by: typeof item.superseded_by === "string" ? item.superseded_by : null,
    kind: typeof item.kind === "string" ? item.kind : "fact",
    topic: typeof item.topic === "string" ? item.topic : "work",
    applies_to: parseAppliesTo(item.applies_to),
    origin: typeof item.origin === "string" ? item.origin : "user_said",
    goal_status: GOAL_STATUSES.includes(item.goal_status as GoalStatus) ? (item.goal_status as GoalStatus) : null,
    needs_review: item.needs_review === true,
    valid_until:
      typeof item.valid_until === "string"
        ? item.valid_until
        : typeof item.expires_at === "string"
          ? item.expires_at
          : null,
  };
}

export function parseCategories(body: unknown): Category[] {
  if (!isObject(body) || !Array.isArray(body.categories)) throw malformed("category list");
  return body.categories.map(parseCategory);
}

export function parseCategory(item: unknown): Category {
  if (!isObject(item) || typeof item.id !== "string" || typeof item.label !== "string") {
    throw malformed("category");
  }
  return {
    id: item.id,
    label: item.label,
    default_label: typeof item.default_label === "string" ? item.default_label : item.label,
    built_in: item.built_in === true,
    known: item.known !== false,
    count: typeof item.count === "number" ? item.count : 0,
  };
}

export function parseCategoryChange(body: unknown): CategoryChange {
  if (!isObject(body)) throw malformed("category change");
  return {
    category: body.category === null || body.category === undefined ? null : parseCategory(body.category),
    moved: typeof body.moved === "number" ? body.moved : 0,
    reembedded: typeof body.reembedded === "number" ? body.reembedded : 0,
  };
}

export function parseIngestResult(body: unknown): IngestResult {
  if (!isObject(body) || !OUTCOMES.includes(body.outcome as IngestOutcome) || !Array.isArray(body.memory_ids)) {
    throw malformed("ingest result");
  }
  return {
    outcome: body.outcome as IngestOutcome,
    memoryIds: body.memory_ids.filter((id): id is string => typeof id === "string"),
    understanding: body.understanding !== false,
  };
}

function rank(value: unknown): number | null {
  return typeof value === "number" ? value : null;
}

export function parseRecallHit(item: unknown): RecallHit {
  if (!isObject(item) || typeof item.relevance !== "number") throw malformed("recall result");
  const matched = isObject(item.matched) ? item.matched : {};
  return {
    memory: parseMemory(item),
    relevance: item.relevance,
    matched: {
      vector: rank(matched.vector_rank),
      keyword: rank(matched.bm25_rank),
      graph: rank(matched.graph_rank),
      rerank: rank(matched.rerank_rank),
    },
  };
}

function parseMapEdge(item: unknown): MapEdge | null {
  if (!isObject(item) || typeof item.source !== "string" || typeof item.target !== "string") return null;
  if (item.kind === "similar" && typeof item.weight === "number") {
    return { kind: "similar", source: item.source, target: item.target, weight: item.weight };
  }
  if (item.kind === "relation") {
    return {
      kind: "relation",
      source: item.source,
      target: item.target,
      predicate: typeof item.predicate === "string" ? item.predicate : "",
      entity: typeof item.entity === "string" ? item.entity : "",
    };
  }
  return null; // an edge kind this UI does not know yet
}

/**
 * The map groups by topic (plan D13): the wire's `topics`, `topic_edges` and
 * `topic` become the map code's groups, which it calls categories.
 */
export function parseMap(body: unknown): MapData {
  if (!isObject(body) || !Array.isArray(body.nodes) || !Array.isArray(body.edges) || !Array.isArray(body.topics)) {
    throw malformed("map");
  }
  return {
    generation: typeof body.generation === "string" ? body.generation : "",
    minWeight: typeof body.min_weight === "number" ? body.min_weight : 0,
    edgesOmitted: body.edges_omitted === true,
    detail: body.detail === "topics" ? "categories" : "memories",
    total: typeof body.total === "number" ? body.total : body.nodes.length,
    category: typeof body.topic === "string" ? body.topic : null,
    nodes: body.nodes.map(parseMemory),
    edges: body.edges.map(parseMapEdge).filter((e): e is MapEdge => e !== null),
    categories: body.topics.map((c) => {
      if (!isObject(c) || typeof c.id !== "string" || typeof c.label !== "string") throw malformed("map topic");
      return {
        id: c.id,
        label: c.label,
        built_in: c.built_in === true,
        count: typeof c.count === "number" ? c.count : 0,
        subcategories: Array.isArray(c.subtopics)
          ? c.subtopics.flatMap((s) =>
              isObject(s) && typeof s.name === "string" ? [{ name: s.name, count: typeof s.count === "number" ? s.count : 0 }] : [],
            )
          : [],
      };
    }),
    categoryEdges: Array.isArray(body.topic_edges)
      ? body.topic_edges.flatMap((e) =>
          isObject(e) && typeof e.source === "string" && typeof e.target === "string" && typeof e.weight === "number"
            ? [{ source: e.source, target: e.target, weight: e.weight }]
            : [],
        )
      : [],
  };
}
