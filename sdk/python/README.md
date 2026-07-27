# recordagent

Python client for [RecordAgent](https://github.com/alexromer0/recordagent) —
a self-hostable long-term memory service for AI agents, over REST and MCP.

```bash
pip install recordagent
```

You need a running daemon. The fastest way:

```bash
docker run -p 7070:7070 -v recordagent-data:/data \
  -e RECORDAGENT_AUTH__MODE=none ghcr.io/alexromer0/recordagent
```

## The loop

```python
from recordagent import Client

ra = Client(base_url="http://localhost:7070", api_key="ra_live_…")

# Raw text in. The service decides what is worth remembering, splits it
# into atomic memories, and supersedes anything they contradict.
ra.save("btw we moved the backend to Hetzner, fly.io got too expensive. "
        "Also always write table-driven tests in Go")

# Ask the question you actually have.
for hit in ra.search("where do we deploy?"):
    print(hit.content)   # "The backend runs on Hetzner (migrated from Fly.io …)"
```

`save` returns a job, because extraction is a model call that takes
seconds:

```python
job = ra.save("User is vegetarian now")
finished = ra.wait_for_job(job)
print(finished.memory_ids)

# Or, for scripts:
memories = ra.save_and_wait("User is vegetarian now")
```

## When you already know what to remember

`save_direct` stores one memory verbatim, with no model in the loop. It
is cheap and synchronous — but nothing read the content, so it cannot
supersede a contradiction. Prefer `save` unless you have already
distilled the memory yourself.

```python
ra.save_direct(
    "User forbids barrel files / index.ts re-exports",
    category="preference.coding",
    tags=["typescript", "imports"],
)
```

## Session distillation

At the end of a working session, hand over what happened and keep only
what outlives it:

```python
result = ra.distill_session(transcript, session_id="s-42", client="my-agent")
print(f"{result.distilled} memories survived the session")
```

`distilled == 0` is the ordinary outcome. Most sessions produce nothing
that stays true after they end.

## Session start

```python
print(ra.profile())   # markdown, ~1500 tokens
```

Read it before the first turn. Recall answers a question; an agent that
has not asked one yet still needs to know the conventions it is expected
to follow.

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

See [`examples/`](examples/) for a LangGraph agent that reads its memory
at the start of a thread and writes back at the end.

## Errors

Every failure is a `RecordAgentError` subclass, keyed off the API's
stable error code rather than its message:

```python
from recordagent import NotFoundError, PermissionError_, RecordAgentError

try:
    ra.get(memory_id)
except NotFoundError:
    ...                     # gone, or someone else's — indistinguishable by design
except PermissionError_:
    ...                     # valid key, missing scope
except RecordAgentError as error:
    print(error.request_id)  # ties this to the server log line with the real cause
```

## Notes

- **Sync only.** Every call is one request to a service usually on
  localhost; the work that takes seconds already happens off the request
  path behind a job id. Inside an async framework, use
  `asyncio.to_thread`.
- **A key is a user.** There is no user parameter anywhere — two users
  means two clients.
- **Unknown response fields are ignored**, so an older SDK keeps working
  against a newer daemon.

Full reference: [docs/sdk-python.md](https://github.com/alexromer0/recordagent/blob/main/docs/sdk-python.md).

Apache-2.0.
