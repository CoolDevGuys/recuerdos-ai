# REST API

**Status: Phase 2.** Authentication and the memory endpoints are live.
The async understanding pipeline (`POST /v1/memories`) arrives in Phase 4.
See [implementation-plan.md](../implementation-plan.md) for what lands when.

## Authentication

Every route under `/v1` requires an API key as a bearer token:

```bash
curl -H "Authorization: Bearer ra_live_b99f884a…" localhost:7070/v1/ping
```

Keys are issued from the CLI (`recordagent key issue`) — there is no
endpoint that mints them, by design. See
[configuration.md](configuration.md#authentication) for the
`[auth].mode = "none"` single-user escape hatch.

The scheme name is case-insensitive (`Bearer`, `bearer`). A missing,
malformed, unknown, wrong-secret or revoked key all produce the identical
401 — which of those it was is not information the API hands out.

### Scopes

| Scope | Grants |
|---|---|
| `read` | Reading memories (Phase 2) |
| `write` | Creating and modifying memories (Phase 2) |
| `admin` | Everything |

`write` does **not** imply `read`: a write-only ingestion key — an agent
that saves memories but can never read them back — is a useful thing to be
able to hand out. A key missing the scope a route needs gets `403`, not
`401`: the credential was valid, the permission wasn't.

## Error envelope

Every error, from every route, has this shape:

```json
{"error": {"code": "unauthorized", "message": "invalid API key"}}
```

Branch on `code`; `message` is for the human reading the log.

| `code` | HTTP | Meaning |
|---|---|---|
| `validation_failed` | 400 | The request was malformed |
| `unauthorized` | 401 | No usable credential (`WWW-Authenticate: Bearer`) |
| `forbidden` | 403 | Valid key, missing scope |
| `not_found` | 404 | No such resource |
| `conflict` | 409 | Violates a uniqueness rule |
| `internal` | 500 | Server fault |

`internal` responses always carry the literal message `"internal error"`.
Details (paths, SQL, driver text) go to the server log with the request id
— they are not for the client.

## Routes

### `GET /healthz`

Unauthenticated by design: a health check that needs a credential is
useless to a load balancer or Docker's `HEALTHCHECK`.

```json
{"status": "ok"}
```

### `GET /version`

Unauthenticated.

```json
{"version": "0.1.0", "git_sha": "a1b2c3d4e5f6"}
```

### `GET /v1/ping`

**Temporary.** Requires any valid key. Exists to prove authentication
end-to-end until Phase 2 adds real routes, then it is removed.

```bash
curl -H "Authorization: Bearer $KEY" localhost:7070/v1/ping
```

```json
{"user": "alex", "scopes": ["read", "write"]}
```

## Request ids

Every response carries `x-request-id` (generated if you don't send one).
Include it when reporting a problem: it ties the response to the server
log line that has the real error.

## Memories

### `POST /v1/memories:direct`

Stores a memory exactly as given. Requires `write`.

```bash
curl -X POST localhost:7070/v1/memories:direct \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' -d '{
    "content": "User forbids barrel files / index.ts re-exports",
    "category": "preference.coding",
    "tags": ["typescript", "imports"]
  }'
```

| Field | Required | Notes |
|---|---|---|
| `content` | yes | 1–4000 characters |
| `category` | no | Defaults to `fact.project`. See the taxonomy below |
| `tags` | no | Lowercased and de-duplicated; max 32 |
| `confidence` | no | `0.0`–`1.0`, default `1.0`; clamped, not rejected |
| `client`, `session_id` | no | Recorded as the memory's source |
| `expires_at` | no | RFC 3339; must be in the future |

Responds `201` with the stored memory.

### `POST /v1/memories/search`

Hybrid recall: semantic (vector) and keyword (BM25) legs, fused by
reciprocal rank. Requires `read`.

```bash
curl -X POST localhost:7070/v1/memories/search \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"query": "how should I structure my typescript imports?", "limit": 3}'
```

```json
{
  "results": [
    {
      "id": "019f7c5a-...",
      "content": "User forbids barrel files / index.ts re-exports",
      "category": "preference.coding",
      "tags": ["typescript", "imports"],
      "confidence": 1.0,
      "created_at": "2026-07-19T08:18:19Z",
      "updated_at": "2026-07-19T08:18:19Z",
      "score": 0.0325,
      "matched": {"vector_rank": 1, "bm25_rank": 2}
    }
  ],
  "took_ms": 9
}
```

`matched` says which leg found the result, so a surprising ranking can be
explained rather than merely distrusted. An absent rank means that leg
didn't return it.

| Field | Notes |
|---|---|
| `query` | Required, 1–1000 characters |
| `limit` | Default from `[retrieval].default_limit`; capped at 50 |
| `categories` | OR-ed. Empty means all |
| `tags` | **AND-ed** — a memory must carry every one |
| `since` | RFC 3339; excludes memories created before it |
| `include_superseded` | Default `false` |

Filters are applied after the indexes answer, over a candidate window
several times `limit`. A very selective filter over a large corpus can
therefore return fewer than `limit` results even when more exist.

### `GET /v1/memories/{id}`

Requires `read`. `404` both when the memory doesn't exist and when it
belongs to another user — the two are deliberately indistinguishable.

### `PATCH /v1/memories/{id}`

Requires `write`. Any of `content`, `category`, `tags`, `expires_at`.
Omitting a field leaves it alone; sending `"expires_at": null` clears it.
Editing content re-embeds and re-indexes.

### `DELETE /v1/memories/{id}`

Requires `write`. Responds `204`. A soft delete: the memory stops being
recalled, and the audit trail keeps what happened to it.

### `GET /v1/memories/export`

Requires `read`. `?format=markdown` (default) or `?format=json`, plus
`?include_inactive=true` to include superseded memories.

Markdown is grouped by category, one memory per line, so it stays
greppable and diffable — a git-versioned backup is a `>` away.

### `GET /v1/audit`

Requires `read`. `?limit=` (default 100, max 500). Every mutation this
user has made, newest first, including deletions.

## Categories

A closed taxonomy, extensible via `[understanding.taxonomy]`:

`preference.coding` · `preference.personal` · `decision` ·
`fact.project` · `fact.person` · `experience` · `skill` · `reference`

Unknown category names are rejected rather than silently created — a typo
would otherwise produce a category nothing ever matches.

## Coming in Phase 4

`POST /v1/memories` (async understanding pipeline, `202 {job_id}`),
`GET /v1/jobs/{id}`.

See [project-plan.md §9](../project-plan.md#9-api-design-rest--mcp) for the
full design.
