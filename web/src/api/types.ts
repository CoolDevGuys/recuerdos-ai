// Wire types for the daemon's /v1 API (docs/api.md). Kept to the fields the
// UI reads; the daemon may send more.

export type Scope = "read" | "write" | "admin";

/** `GET /v1/ping`: who the credential resolves to, and what it may do. */
export interface Identity {
  user: string;
  scopes: Scope[];
  /** A language model is configured: Suggest works. Absent means no. */
  understanding?: boolean;
}

/** `GET`/`PUT /v1/me`: who "the user" is in this store. */
export interface SelfNames {
  /** First the one they go by in full. */
  names: string[];
  /** How the profile says it; null with no names. */
  sentence: string | null;
  /** On save: memories newly sent to Needs review because they are about the user. */
  flagged?: number;
}

/** `GET /v1/about`: what the daemon says about itself. */
export interface About {
  version: string;
  git_sha: string;
  /** ISO 8601. */
  started_at: string;
  user: string;
  scopes: Scope[];
  auth_mode: "api-key" | "none";
  understanding: boolean;
  /** Only for an admin key (or with auth off): how the daemon was started. */
  daemon: DaemonDetails | null;
}

export interface DaemonDetails {
  /** The command line after the binary's name: `["serve", "--config", "…"]`. */
  command: string[];
  config_file: { path: string | null; from: "flag" | "env" | "none"; exists: boolean };
  /** `RECUERDOS_AI_*` variables set at start, by name (values are never sent). */
  overrides: string[];
  settings: { name: string; settings: { key: string; value: string }[] }[];
}

/** Who or what a memory is true for (ai-specs/memory-structure-plan.md §3). */
export type Target = "me" | "project" | "general";

export interface AppliesTo {
  target: Target;
  /** With `target: "project"`, the project's name. */
  project_name: string | null;
}

export type GoalStatus = "active" | "achieved" | "abandoned";

/** One memory, as the export, `GET`/`PATCH /v1/memories/{id}` and restore return it. */
export interface Memory {
  id: string;
  content: string;
  category: string;
  subcategory: string | null;
  tags: string[];
  confidence: number;
  created_at: string;
  /** Also the memory's version: sent back as `If-Match` to edit it. */
  updated_at: string;
  superseded_by: string | null;
  /** What sort of statement it is: preference, rule, convention… */
  kind: string;
  /** What it is about: a topic id. */
  topic: string;
  applies_to: AppliesTo;
  /** Where it came from: user_said, user_confirmed, inferred, document. */
  origin: string;
  /** For goals only. A closed goal (achieved, abandoned) leaves recall. */
  goal_status: GoalStatus | null;
  /** Filed by a guess (the migration from categories): the review queue lists it. */
  needs_review: boolean;
  /** When it stops being true; past it, it leaves recall. */
  valid_until: string | null;
}

/** `GET /v1/topics`: one topic a memory can be about. */
export interface Topic {
  id: string;
  /** The user's label, else the default wording. */
  label: string;
  default_label: string;
  built_in: boolean;
  /** Active memories about it. */
  count: number;
}

/** `GET /v1/projects`: one project memories can apply to. */
export interface Project {
  id: string;
  name: string;
  /** Remotes, folder names and former names that also find it. */
  aliases: string[];
  /** Active memories filed under it. */
  memories: number;
}

/** What renaming or merging a project did. */
export interface ProjectChange {
  project: Project;
  /** The daemon's sentence about it. */
  message: string;
  moved: number;
}

/** `POST /v1/memories:suggest`: what one memory may really be. */
export interface Suggestion {
  id: string;
  kind: string;
  topic: string;
  applies_to: AppliesTo;
  reason: string;
}

/** `GET /v1/categories`: one category a memory can be filed under. */
export interface Category {
  id: string;
  /** What to show: the user's own label, else the default wording. */
  label: string;
  /** The default wording, for "reset to default". */
  default_label: string;
  built_in: boolean;
  /**
   * False for a category still on a stored memory but missing from the
   * user's catalog: listed so its memories can be moved, not offered for new ones.
   */
  known: boolean;
  count: number;
}

/** `PATCH /v1/categories/{id}`: a new label (`null` resets it) and/or, for a custom category, a new id. */
export interface CategoryUpdate {
  label?: string | null;
  id?: string;
}

/** What a category change did: the category as it now is, and how many memories moved. */
export interface CategoryChange {
  category: Category | null;
  moved: number;
  reembedded: number;
}

/** `POST /v1/memories:bulk`: what to change on every selected memory. */
export interface BulkEdit {
  category?: string;
  /** `null` clears the sub-label. */
  subcategory?: string | null;
  addTags?: string[];
  removeTags?: string[];
  kind?: string;
  topic?: string;
  /** A known project only, for `target: "project"`. */
  appliesTo?: AppliesTo;
  /** Confirms each memory's structure as it is: it leaves the review queue. */
  reviewed?: boolean;
}

export interface BulkResult {
  memories: Memory[];
  changed: number;
  reembedded: number;
}

/** What a memory is made of, for a save or an edit. */
export interface MemoryFields {
  content: string;
  category: string;
  subcategory: string | null;
  tags: string[];
}

/**
 * What a new memory is, what it is about and who it is true for
 * (ai-specs/memory-structure-plan.md §3). The daemon applies the rules.
 */
export interface MemoryStructure {
  kind: string;
  topic: string;
  applies_to: { target: "me" | "project" | "general"; project_name?: string; new_project?: boolean };
  origin: "user_said";
  /** A stored memory this one replaces: the user changed their mind. */
  replaces?: string;
  /** Stored memories the user said this does not replace: both hold. */
  coexists_with?: string[];
  /** When it stops being true (RFC 3339). Required for a plan. */
  valid_until?: string;
}

/** What a save did: stored it, or found a stored memory already says it. */
export interface SavedMemory {
  memory: Memory;
  status: "stored" | "already_known";
  /** The daemon's sentence about how it was filed. */
  message: string;
}

/** A stored memory a save contradicts (a 422 `conflict`). */
export interface ConflictingMemory {
  id: string;
  content: string;
}

/** An edit of what a memory is (PATCH): only what changed. */
export interface StructurePatch {
  kind?: string;
  topic?: string;
  applies_to?: { target: Target; project_name?: string; new_project?: boolean };
  goal_status?: GoalStatus;
  /** `null` removes the end date. */
  valid_until?: string | null;
  /** Who asked for the change: the user, here. Required to close a goal. */
  origin?: "user_said";
}

/** An edit: only the fields that changed. */
export type MemoryPatch = Partial<MemoryFields> & StructurePatch;

/** What the understanding pipeline did with a submission (`POST /v1/memories`, `wait: true`). */
export type IngestOutcome = "stored" | "nothing_durable" | "already_known" | "changed_without_storing";

export interface IngestResult {
  outcome: IngestOutcome;
  memoryIds: string[];
  /** False when the daemon has no model configured and stored the text as sent. */
  understanding: boolean;
}

/** A submission to the understanding pipeline: the text, plus optional hints. */
export interface IngestRequest {
  content: string;
  category: string | null;
  tags: string[];
}

/** The daemon's error body: `{"error": {"code", "message"}}`. */
export interface ApiErrorBody {
  error: { code: string; message: string };
}

/** `GET /v1/map` (web-ui-plan.md §4.1). */
export interface MapData {
  generation: string;
  /** The least weight a similarity edge here carries. */
  minWeight: number;
  /** The store is too large for similarity edges. */
  edgesOmitted: boolean;
  /**
   * `memories` when `nodes` holds every memory in scope; `categories` when the
   * store is above the daemon's detail limit and only the category summary
   * came back — ask for one category to see its memories.
   */
  detail: "memories" | "categories";
  /** Memories in scope, whether or not `nodes` carries them. */
  total: number;
  /**
   * The topic this map is limited to, or `null` for the whole store. The map
   * groups by topic (plan D13); the wire calls the groups `topics`, and the
   * map code calls each group a "category" internally.
   */
  category: string | null;
  nodes: Memory[];
  edges: MapEdge[];
  categories: MapCategory[];
  categoryEdges: { source: string; target: string; weight: number }[];
}

export type MapEdge =
  | { kind: "similar"; source: string; target: string; weight: number }
  | { kind: "relation"; source: string; target: string; predicate: string; entity: string };

export interface MapCategory {
  id: string;
  label: string;
  built_in: boolean;
  count: number;
  subcategories: { name: string; count: number }[];
}

/** One recall result (`POST /v1/memories/search`), for the map's recall preview. */
export interface RecallHit {
  memory: Memory;
  /** How well it matches, 0–1. */
  relevance: number;
  /** Which legs found it, 1-based ranks. */
  matched: { vector: number | null; keyword: number | null; graph: number | null; rerank: number | null };
}
