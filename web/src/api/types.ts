// Wire types for the daemon's /v1 API (docs/api.md). Kept to the fields the
// UI reads; the daemon may send more.

export type Scope = "read" | "write" | "admin";

/** `GET /v1/ping`: who the credential resolves to, and what it may do. */
export interface Identity {
  user: string;
  scopes: Scope[];
}

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

/** An edit: only the fields that changed. */
export type MemoryPatch = Partial<MemoryFields>;

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
  /** The category this map is limited to, or `null` for the whole store. */
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
