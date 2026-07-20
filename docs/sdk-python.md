# Python SDK

A thin typed client over the [REST API](api.md). Source lives in
[`sdk/python`](../sdk/python).

```bash
pip install recordagent
```

```python
from recordagent import Client

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
| `api_key` | From `recordagent key issue`. Optional only because `[auth].mode = "none"` accepts unauthenticated requests. |
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

### `save_direct(content, *, category, tags, confidence, expires_at, …) -> Memory`

Stores one memory verbatim. Synchronous, cheap, no model.

It also cannot supersede a contradiction, because nothing read the
content. Use it when you have already decided what the memory should say;
prefer `save` for raw material.

```python
ra.save_direct(
    "User forbids barrel files / index.ts re-exports",
    category="preference.coding",
    tags=["typescript", "imports"],
)
```

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

### `search(query, *, limit, categories, tags, since, include_superseded) -> list[SearchHit]`

Hybrid recall — semantic and keyword legs fused by reciprocal rank.

```python
hits = ra.search("which package manager?", limit=3, categories=["preference.coding"])
for hit in hits:
    print(hit.score, hit.content, hit.matched.vector_rank, hit.matched.bm25_rank)
```

Ask the question you actually have rather than keywords. Exact
identifiers (`useQuery`, a ticket id) work too — that is the keyword
leg's reason for existing.

`categories` are OR-ed; `tags` are **AND**-ed. `hit.matched` says which
leg found the result, so a surprising ranking can be explained rather
than merely distrusted.

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

### `update(memory_id, *, content, category, tags, expires_at, clear_expiry) -> Memory`

Omitted fields are left alone. Editing `content` re-embeds and re-indexes.

`clear_expiry=True` removes an existing expiry — distinct from
`expires_at=None`, which means "leave it alone". JSON cannot express the
difference between an absent field and an explicit null, so the SDK gives
it two call shapes.

### `forget(memory_id) -> None`

A soft delete: the memory stops being recalled and the audit trail keeps
what happened to it.

Only do this when the user asked. They cannot see what you removed, and a
memory deleted by mistake is gone from every future session. If a memory
is merely out of date, `save` the correction and let reconciliation
supersede it.

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

Every failure is a `RecordAgentError` subclass, mapped from the API's
error `code` rather than its message:

| Exception | `code` | HTTP |
|---|---|---|
| `ValidationError` | `validation_failed` | 400 |
| `AuthenticationError` | `unauthorized` | 401 |
| `PermissionError_` | `forbidden` | 403 |
| `NotFoundError` | `not_found` | 404 |
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
except RecordAgentError as error:
    print(error.code, error.status, error.request_id)
```

## LangChain

```bash
pip install "recordagent[langchain]"
```

```python
from recordagent import Client
from recordagent.langchain import RecordAgentRetriever

retriever = RecordAgentRetriever(
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
tool decorator — there is nothing RecordAgent-specific about it, so the
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

## Developing

Everything runs in Docker, like the rest of the repo:

```bash
just sdk-test
```

That formats, lints (`ruff`), type-checks (`mypy --strict`), runs the
unit suite against a mock transport, then runs the integration suite
against a daemon built from `docker/Dockerfile` — the same image users
get, so the tests also prove that image serves.

Integration tests skip without `RECORDAGENT_TEST_URL`, so a bare
`pytest` still passes on a machine with no Docker.
