# REST API

**Status: Phase 4.** Authentication, the memory endpoints, the profile and
the async understanding pipeline are live; agents can also reach all of it
over [MCP](mcp.md).
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

There are two ways in, and the difference is who decides what to remember.

| | You decide | The service decides |
|---|---|---|
| Endpoint | `POST /v1/memories:direct` | `POST /v1/memories` |
| Input | one memory, already written | raw text of any length |
| Work | one insert | extract → reconcile, via a language model |
| Returns | `201` with the memory | `202` with a job id |

Use `:direct` when you have already distilled something — an agent that
just decided "this is worth remembering" knows what the memory should
say. Use `/v1/memories` for raw material: a sentence the user typed, a
session summary, a paragraph that might contain three durable facts or
none.

### `POST /v1/memories` — ingest raw content

Submits content for understanding. Requires `write`.

```bash
curl -X POST localhost:7070/v1/memories \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' -d '{
    "content": "btw we are switching the backend to Hetzner, fly.io got too expensive. Also always write table-driven tests in Go"
  }'
```

```json
{"job_id": "019f7c5a-…", "status": "pending", "poll": "/v1/jobs/019f7c5a-…"}
```

`202`, not `201`: the work is a model call that takes seconds, and holding
the request open for it would make your timeout our problem and lose the
work on a disconnect.

| Field | Required | Notes |
|---|---|---|
| `content` | yes | Raw text. No length limit beyond what your model can read |
| `category` | no | A hint. Extraction may find several memories that don't share it |
| `tags` | no | Applied to everything extracted from this content |
| `client`, `session_id` | no | Recorded as the source of every memory produced |
| `wait` | no | Run it now and answer with the result — see below |

The example above becomes **two** memories, each separately recallable,
filterable and supersedable. If one of them contradicts something already
stored, the old memory is superseded rather than kept alongside — that is
the part `:direct` cannot do for you.

#### `wait: true`

Runs the pipeline inline and answers `201` with what happened:

```json
{
  "job_id": "019f7c5a-…",
  "status": "succeeded",
  "memory_ids": ["019f7c61-…", "019f7c62-…"],
  "understanding": true
}
```

For callers with nowhere to put a job id — the MCP tools use it, because
an agent has to tell the user what happened in one turn. Everything else
should take the `202`. `understanding` is `false` when no provider is
configured, so you can tell "extracted and reconciled" from "stored as
sent".

### `GET /v1/jobs/{id}`

How an ingestion is going. Requires `read`. Only your own jobs are
visible; someone else's id reads as `404`.

```json
{
  "job_id": "019f7c5a-…",
  "status": "succeeded",
  "attempts": 1,
  "memory_ids": ["019f7c61-…"],
  "created_at": "2026-07-20T09:14:02Z",
  "updated_at": "2026-07-20T09:14:05Z"
}
```

| `status` | Meaning |
|---|---|
| `pending` | Queued, or waiting out a retry backoff |
| `running` | A worker has it |
| `succeeded` | Done. `memory_ids` is what it produced |
| `failed` | Out of attempts. `error` says why |

`memory_ids` can be empty on success: "nothing here was worth
remembering" is a legitimate outcome, and the most common one.

An `error` field can be present while a job is still `pending` — that is
the reason for the previous attempt, kept because "it worked eventually,
but here is what went wrong" is the more useful record.

### Without a provider

`[understanding].provider = "none"` is the default. Both endpoints keep
working and the response shapes are identical; `/v1/memories` simply
stores the content as sent, inferring only the category from unambiguous
phrasing ("I prefer", "we decided"). Nothing is split, and nothing is
reconciled — so a contradiction sits alongside what it contradicts.

Configuring a provider changes that with no client changes at all. See
[configuration.md](configuration.md).

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

### `GET /v1/profile`

Requires `read`. Returns the markdown digest described in
[mcp.md](mcp.md#resource-memoryprofile) — the user's standing
preferences, decisions and durable facts, grouped by category and capped
at roughly 1500 tokens.

The same content the MCP `memory://profile` resource serves, so an agent
and a shell script see one profile.

### `GET /v1/audit`

Requires `read`. `?limit=` (default 100, max 500). Every mutation this
user has made, newest first, including deletions.

## Categories

A closed taxonomy, extensible via `[understanding.taxonomy]`:

`preference.coding` · `preference.personal` · `decision` ·
`fact.project` · `fact.person` · `experience` · `skill` · `reference`

Unknown category names are rejected rather than silently created — a typo
would otherwise produce a category nothing ever matches.

## Not yet implemented

`POST /v1/sessions/distill` (submit a whole transcript), and `GET /metrics`.

See [project-plan.md §9](../project-plan.md#9-api-design-rest--mcp) for the
full design.
