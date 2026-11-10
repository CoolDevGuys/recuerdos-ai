// Wire types for the daemon's /v1 API (docs/api.md). Kept to the fields the
// UI reads; the daemon may send more.

export type Scope = "read" | "write" | "admin";

/** `GET /v1/ping`: who the credential resolves to, and what it may do. */
export interface Identity {
  user: string;
  scopes: Scope[];
}

/** One memory, as `GET /v1/memories/export?format=json` returns it. */
export interface Memory {
  id: string;
  content: string;
  category: string;
  subcategory: string | null;
  tags: string[];
  confidence: number;
  created_at: string;
  updated_at: string;
  superseded_by: string | null;
}

/** The daemon's error body: `{"error": {"code", "message"}}`. */
export interface ApiErrorBody {
  error: { code: string; message: string };
}
