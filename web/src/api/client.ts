import type { ApiErrorBody, Identity, Memory, Scope } from "./types";

/**
 * Everything the UI asks of the daemon. Components never call `fetch`:
 * they go through an `ApiClient`, which tests replace with
 * `InMemoryApiClient` (./fake.ts).
 */
export interface ApiClient {
  ping(): Promise<Identity>;
  listMemories(): Promise<Memory[]>;
}

/** A failed request, carrying the daemon's own error code and message. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

/** The daemon could not be reached at all (down, network, CORS). */
export class NetworkError extends Error {
  constructor(cause: unknown) {
    super("The memory server could not be reached.", { cause });
    this.name = "NetworkError";
  }
}

const SCOPES: readonly Scope[] = ["read", "write", "admin"];

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
    const body = await this.request("/v1/ping");
    return parseIdentity(body);
  }

  async listMemories(): Promise<Memory[]> {
    const body = await this.request("/v1/memories/export?format=json");
    return parseMemories(body);
  }

  private async request(path: string): Promise<unknown> {
    const headers: Record<string, string> = {
      Accept: "application/json",
      "X-Recuerdos-Client": "web-ui",
    };
    if (this.key !== null) headers.Authorization = `Bearer ${this.key}`;

    let response: Response;
    try {
      response = await this.fetcher(path, { headers, credentials: "omit" });
    } catch (cause) {
      throw new NetworkError(cause);
    }

    const body: unknown = await response.json().catch(() => null);
    if (!response.ok) throw toApiError(response.status, body);
    return body;
  }
}

function toApiError(status: number, body: unknown): ApiError {
  if (isErrorBody(body)) return new ApiError(status, body.error.code, body.error.message);
  return new ApiError(status, "unknown", `The server answered ${status}.`);
}

function isErrorBody(body: unknown): body is ApiErrorBody {
  if (typeof body !== "object" || body === null || !("error" in body)) return false;
  const error = (body as { error: unknown }).error;
  return (
    typeof error === "object" &&
    error !== null &&
    typeof (error as { code?: unknown }).code === "string" &&
    typeof (error as { message?: unknown }).message === "string"
  );
}

// Parsed at the boundary: past this file, every value has the shape its
// type claims.

function malformed(what: string): ApiError {
  return new ApiError(0, "malformed_response", `The server sent an unexpected ${what}.`);
}

export function parseIdentity(body: unknown): Identity {
  if (typeof body !== "object" || body === null) throw malformed("identity");
  const { user, scopes } = body as { user?: unknown; scopes?: unknown };
  if (typeof user !== "string" || !Array.isArray(scopes)) throw malformed("identity");
  return {
    user,
    scopes: scopes.filter((s): s is Scope => SCOPES.includes(s as Scope)),
  };
}

export function parseMemories(body: unknown): Memory[] {
  if (typeof body !== "object" || body === null) throw malformed("memory list");
  const items = (body as { memories?: unknown }).memories;
  if (!Array.isArray(items)) throw malformed("memory list");
  return items.map(parseMemory);
}

function parseMemory(item: unknown): Memory {
  if (typeof item !== "object" || item === null) throw malformed("memory");
  const m = item as Record<string, unknown>;
  if (
    typeof m.id !== "string" ||
    typeof m.content !== "string" ||
    typeof m.category !== "string" ||
    typeof m.created_at !== "string" ||
    typeof m.updated_at !== "string"
  ) {
    throw malformed("memory");
  }
  return {
    id: m.id,
    content: m.content,
    category: m.category,
    subcategory: typeof m.subcategory === "string" ? m.subcategory : null,
    tags: Array.isArray(m.tags) ? m.tags.filter((t): t is string => typeof t === "string") : [],
    confidence: typeof m.confidence === "number" ? m.confidence : 1,
    created_at: m.created_at,
    updated_at: m.updated_at,
    superseded_by: typeof m.superseded_by === "string" ? m.superseded_by : null,
  };
}
