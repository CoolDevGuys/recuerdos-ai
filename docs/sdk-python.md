# Python SDK

A thin typed client over the [REST API](api.md). Source lives in
[`sdk/python`](../sdk/python).

```bash
pip install recuerdos-ai
```

```python
from recuerdos-ai import Client

ra = Client(base_url="http://localhost:7070", api_key="ra_live_…")
ra.save("We moved the backend to Hetzner; fly.io got too expensive")

for hit in ra.search("where do we deploy?"):
    print(hit.content)
```

Requires Python 3.10+. Two runtime dependencies (`httpx`, `pydantic`);
LangChain support is an extra.

## Why it is thin

One method per endpoint, typed models in and out, and no caching,
batching or retry logic of its own. The service already decides what is
worth remembering and when to supersede it — an SDK with its own opinions
about that would be a second place for those opinions to drift.

The two things it does add are the two a raw `httpx` call makes awkward:
typed errors keyed off the API's stable error `code`, and job polling.

## Constructing a client

```python
Client(base_url="http://127.0.0.1:7070", api_key=None, *, timeout=30.0, client=None)
```

| | |
|---|---|
| `base_url` | Where the daemon is. |
| `api_key` | From `recuerdos-ai key issue`. Optional only because `[auth].mode = "none"` accepts unauthenticated requests. |
| `timeout` | Per-request, in seconds. |
| `client` | Bring your own `httpx.Client` — for a proxy, custom transport or shared pool. Its lifetime stays yours; the SDK will not close it. |

**A key is a user.** There is no user parameter anywhere in the class,
because the key already determines whose memories these are. Two users
means two clients.

Use it as a context manager, or call `close()`:

```python
with Client(api_key="ra_live_…") as ra:
    ra.search("anything")
```

## Writing

### `save(content, *, category, tags, client, session_id, wait=False) -> Job`

Raw text in. The service extracts atomic memories from it, labels them,
and checks each against what is already known — so a contradiction
supersedes what it replaces instead of accumulating beside it.

Returns a `Job`, because the work is a model call taking seconds. The
returned job is `pending` with empty `memory_ids`.

```python
job = ra.save("we're switching to Hetzner; also always write table-driven tests")
finished = ra.wait_for_job(job)
print(finished.memory_ids)   # two ids: one fact, one preference
```

`wait=True` runs the pipeline inline and returns a terminal job. Fine in
a script; a poor idea anywhere holding a request open, which is why it is
not the default.

### `save_and_wait(content, **kwargs) -> list[Memory]`

`save`, then `wait_for_job`, then fetch each result. The convenience
wrapper for scripts and notebooks, where a job id is a nuisance.

```python
memories = ra.save_and_wait("User is vegetarian now")
```

An empty list is a legitimate outcome — most text contains nothing that
stays true beyond the conversation.

### `save_direct(content, *, kind, topic, origin, applies_to, subtopic, tags, valid_until, replaces, coexists_with, …) -> SavedMemory`

Stores one memory as given, with its structure. Synchronous, cheap, no model.
Every field is explained in [memory-model.md](memory-model.md).

```python
from recuerdos_ai import AppliesTo, RejectedError

saved = ra.save_direct(
    "User forbids barrel files / index.ts re-exports",
    kind="preference",
    topic="coding",
    applies_to="me",
    origin="user_said",
    tags=["typescript", "imports"],
)
print(saved.message)  # Saved as your personal coding preference (applies to you everywhere).

ra.save_direct(
    "Quill forbids mocking the database",
    kind="rule",
    topic="coding",
    applies_to=AppliesTo(target="project", project_name="quill", new_project=True),
    origin="user_said",
)
```

A memory rule that stops the save raises `RejectedError`, whose `code` says which
(`needs_confirmation`, `kind_target_mismatch`, `secret`, …) and whose message says
how to fix it. `Memory` carries the structure too: `kind`, `topic`, `applies_to`,
`origin`, `goal_status` and `needs_review`.

Before storing, the daemon compares it with the nearest stored memories for the
same scope. One that contradicts a stored memory (judged by the model when one is
configured; without one, only a near repeat) raises `RejectedError` with
`code == "conflict"`, and `error.conflicts` lists the stored memories (`id`,
`content`, `kind`, `applies_to`, `reason`). Ask the user which holds:

```python
try:
    saved = ra.save_direct("The user now avoids mocks in unit tests", **structure)
except RejectedError as refused:
    if refused.code != "conflict":
        raise
    old = refused.conflicts[0]["id"]
    # They changed their mind: replace it (kept in history) …
    saved = ra.save_direct("The user now avoids mocks in unit tests", replaces=old, **structure)
    # … or both hold: ra.save_direct(..., coexists_with=[old], **structure)
    # … or the stored one was wrong: ra.update(old, content=...)
```

`saved.status` is `"already_known"` when a stored memory already says this:
nothing was written, and the memory returned is that one. Use `save_direct` when
you have already decided what the memory should say; prefer `save` for raw
material.

### `distill_session(content, *, session_id, client, tags) -> Distillation`

Reduces a finished session to what outlives it.

```python
result = ra.distill_session(transcript, session_id="s-42", client="my-agent")
print(result.distilled)   # usually 0
```

Pass what actually happened rather than pre-filtering to the "important"
parts — everything about the task itself is discarded on purpose, and what
survives is conventions, decisions with their reasons, and root causes.

The one method that needs a configured provider: without one it raises
`ValidationError` rather than storing your transcript as a single
unrecallable memory.

## Reading

### `search(query, *, limit, categories, tags, since, as_of, include_superseded, kinds, topics, targets, projects, project, scope) -> list[SearchHit]`

`kinds`, `topics`, `targets` and `projects` filter by structure (each OR-ed).
`project` is the project the caller works in: its memories rank with the user's
own, and other projects' count half unless the query names them. `scope="all"`
ranks every project alike. Each hit carries `relevance`.

Hybrid recall — semantic, keyword, and a graph-hop leg fused by reciprocal
rank.

```python
hits = ra.search("which package manager?", limit=3, categories=["preference.coding"])
for hit in hits:
    print(hit.score, hit.content, hit.matched.vector_rank, hit.matched.bm25_rank)
```

Ask the question you actually have rather than keywords. Exact
identifiers (`useQuery`, a ticket id) work too — that is the keyword
leg's reason for existing. When the query names an entity the store knows,
a third leg walks the entity graph to reach a memory that answers it
without sharing a word (`hit.matched.graph_rank` is set when that is why a
result appeared).

`categories` are OR-ed; `tags` are **AND**-ed. `hit.matched` says which
leg found the result, so a surprising ranking can be explained rather
than merely distrusted. `as_of` (a `datetime`) reads the graph hop in
*valid* time — "who owned this before the reorg?" — leaving the vector and
keyword legs untouched. `min_relevance` overrides the daemon's floor for
this one query: pass `0.0` to see the near-misses, a higher number when the
surface has a cost to being wrong.

### `search_report(query, *, limit, categories, tags, since, as_of, min_relevance, include_superseded) -> SearchReport`

The same request, keeping the parts of the response that are not about any
one hit:

```python
report = ra.search_report("which package manager?", limit=5)
print(len(report.hits), report.took_ms, report.min_relevance, report.rerank)
```

| Field | Why it earns a variable |
|---|---|
| `hits` | What `search()` returns |
| `took_ms` | The server's own timing, so a slow call can be told apart from a slow network on the way to it |
| `min_relevance` | The floor that produced the list. An empty list at a floor above `0.0` means "nothing cleared it", not "nothing matched" — which is exactly what a retry at `min_relevance=0.0` turns on. A request that names a `category` or `tag` reports `0.0` unless you pass one yourself: filtering in the store's own vocabulary is already a statement of what you want, so the daemon does not let weak wording empty the answer |
| `rerank` | What the optional second stage did, or `None` when there is no stage |

`rerank` is the reason the method exists. A reranker that misses
`[rerank].timeout_ms` serves the pre-rerank order **by design** — the second
stage may cost milliseconds and must never cost a request — and before the
server reported it, that was indistinguishable from `mode = "off"`. It is a
small union keyed on `state`:

| `rerank.state` | Means |
|---|---|
| `applied` | Judged `judged` candidates in `took_ms`, and its order is the list above. `judged` may be fewer than the server's `[rerank].depth`: the deadline arrived part way through the head, so the judged part is reordered and the tail keeps fusion's order |
| `shadow` | Judged them and logged what it *would* have served; `changed` says whether the top result would have moved |
| `not_needed` | Fewer than two candidates cleared the floor, so there was nothing to reorder |
| `skipped` | Configured, and did not contribute: `reason` is `deadline` (raise `timeout_ms`, or lower `[rerank].depth`) or `model` |

`deadline` covers two faults on purpose: a stage that ran out of time judging,
and one that never got a session on a busy host, both say `deadline`, because the
union has two reason values and a third would break every client written against
it. The server's log says which, and only the server can: the fix for one is a
bigger `timeout_ms`, for the other a smaller one.

A server older than 0.4.2 never sends the key, which reads as `None` — the
same as `mode = "off"`, and correct, because to those servers the stage really
is invisible.

### `get(memory_id) -> Memory`

Raises `NotFoundError` both when the memory does not exist and when it
belongs to another user — the two are indistinguishable by design, so the
SDK cannot tell them apart either.

### `profile() -> str`

The user's standing profile as markdown, roughly 1500 tokens. Read it
before the first turn of a session.

Written by a model and cached when a provider is configured; assembled
from the memories directly when not. Same shape either way.

### `export(*, format="markdown", include_inactive=False) -> str`

Every memory as markdown (grouped by category, greppable) or JSON.

## Modifying

### `update(memory_id, *, content, kind, topic, applies_to, subtopic, tags, valid_until, clear_valid_until, goal_status, origin, …) -> UpdatedMemory`

Amends a memory in place; omitted fields are left alone. The same rules as a save
apply to what it becomes, and a refusal raises `RejectedError`. Editing `content`
re-embeds and re-indexes, and the audit trail keeps the old text. The result
carries the server's `message`.

```python
ra.update(memory_id, content="The user prefers mocks in unit tests")  # a typo fixed
ra.update(goal_id, goal_status="achieved", origin="user_confirmed")    # a goal closed
```

Only the user closes a goal: `origin` must be `user_said` or `user_confirmed`, and
`inferred` raises `RejectedError` with `needs_confirmation`.

`clear_valid_until=True` removes an existing end date — distinct from
`valid_until=None`, which means "leave it alone". JSON cannot express the
difference between an absent field and an explicit null, so the SDK gives it two
call shapes. `expires_at` and `clear_expiry` still work, as the older names.

### `forget(memory_id) -> None`

A soft delete: the memory stops being recalled and the audit trail keeps
what happened to it.

Only do this when the user asked. They cannot see what you removed, and a
memory deleted by mistake is gone from every future session. If a memory
is merely out of date, `save` the correction and let reconciliation
supersede it.

## Projects

### `projects() -> list[Project]` · `project(project) -> Project`

The user's projects, each with `id`, `name`, `aliases` (remotes, folder names,
former names) and `memories` (its active ones). `project()` takes an id, a name or
an alias; an unknown one raises `RejectedError` with code `unknown_project`.

### `rename_project(project, new_name) -> ProjectChange` · `merge_project(project, into) -> ProjectChange`

Renaming keeps every memory (they store the project's id) and leaves the old name as
an alias; `ConflictError` when another project has the name. Merging moves every
memory of `project` into `into`. A `ProjectChange` carries the `project` as it is
now, the server's `message`, `previous` (the old name, or the project merged away)
and `moved`.

```python
ra.rename_project("quill", "quill-app")
ra.merge_project("me-quill", into="quill-app")
```

## Topics and the review queue

### `topics() -> list[Topic]` · `create_topic(id, *, label) -> Topic` · `relabel_topic(id, label) -> Topic`

The built-in topics and the user's own, with `count`. A new topic can be used at
once; a label is display only (`None` resets it).

### `bulk_edit(memory_ids, *, kind, topic, applies_to, reviewed, add_tags, remove_tags) -> BulkResult`

One edit to many memories, all or nothing, under the save rules: a memory that
cannot take the structure raises `RejectedError` (`invalid_value`) and nothing
changes. `reviewed=True` confirms each memory's structure as it is.

### `suggest(memory_ids) -> list[Suggestion]`

What each memory (up to 25) may really be, from the model: `kind`, `topic`,
`applies_to` and a `reason`. Writes nothing. Without a model, `RejectedError`
with code `no_model`.

## Jobs

### `job(job_id) -> Job` · `wait_for_job(job, *, timeout=60, raise_on_failure=True) -> Job`

```python
finished = ra.wait_for_job(job, timeout=120)
```

`TimeoutError_` means the client gave up, **not** that the job failed —
the work is usually still running, so poll again rather than
resubmitting. `JobFailedError` means it ran out of attempts;
`raise_on_failure=False` returns the job so you can inspect `job.error`
yourself.

## Errors

Every failure is a `Recuerdos AIError` subclass, mapped from the API's
error `code` rather than its message:

| Exception | `code` | HTTP |
|---|---|---|
| `ValidationError` | `validation_failed` | 400 |
| `AuthenticationError` | `unauthorized` | 401 |
| `PermissionError_` | `forbidden` | 403 |
| `NotFoundError` | `not_found` | 404 |
| `RejectedError` | a memory rule's code (`needs_confirmation`, `conflict`, …) | 422 |
| `ConflictError` | `conflict` | 409 |
| `ServerError` | `internal` | 5xx |
| `TimeoutError_` | — | local |
| `JobFailedError` | — | local |

`PermissionError_` has the trailing underscore because `PermissionError`
is a builtin, and shadowing it in a library people star-import would be
hostile.

Every exception carries `.request_id` from the `x-request-id` header.
Quote it when reporting a problem — for a 500 the message is always the
literal `"internal error"`, and the real cause exists only in the server
log under that id.

```python
try:
    ra.get(memory_id)
except NotFoundError:
    ...
except Recuerdos AIError as error:
    print(error.code, error.status, error.request_id)
```

## LangChain

```bash
pip install "recuerdos-ai[langchain]"
```

```python
from recuerdos-ai import Client
from recuerdos-ai.langchain import Recuerdos AIRetriever

retriever = Recuerdos AIRetriever(
    client=Client(api_key="ra_live_…"),
    limit=5,
    categories=["preference.coding", "decision"],
)
docs = retriever.invoke("how should imports be structured?")
```

Each `Document` carries the memory text as `page_content`, and its id,
category, tags, score and matching legs as `metadata` — enough for a
chain to cite a memory or explain a retrieval.

**Retriever or tool?** A retriever fetches on every turn, which is right
when the memories are context the model should always have. A tool lets
the model decide when to look, which is right when memory is one source
among several. For the latter, wrap `client.search` in your framework's
tool decorator — there is nothing Recuerdos AI-specific about it, so the
SDK does not ship a second wrapper to learn.

See [`sdk/python/examples/langgraph_memory.py`](../sdk/python/examples/langgraph_memory.py)
for an agent that recalls before answering and distils after, and
[langchain.md](integrations/langchain.md) for the full recipe.

## Sync only

There is no async client. Every call is one request to a service usually
on localhost, and the work that actually takes seconds already happens
off the request path behind a job id — so a parallel async surface would
double the API for latency that mostly is not there. Inside an async
framework:

```python
hits = await asyncio.to_thread(ra.search, "dietary restrictions")
```

If that turns out to be the wrong call for real deployments, an async
client is additive rather than a breaking change.

## Compatibility

Models ignore unknown response fields, so an SDK pinned to an older
version keeps working against a newer daemon. Adding a response field is
not a breaking change.

**0.5.0 needs daemon 0.6.0 or later**: structured memories changed what a
save sends and what a refusal returns, and the projects, topics and review
calls do not exist on an older daemon. Against a 0.5.x daemon, stay on SDK
0.4.0.

## Developing

Everything runs in Docker, like the rest of the repo:

```bash
just sdk-test
```

That formats, lints (`ruff`), type-checks (`mypy --strict`), runs the
unit suite against a mock transport, then runs the integration suite
against a daemon built from `docker/Dockerfile` — the same image users
get, so the tests also prove that image serves.

Integration tests skip without `RECUERDOS_AI_TEST_URL`, so a bare
`pytest` still passes on a machine with no Docker.
