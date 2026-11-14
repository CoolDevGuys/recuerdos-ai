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
| *(a memory rule's code)* | 422 | A save or edit refused by a memory rule; see [memory-model.md](memory-model.md#when-a-save-is-refused) |
| `internal` | 500 | Server fault |

A `422` may carry `details`: for a `conflict` (the memory contradicts one already
stored), the stored memories it names. A `422 conflict` and a `409 conflict` are
different things; tell them apart by the status.

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
{"user": "alex", "scopes": ["read", "write"], "understanding": false}
```

`understanding` says whether a language model is configured, so a client offers
only what works (the web UI's review queue shows **Suggest** only with one).

### `GET /v1/about`

Requires any valid key. What the daemon says about itself: the web UI's **About**
page. Every key gets the version, build, start time and who it belongs to; an
**admin** key (or `[auth].mode = "none"`) also gets how the daemon was started.

```json
{
  "version": "0.7.0", "git_sha": "f66418ce03e5", "started_at": "2026-11-14T08:00:00Z",
  "user": "ops", "scopes": ["admin"], "auth_mode": "api-key", "understanding": true,
  "daemon": {
    "command": ["serve", "--config", "/etc/recuerdos-ai/recuerdos-ai.toml"],
    "config_file": {"path": "/etc/recuerdos-ai/recuerdos-ai.toml", "from": "flag", "exists": true},
    "overrides": ["RECUERDOS_AI_AUTH__MODE", "RECUERDOS_AI_SERVER__PORT"],
    "settings": [{"name": "embeddings", "settings": [{"key": "provider", "value": "local"}]}]
  }
}
```

`daemon` is `null` for any other key. `config_file.from` is `flag` (`--config`),
`env` (`RECUERDOS_AI_CONFIG`) or `none`; `exists: false` means the named file is
missing and defaults apply. `overrides` are the `RECUERDOS_AI_*` variables set at
start, **by name only**. `settings` is the report `recuerdos-ai config` prints: an
API key appears as the variable that holds it and whether it is set, never its
value.

### `GET /v1/me`, `PUT /v1/me`

Who "the user" is in this store. `GET` requires `read`; `PUT` requires `write` and
replaces the list (an empty list clears it).

```bash
curl -X PUT -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"names": ["Alejandro Romero", "Alejandro", "Alex"]}' localhost:7070/v1/me
```

```json
{"names": ["Alejandro Romero", "Alejandro", "Alex"],
 "sentence": "The user is Alejandro Romero (also called Alejandro, Alex).",
 "flagged": 2}
```

The first name is the one the user goes by in full; at most 8, each up to 64
characters, trimmed, with repeats removed. With them, a memory that names the user is
known to be about them: a save filing it as `general` is refused as `about_the_user`,
the pipeline files it as theirs, and the profile opens with `sentence`. On `PUT`,
`flagged` counts the stored memories it revealed as the user's own (filed as
`general`) that were sent to the review queue; nothing is re-filed without the user.

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
an agent has to tell the user what happened in one turn.

Extraction gives every memory the same structure a direct save states, under the
same rules. Send `X-Recuerdos-Project` (and `X-Recuerdos-Project-Remote`) to say
which project the text is about; only a project you already have is used, since
a job cannot ask whether a new one is meant. A project memory with no known
project is filed as `general` and marked `needs_review`. Each memory is reconciled
only against stored memories for the same scope. What the model only inferred
about you is never stored as yours: it comes back in `suggestions`, with outcome
`needs_confirmation` when nothing else was stored, for the agent to ask you about. Everything else
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
| `replaces` | no | The id of a stored memory this one replaces: the user changed their mind. It must be yours, active and filed for the same `applies_to`; it is superseded (out of recall, kept in history) |
| `coexists_with` | no | Ids of stored memories the user said this does not replace: both hold, so they are not reported as conflicts |

The old shape is refused: a body with only `category` gets `missing_field`, and
`subcategory`, `confidence` and `expires_at` get `invalid_value` naming their
replacement (`subtopic`, derived from `origin`, `valid_until`).

Headers `X-Recuerdos-Project` (a project name) and `X-Recuerdos-Project-Remote`
(a normalised git remote, `github.com/acme/quill`) say which project the caller is
working in; the `recuerdos-ai mcp` shim sends them.

Responds `201` with the memory, plus `status: "stored"`, `message` (how it was
filed, to repeat to the user), `notes` (`applies_to_defaulted`,
`filed_from_document`, `project_created`, `replaced`, `overrides`) and, after a
`replaces`, the superseded ids in `replaced`:

```json
{
  "id": "019f7c5a-…",
  "content": "User forbids barrel files / index.ts re-exports",
  "kind": "preference", "topic": "coding",
  "applies_to": {"target": "me"}, "origin": "user_said",
  "status": "stored",
  "message": "Saved as your personal coding preference (applies to you everywhere).",
  "notes": []
}
```

When a stored memory for the same scope already says this, nothing is written:
the answer is `200` with `status: "already_known"` and the memory already stored.

Before anything is written, the save is compared with the nearest stored memories
for the same `applies_to` (and, for a project memory, with the user's own, which it
may override). One that contradicts a stored memory is refused:

```json
{"error": {
  "code": "conflict",
  "message": "It contradicts what is stored for me: \"The user prefers mocks…\" (id 019f…). Ask the user which holds. …",
  "details": {
    "judged": true,
    "conflicts": [{"id": "019f…", "content": "The user prefers mocks…", "kind": "preference",
                   "applies_to": "me", "reason": "…"}]
  }
}}
```

With a language model configured, the model judges it (one call per save, none
when there is nothing to compare and nothing to check). Without one, only a near
repeat of a stored memory is stopped, with `judged: false`. Then: save again with
`replaces` (changed their mind), amend the stored one with `PATCH` (it was wrong),
or save again with `coexists_with` (both hold).

Any other rule that stops the save also answers `422` with its code:
`missing_field`, `invalid_value`, `unknown_topic`, `kind_target_mismatch`,
`needs_confirmation`, `scope_contradiction`, `about_the_user`, `no_project`,
`unknown_project` or `secret`. The message says how to fix it. See
[memory-model.md](memory-model.md#when-a-save-is-refused).

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
| `kinds` | OR-ed: `preference`, `rule`, `convention`, … Unknown → `400` |
| `topics` | OR-ed topic ids: `coding`, or one of yours |
| `targets` | OR-ed: `me`, `project`, `general` |
| `projects` | OR-ed project names, aliases or ids. Unknown → `422 unknown_project` |
| `project` | The project the caller works in, when not sent as the `X-Recuerdos-Project` headers. Its memories rank with the user's own |
| `scope` | `context` (default): another project's memories count half in rank and relevance, unless the query names that project. `all`: every scope alike |

Any structure filter, like a category or tag filter, lifts the default relevance
floor when `min_relevance` is not given. Each hit carries its `kind`, `topic` and
`applies_to`; its `relevance` is the one the floor compared, demotion included.
See [memory-model.md](memory-model.md#recall-and-the-profile-know-where-you-are).

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

Requires `write`. Amends a memory in place: it keeps its id, and the audit trail
keeps what it said before. Any of:

| Field | Notes |
|---|---|
| `content` | The corrected sentence |
| `kind`, `topic`, `applies_to` | A new structure, as a save takes it. Moving a memory to another scope is done here, not with `replaces` |
| `subtopic` (or `subcategory`), `tags`, `category` | Labels. Re-filing under another category leaves a stated structure alone |
| `valid_until` (or `expires_at`) | `null` clears it |
| `goal_status` | `achieved` or `abandoned` closes a goal; `active` reopens it. Closing needs `origin` `user_said` or `user_confirmed`: only the user closes a goal (`inferred` is refused with `needs_confirmation`). A closed goal leaves recall and the profile and stays in history |
| `origin` | Who asked for the change. It becomes the memory's origin when its wording, kind or scope changes |

Omitting a field leaves it alone. What the memory becomes is held to the same
rules as a save (no secret, a kind its target allows, a plan keeps its end date…)
and a refusal is a `422` with its code. An amendment is not checked for conflicts:
it is how one is resolved. Editing content re-embeds and re-indexes; so does
editing the labels under `document_text = "enriched"`. The response is the memory
plus a `message` ("Updated; filed as your personal coding preference.").

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
  "topic": null,
  "nodes": [{"id": "…", "content": "…", "kind": "preference", "topic": "coding", …}],
  "edges": [
    {"kind": "similar", "source": "…", "target": "…", "weight": 0.62},
    {"kind": "relation", "source": "…", "target": "…", "predicate": "deploys_on", "entity": "Hetzner"}
  ],
  "topics": [{"id": "coding", "label": "Coding", "built_in": true,
              "count": 72, "subtopics": [{"name": "testing", "count": 9}]}],
  "topic_edges": [{"source": "coding", "target": "work", "weight": 0.41}]
}
```

The map groups memories **by topic** ([memory-structure-plan.md](../ai-specs/memory-structure-plan.md)
D13); the web UI colours each memory by its kind. A topic's `label` is yours when
you relabelled it (`PATCH /v1/topics/{id}`).

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
- **`topic_edges`** add up the similarity between topics, scaled by their sizes.
- **`generation`** changes whenever your memories do. The similarity links are
  cached until it changes.
- **Above 3 000 memories** the pairwise comparison is skipped and
  `edges_omitted` is `true`.
- **`?topic=<id>`** limits the map to one topic's memories, with the similarity
  links among them. A topic nobody uses gives an empty map, not an error.
  `topic` in the answer echoes it.
- **Above the detail limit** (`[ui].map_detail_limit`, 5 000 by default), a
  whole-store map comes back as `"detail": "topics"`:
  - `nodes` and `edges` are empty;
  - `topics` carries every topic with its count;
  - `topic_edges` link each topic to its 3 most similar, by comparing the topics'
    mean vectors.

  Ask with `?topic=` for each topic's memories. A topic map is always
  `"detail": "memories"`, whatever its size.
- **`total`** is how many memories are in scope, whether or not `nodes`
  carries them.

### `POST /v1/memories:bulk`

Requires `write`. One edit applied to many memories:

```json
{"ids": ["…", "…"],
 "set": {"kind": "reference", "topic": "tech", "applies_to": {"target": "general"},
         "subcategory": null},
 "add_tags": ["rust"], "remove_tags": ["misc"]}
```

- **`set`** fields are optional. `subcategory: null` clears the sub-label;
  leaving it out leaves it alone. `category` must be one of yours.
- **`kind`, `topic` and `applies_to`** file every memory under the same structure,
  as the review queue does ([memory-model.md](memory-model.md#memories-saved-before-structured-memories)).
  Each memory must be able to take it under the save rules — a kind its target
  allows, no project-bound wording ("this repo") filed as yours or general — or
  nothing changes: `422 invalid_value` names the memories that cannot. A project
  must already exist (`422 unknown_project`). The memories' origin becomes
  `user_confirmed`, and they leave the review queue.
- **`reviewed: true`** confirms each memory's structure as it is: it leaves the
  review queue. A structure the rules refuse cannot be confirmed.
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

### `POST /v1/memories:suggest`

Requires `read`. `{"ids": ["…", …]}`, at most 25: what each memory may really be,
for the review queue. One model call; nothing is written.

```json
{"suggestions": [
  {"id": "…", "kind": "reference", "topic": "tech",
   "applies_to": {"target": "general"}, "reason": "a path on the home NAS"}
]}
```

A suggestion is kept only when the rules allow it (a kind its target allows, a
topic and project you have); a memory the model could not place is missing from
the answer. Without a model: `422 no_model`.

## Topics

What memories are about (memory-model.md, `topic`). The built-ins are fixed; you
add your own.

### `GET /v1/topics`

Requires `read`. Built-ins in their order, then yours:

```json
{"topics": [{"id": "coding", "label": "Software", "default_label": "Coding",
             "built_in": true, "count": 72}]}
```

`count` is your active memories about it.

### `POST /v1/topics`

Requires `write`. `{"id": "homelab", "label": "Home lab"}` (label optional). The id
starts with a letter and uses `a–z 0–9 - _`, up to 32 characters. `409` when you have
it or it is a built-in's. Answers `201` and the topic; saves can use it at once.

### `PATCH /v1/topics/{id}`

Requires `write`. `{"label": "Software"}`; `null` or blank goes back to the default
wording. Any topic, built-ins included. Display only: memories store the id. To move
memories between topics, bulk-edit them (`set.topic`).

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

## Projects

What memories with `applies_to.target: "project"` are filed under
([memory-model.md](memory-model.md#projects)). Wherever a path takes
`{project}`, it may be the project's id, its name, or one of its aliases (a
former name, a git remote with `/` encoded as `%2F`, a folder name). An unknown
one is `422 unknown_project`, listing the known names.

### `GET /v1/projects`

Requires `read`. Your projects, by name:

```json
{"projects": [
  {"id": "019f…", "name": "quill", "memories": 12, "created_at": "2026-11-12T08:00:00Z",
   "aliases": ["github.com/acme/quill", "quill-old"]}
]}
```

`aliases` are what else finds the project: remotes, folder names, and the names it
had before a rename. `memories` counts its active ones. `GET /v1/projects/{project}`
answers one.

### `PATCH /v1/projects/{project}`

Requires `write`. `{"name": "quill-app"}` renames it. Memories store the project's
id, so every one follows and nothing is re-embedded; the old name becomes an alias.
Sent with the `X-Recuerdos-Project` headers of a folder whose project is the new
name (the local MCP server sends them), that folder's remote is learned too: the fix
for a repository renamed on its host. `409` when another project has the name (merge
them instead); `422 invalid_value` for a name that is not lowercase `a–z 0–9 . _ -`.

```json
{"project": {"id": "019f…", "name": "quill-app", "aliases": ["github.com/acme/quill",
             "github.com/acme/quill-app", "quill"], "memories": 12, "created_at": "…"},
 "message": "Renamed quill to quill-app: its 12 memories moved with it, and quill still finds it. It is now also found by github.com/acme/quill-app.",
 "previous": "quill", "moved": 0, "learned": ["github.com/acme/quill-app"]}
```

### `POST /v1/projects/{project}/merge`

Requires `write`. `{"into": "quill"}`: every memory of `{project}`, superseded ones
included, moves to `into` (audited per memory); `{project}`'s name and aliases find
`into` from then on, and it is deleted. Answers like `PATCH`, with `moved` and
`previous` (the project that went). Merged memories are not checked against each
other; consolidation merges duplicates. `422 invalid_value` for a project merged
into itself.

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
[mcp.md](mcp.md#resource-memoryprofile) — about the user, then the current
project, then their general world — capped at roughly 1500 tokens. The
project is `?project=<name>`, or the `X-Recuerdos-Project` headers; without
one, no project's memories are in it.

With a provider configured this is written by a model and cached until
the memories under it change; without one it is assembled from the
memories directly. The route, the media type and the shape are identical
either way.

With the user's names set (`PUT /v1/me`), the line under the title says who "the
user" is: `The user is Alejandro Romero (also called Alex).`

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
