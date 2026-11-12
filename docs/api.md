# REST API

**Status: Phase 5.** Authentication, the memory endpoints, the profile and
the async understanding pipeline are live; agents can also reach all of it
over [MCP](mcp.md).
See [implementation-plan.md](../implementation-plan.md) for what lands when.

## Authentication

Every route under `/v1` requires an API key as a bearer token:

```bash
curl -H "Authorization: Bearer ra_live_b99f884a…" localhost:7070/v1/ping
```

Keys are issued from the CLI (`recuerdos-ai key issue`) — there is no
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

### The structure every memory carries

Every memory the API returns (saves, reads, search hits, edits, the JSON
export) also says what sort of statement it is, what it is about, and who it
is true for ([ai-specs/memory-structure-plan.md](../ai-specs/memory-structure-plan.md)):

```json
{
  "kind": "preference",
  "topic": "coding",
  "applies_to": {"target": "me"},
  "origin": "user_said",
  "needs_review": true
}
```

| Field | Values |
|---|---|
| `kind` | `preference`, `rule`, `convention`, `decision`, `fact`, `experience`, `learning`, `procedure`, `reference`, `goal`, `plan` |
| `topic` | `coding`, `tech`, `work`, `family`, `people`, `health`, `lifestyle`, `home`, `finance`, `travel`, `food`, or one of your own |
| `applies_to.target` | `me` (you, everywhere), `project` (one project, named by `applies_to.project_name`), `general` (your world, not tied to a project) |
| `origin` | `user_said`, `user_confirmed`, `inferred`, `document` |
| `goal_status` | goals only: `active`, `achieved`, `abandoned` |
| `needs_review` | `true` when the structure was derived from an old category and may be wrong |

For now the structure is **derived from the category**, by the mapping in the
plan's §8: every memory saved before this release was given one on the
daemon's first start, and a new memory gets one from the category it is saved
with. Saves that state the structure directly arrive in a later release.

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

Stores a memory exactly as given, with its structure: what it is, what it is
about, who it is true for, and where it came from. Requires `write`. Every field
is explained, with examples, in [memory-model.md](memory-model.md).

```bash
curl -X POST localhost:7070/v1/memories:direct \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' -d '{
    "content": "User forbids barrel files / index.ts re-exports",
    "kind": "preference",
    "topic": "coding",
    "applies_to": {"target": "me"},
    "origin": "user_said",
    "tags": ["typescript", "imports"]
  }'
```

| Field | Required | Notes |
|---|---|---|
| `content` | yes | 1–4000 characters; never a secret |
| `kind` | yes | `preference`, `rule`, `convention`, `decision`, `fact`, `experience`, `learning`, `procedure`, `reference`, `goal`, `plan` |
| `topic` | yes | `coding`, `tech`, `work`, `family`, `people`, `health`, `lifestyle`, `home`, `finance`, `travel`, `food`, or one of yours |
| `applies_to` | no | `{"target": "me" \| "project" \| "general", "project_name": …, "new_project": …}`. Defaults to the known project, else `general`; never `me` |
| `origin` | yes | `user_said`, `user_confirmed`, `inferred`, `document` |
| `subtopic` | no | A finer label within the topic |
| `tags` | no | Lowercased and de-duplicated; max 32 |
| `valid_until` | for a plan | RFC 3339; in the future; not allowed on a goal |
| `category` | no | The old category to also file it under; derived from kind and topic when left out |
| `client`, `session_id` | no | Recorded as the memory's source |

The old shape is refused: a body with only `category` gets `missing_field`, and
`subcategory`, `confidence` and `expires_at` get `invalid_value` naming their
replacement (`subtopic`, derived from `origin`, `valid_until`).

Headers `X-Recuerdos-Project` (a project name) and `X-Recuerdos-Project-Remote`
(a normalised git remote, `github.com/acme/quill`) say which project the caller is
working in; the `recuerdos-ai mcp` shim sends them.

Responds `201` with the memory, plus `message` (how it was filed, to repeat to
the user) and `notes` (`applies_to_defaulted`, `filed_from_document`,
`project_created`):

```json
{
  "id": "019f7c5a-…",
  "content": "User forbids barrel files / index.ts re-exports",
  "kind": "preference", "topic": "coding",
  "applies_to": {"target": "me"}, "origin": "user_said",
  "message": "Saved as your personal coding preference (applies to you everywhere).",
  "notes": []
}
```

A rule that stops the save answers `422` with its code: `missing_field`,
`invalid_value`, `unknown_topic`, `kind_target_mismatch`, `needs_confirmation`,
`scope_contradiction`, `no_project`, `unknown_project` or `secret`. The message
says how to fix it. See [memory-model.md](memory-model.md#when-a-save-is-refused).

### `POST /v1/memories/search`

Hybrid recall: three legs — semantic (vector), keyword (BM25), and a graph
hop over the entity/relation graph — fused by reciprocal rank. Requires
`read`. The graph leg runs when `[graph].enabled` (the default) and the
query names an entity some memory of yours declared; otherwise it stays
silent and recall is exactly its two-leg self.

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
didn't return it. It carries a third key, `graph_rank`, present only when
the graph hop reached a result — the case where a memory answers a
question it shares no words with, reached by walking a relation:

```json
{
  "id": "019f7c5a-...",
  "content": "Nadia is the tech lead of the Meridian team",
  "score": 0.0161,
  "matched": {"graph_rank": 1}
}
```

Here vector and BM25 both missed (the memory never mentions "billing"),
and only the hop `billing service → Meridian team → Nadia` connected the
query to it.

With `[rerank].mode = "on"`, an optional second-stage ranker re-scores the
top of the fused list query-to-memory, and each result it scored carries
two more keys inside `matched`: `rerank_rank` (its position in the
reranker's ordering) and `rerank_score` (its confidence, in `(0, 1)`).
Read `rerank_score` as a diagnostic and rank as the signal: every model's
output is squashed onto that interval by a strictly increasing function, so
**ranks are comparable across models while score magnitudes are not** — and
neither number is a calibrated probability that the memory answers the query.
`relevance` keeps its calibrated meaning either way.
A result outside the reranked head (`[rerank].depth`) carries neither key, and
so does any result in a one-candidate head — there is nothing to reorder — so
their absence does not mean the stage is off. Off by default, and never
visible in `shadow` mode: a server without a reranker, or one running shadow
mode, produces exactly the `matched` objects above.

What the stage *did*, as opposed to what it did to each row, is reported once
per response in a top-level `rerank` object. It is present whenever a stage is
configured and absent when `mode = "off"`, so a deployment without a reranker
gets back exactly the response it has always got:

```json
{
  "results": [ … ],
  "took_ms": 258,
  "min_relevance": 0.25,
  "rerank": {"state": "skipped", "reason": "deadline", "candidates": 10, "took_ms": 251}
}
```

| `state` | Meaning |
|---|---|
| `applied` | Judged `judged` candidates in `took_ms`, and its order is folded into the list above. `judged` can be below `[rerank].depth`: the deadline arrived part way through the head, so those candidates are reordered and the rest keep the order fusion gave them |
| `shadow` | Judged them and logged what it *would* have served; `changed` says whether the top result would have moved |
| `not_needed` | Fewer than two candidates cleared `min_relevance`, so there was nothing to reorder |
| `skipped` | Asked, and it did not contribute — `reason` is `deadline` (past `[rerank].timeout_ms`) or `model`. The results above are the pre-rerank order |

`skipped` is the one worth noticing. It exists because its absence cost an
operator an afternoon: a stage that misses its deadline serves the pre-rerank
order *by design* — the request must never fail because of the second stage —
and that looks exactly like `mode = "off"` from the client's side. On a machine
that is not idle it is the ordinary outcome for a deep head, not a rare one;
[size it against your own recalls](configuration.md#sizing-timeout_ms-for-your-machine)
rather than trusting the default, which was measured on a quiet benchmark box.

One thing `deadline` cannot say, deliberately: whether the stage ran out of time
*judging* or never got a session to judge with. Both are `deadline` here because
clients and SDKs parse two reason values and a third breaks them, and the
remedies are opposite — a slow model wants a larger `timeout_ms`, a contended one
wants `[rerank].max_concurrency` and a *smaller* timeout. The daemon's log carries
both halves (`queued_ms`, `inference_ms`) for every leg like that.

| Field | Notes |
|---|---|
| `query` | Required, 1–1000 characters |
| `limit` | Default from `[retrieval].default_limit`; capped at 50 |
| `categories` | OR-ed. Empty means all |
| `tags` | **AND-ed** — a memory must carry every one |
| `since` | RFC 3339; excludes memories created before it |
| `as_of` | RFC 3339. Reads the **graph hop** as of that instant in valid time — "who owned this before the reorg?" Omitted means now. Only the graph leg is affected; the vector and keyword legs ignore it |
| `include_superseded` | Default `false` |

`as_of` is *valid* time — when a relation was true in the world — which is
a different clock from `since` and `created_at`, both of which are
*transaction* time, when the memory was learned. `as_of` reads history the
graph still holds: an edge a later memory contradicted keeps its old
interval, so a hop dated before the change still traverses it.

Filters are applied after the indexes answer, over a candidate window
several times `limit`. A very selective filter over a large corpus can
therefore return fewer than `limit` results even when more exist.

### `GET /v1/memories/{id}`

Requires `read`. `404` both when the memory doesn't exist and when it
belongs to another user — the two are deliberately indistinguishable.

The response carries an `ETag`: the memory's `updated_at`, quoted. Send it
back as `If-Match` on a `PATCH` to make the edit conditional.

### `PATCH /v1/memories/{id}`

Requires `write`. Any of `content`, `category`, `subcategory`, `tags`,
`expires_at`. Omitting a field leaves it alone; sending `null` for
`subcategory` or `expires_at` clears it. Editing content re-embeds and
re-indexes; so does editing the labels under `document_text = "enriched"`.

**Conditional edits.** With `If-Match: "<updated_at>"` (the `ETag` of a
`GET`, `PATCH` or restore, or the `updated_at` of any listing), the edit
applies only if the memory has not changed since. If it has, the answer is
`412 {"error": {"code": "precondition_failed", …}}` and nothing changes:
re-read it and decide. `If-Match: *` or no header edits unconditionally,
as before. The response carries the new `ETag`.

### `DELETE /v1/memories/{id}`

Requires `write`. Responds `204`. A soft delete: the memory stops being
recalled, and the audit trail keeps what happened to it.

### `POST /v1/memories/{id}/restore`

Requires `write`. Undoes a `DELETE`: the memory comes back with its id,
`created_at` and history, is re-embedded and re-indexed, and is recalled
again. Answers the memory (with an `ETag`). `404` when it is not yours or
was never forgotten. Audited as `restore`.

### `GET /v1/map`

Requires `read`. Your memories as a graph, for the web UI's mental map:

```json
{
  "generation": "4182",
  "k": 5,
  "min_weight": 0.15,
  "edges_omitted": false,
  "detail": "memories",
  "total": 222,
  "category": null,
  "nodes": [{"id": "…", "content": "…", "category": "preference.coding", …}],
  "edges": [
    {"kind": "similar", "source": "…", "target": "…", "weight": 0.62},
    {"kind": "relation", "source": "…", "target": "…", "predicate": "deploys_on", "entity": "Hetzner"}
  ],
  "categories": [{"id": "preference.coding", "label": "Coding preference", "built_in": true,
                  "count": 72, "subcategories": [{"name": "testing", "count": 9}]}],
  "category_edges": [{"source": "decision", "target": "fact.project", "weight": 0.41}]
}
```

- **`nodes`**: your active memories, plus superseded ones with
  `?include_superseded=true`.
- **`similar` edges** join *mutual* nearest neighbours: each of the two memories is
  among the other's `k` most similar (`?k=1..15`, default 5).
  - "Mutual" stops one generic memory from linking to everything.
  - `weight` is the similarity above the embedding model's baseline for
    unrelated text (the same calibration recall uses), from 0 to 1. Only links
    of at least `min_weight` are returned.
- **`relation` edges** come from the entity graph, and only when `[graph].enabled`
  is on. One memory stated a relation about an entity the other mentions.
- **`category_edges`** add up the similarity between categories, scaled by
  their sizes.
- **`generation`** changes whenever your memories do. The similarity links are
  cached until it changes.
- **Above 3 000 memories** the pairwise comparison is skipped and
  `edges_omitted` is `true`.
- **`?category=<id>`** limits the map to one category's memories, with the
  similarity links among them. A category nobody uses gives an empty map,
  not an error. `category` in the answer echoes it.
- **Above the detail limit** (`[ui].map_detail_limit`, 5 000 by default), a
  whole-store map comes back as `"detail": "categories"`:
  - `nodes` and `edges` are empty;
  - `categories` carries every category with its count;
  - `category_edges` link each category to its 3 most similar, by comparing the
    categories' mean vectors.

  Ask with `?category=` for each category's memories. A category map is
  always `"detail": "memories"`, whatever its size.
- **`total`** is how many memories are in scope, whether or not `nodes`
  carries them.

### `POST /v1/memories:bulk`

Requires `write`. One edit applied to many memories:

```json
{"ids": ["…", "…"],
 "set": {"category": "skill", "subcategory": null},
 "add_tags": ["rust"], "remove_tags": ["misc"]}
```

- **`set`** fields are optional. `subcategory: null` clears the sub-label;
  leaving it out leaves it alone. `category` must be one of yours.
- **All or nothing.** Every id must be one of your active memories, or
  nothing changes and the answer is `404` naming the ones that were not.
  Unchanged memories are left alone.
- **At most 500 ids**, and the edit must change something (`400` otherwise).
- Memories whose embedded text changed are re-embedded (under the default
  `document_text = "enriched"`, a re-file or re-tag does that). The new
  vectors are computed before anything is written, so a failing embedder
  changes nothing.
- One audit entry per memory, all with the same `detail`
  (`bulk edit of 2 memories: category → skill; +tags rust`).

Answers `{"memories": [...], "changed": 2, "reembedded": 2}`.

## Categories you can change

Each user has their own taxonomy: the eight built-ins, plus custom
categories they create. A category created here is valid at once for
`memory_save`, `POST /v1/memories:direct`, search filters, and the AI
classifier, which is offered it on that user's next ingest.

### `GET /v1/categories`

Requires `read`. Every category, with how many of your active memories are
filed under it:

```json
{"categories": [
  {"id": "preference.coding", "label": "How I code", "default_label": "Coding preference",
   "built_in": true, "known": true, "count": 12},
  {"id": "health.sleep", "label": "Sleep log", "default_label": "Health sleep",
   "built_in": false, "known": true, "count": 3}
]}
```

Built-ins come first, in taxonomy order, even when empty, then your custom
categories by id. Last come any categories still on one of your memories but
not in your list (`known: false`), so they can be found and moved, or added
back with `POST /v1/categories`.

### `POST /v1/categories`

Requires `write`. `{"id": "health.sleep", "label": "Sleep log"}` (label
optional). The id is lowercased and must start with a letter and use only
`a–z`, `0–9`, `.`, `_` and `-`, up to 64 characters. `409` when you already
have it or it is a built-in's id. Answers `201` and the category.

### `PATCH /v1/categories/{id}`

Requires `write`. Either field, or both:

- **`label`**: what the UI shows, for any category, built-ins included.
  `null` or blank goes back to the default wording. Display only: nothing is
  re-embedded, and agents keep filtering by the id.
- **`id`**: renames a custom category (`409` for a built-in, whose id never
  changes). Every memory filed under it moves to the new id and is
  re-embedded, because the enriched vector's text names the category. Agents
  filtering by the old id need the new one.

Answers `{"category": {...}, "moved": 5, "reembedded": 5}`.

### `POST /v1/categories/{id}/merge`

Requires `write`. `{"into": "fact.project"}`: moves every memory of custom
category `id` into `into` (a built-in or another of yours), then deletes
`id`. Answers like `PATCH`.

### `DELETE /v1/categories/{id}`

Requires `write`. Deletes a custom category. While memories are still filed
under it the answer is `409`, unless `?move_to=<category>` says where they
go (then it is a merge). Built-ins cannot be deleted.

All category writes and bulk edits are audited per memory moved. The audit
`actor` is the client's own name from an `X-Recuerdos-Client` header when it
sends one (the web UI sends `web-ui`), else `rest`. It is informational: any
client can send any name.

### `GET /v1/memories/export`

Requires `read`. `?format=markdown` (default) or `?format=json`, plus
`?include_inactive=true` to include superseded memories.

Markdown is grouped by category, one memory per line, so it stays
greppable and diffable — a git-versioned backup is a `>` away.

JSON items carry `id`, `content`, `category`, `subcategory` (or `null`),
`tags`, `confidence`, `created_at`, `updated_at`, `superseded_by`, and the
structure: `kind`, `topic`, `applies_to`, `origin`, `goal_status` (or `null`)
and `needs_review`.

### `POST /v1/sessions/distill`

Requires `write`. Reduces a finished session to the few things that
outlive it.

```bash
curl -X POST localhost:7070/v1/sessions/distill \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' -d '{
    "content": "…a transcript, or a summary of one…",
    "session_id": "s-42",
    "client": "claude-code"}'
```

```json
{"memory_ids": ["mem_01J…", "mem_01K…"], "distilled": 2}
```

| field | |
|---|---|
| `content` | The session. Required, and capped at 200 000 characters — summarize anything longer. |
| `session_id` | Your own id for the session, recorded on every memory it produces. |
| `client` | Recorded as the source. Defaults to `session-distill`. |
| `tags` | Applied to everything the session yields. |

Synchronous, unlike `POST /v1/memories`: the caller is a session-end hook
with nowhere to put a job id and no next turn in which to poll.

`"distilled": 0` is the ordinary outcome — most sessions produce nothing
that stays true after they end — and is a success, not a failure.

This is the same extraction and reconciliation pipeline as
`POST /v1/memories`, asked a stricter question: *what is still true after
this session ends?* A convention established in this session that
contradicts an older one supersedes it rather than landing beside it.

**Requires a provider.** With `[understanding].provider = "none"` this
returns `400`. It is the one endpoint that does not degrade to storing
content verbatim — a whole transcript as a single memory is unrecallable
and would be spent from a context window on every match, which is worse
than storing nothing.

### `GET /v1/profile`

Requires `read`. Returns the markdown digest described in
[mcp.md](mcp.md#resource-memoryprofile) — the user's standing
preferences, decisions and durable facts, capped at roughly 1500 tokens.

With a provider configured this is written by a model and cached until
the memories under it change; without one it is assembled from the
memories directly. The route, the media type and the shape are identical
either way.

The same content the MCP `memory://profile` resource serves, so an agent
and a shell script see one profile.

### `GET /v1/audit`

Requires `read`. `?limit=` (default 100, max 500). Every mutation this
user has made, newest first, including deletions.

## The taxonomy

Eight built-in categories, always valid:

`preference.coding` · `preference.personal` · `decision` ·
`fact.project` · `fact.person` · `experience` · `skill` · `reference`

Each user adds their own with `POST /v1/categories`.
`[understanding.taxonomy].extra_categories` seeds every user's list the first
time it is read. Unknown category names are rejected rather than silently
created: a typo would otherwise produce a category nothing ever matches.

## Not yet implemented

`GET /metrics`.

See [project-plan.md §9](../ai-specs/project-plan.md#9-api-design-rest--mcp) for the
full design.
