# Custom agents (plain REST)

For any framework without a dedicated recipe. Everything here is `curl`,
so it ports to whatever HTTP client your language has.

If your framework speaks MCP, prefer that — see [mcp.md](../mcp.md). MCP
gives the model tool descriptions already written to trigger correctly,
which is most of the work. This page is for everything else.

## Setup

```bash
recuerdos-ai serve &
recuerdos-ai user add alex
recuerdos-ai key issue --user alex --scopes read,write
export RECUERDOS_AI_API_KEY=ra_live_…
export RA=http://localhost:7070
```

Every example below assumes those two variables.

## The three calls that matter

An agent needs to do three things with memory: read the standing picture
at the start, look things up during a turn, and hand back what happened.

### 1. Session start — read the profile

```bash
curl -sS "$RA/v1/profile" -H "Authorization: Bearer $RECUERDOS_AI_API_KEY"
```

Returns markdown, roughly 1500 tokens. Put it in your system prompt.

Do this unconditionally. Recall answers a question, and an agent that has
not asked one yet still needs to know the conventions it is expected to
follow.

### 2. Mid-turn — search

```bash
curl -sS -X POST "$RA/v1/memories/search" \
  -H "Authorization: Bearer $RECUERDOS_AI_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"query": "how does this user ship infrastructure?", "limit": 3}'
```

```json
{"results":[{"id":"019f8007-437b-7420-ab01-8cbfedbcb49b",
  "content":"User deploys with Terraform, never by hand",
  "category":"preference.coding","tags":["infrastructure"],"confidence":1.0,
  "created_at":"2026-07-20T14:56:27.515400587Z",
  "updated_at":"2026-07-20T14:56:27.515400587Z",
  "score":0.031544957,"matched":{"vector_rank":1,"bm25_rank":6}}],
 "took_ms":8}
```

Ask the question you actually have, in words — the semantic leg does most
of the work. Exact identifiers (`useQuery`, a ticket id) work too,
because the keyword leg matches literal tokens the vector leg blurs.

`matched` says which leg found each result, which is how you explain a
surprising ranking instead of merely distrusting it.

### 3. Turn end — hand back what happened

```bash
curl -sS -X POST "$RA/v1/memories" \
  -H "Authorization: Bearer $RECUERDOS_AI_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"content": "we moved observability to Grafana Cloud",
       "client": "my-agent"}'
```

```json
{"job_id":"019f8007-43a8-75d1-9a83-0b4cf824e388","status":"pending",
 "poll":"/v1/jobs/019f8007-43a8-75d1-9a83-0b4cf824e388"}
```

`202`, because extraction is a model call that takes seconds. Fire and
forget: your user is not waiting for it, and blocking the reply on it
would add seconds to every turn for nothing.

**Send the raw exchange, not your guess at what mattered.** Deciding what
is durable is the service's job, and it is the part that supersedes a
stale memory rather than stacking a new one on top of it. An agent that
pre-filters is doing the work twice and worse.

## Exposing memory as a tool

If you want the model to decide when to look, define one tool. This shape
works across OpenAI, Anthropic and anything that copies them:

```json
{
  "name": "memory_recall",
  "description": "Search the user's long-term memory for their preferences, past decisions, and durable facts about their projects. Call this before assuming anything about how the user works — their tooling, conventions, architecture choices. It is cheap, and being wrong about a preference they already stated is expensive. An empty result means nothing is stored on the subject, not that they have no opinion.",
  "input_schema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "The question you actually have, in plain words."
      }
    },
    "required": ["query"]
  }
}
```

The description is product surface, not boilerplate — it is the only
thing deciding whether the model calls the tool at the right moment. Two
things earn their place in it: *when* to call (before assuming), and what
an empty result means (nothing stored, not "no opinion"). Without the
second, models read an empty recall as evidence and assert it.

If you also expose a save tool, say plainly what does **not** belong in it
— transient task detail — or you will accumulate "the user asked me to fix
the login bug" forever. Copy the wording from [mcp.md](../mcp.md); it is
tuned for exactly this.

## Session end

```bash
curl -sS -X POST "$RA/v1/sessions/distill" \
  -H "Authorization: Bearer $RECUERDOS_AI_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"content": "…summary of the session…", "session_id": "s-42",
       "client": "my-agent"}'
```

```json
{"memory_ids": ["019f…", "019f…"], "distilled": 2}
```

Synchronous, unlike `/v1/memories`: a session-end hook has nowhere to put
a job id and no next turn in which to poll.

`"distilled": 0` is the ordinary outcome. Most sessions produce nothing
that stays true after they end — treat it as success, not as something to
report to the user as a failure.

This is the one endpoint that requires `[understanding].provider`. It
returns `400` without one rather than storing your transcript whole,
because that memory would be unrecallable and would cost a context window
on every match.

## Errors

Every failure has the same shape:

```json
{"error":{"code":"validation_failed","message":"query is empty"}}
```

Branch on `code` — it is the stable part. `message` is for the human
reading the log.

| `code` | HTTP | What to do |
|---|---|---|
| `validation_failed` | 400 | Fix the request; retrying it unchanged will not help |
| `unauthorized` | 401 | Key missing, malformed, unknown or revoked |
| `forbidden` | 403 | Valid key, wrong scope. `write` does not imply `read` |
| `not_found` | 404 | Gone, or someone else's — indistinguishable by design |
| `internal` | 500 | Ours. Log `x-request-id` and report it |

Every response carries `x-request-id`. For a 500 the message is always the
literal `"internal error"`, and the real cause is only in the server log,
findable by that id.

## Things worth getting right

**Don't block a reply on a save.** `/v1/memories` returns a job id
precisely so you don't have to.

**Don't poll a job you don't need.** Most agents never read the result.
Poll only when you want to show the user what was remembered.

**One key per user.** A key *is* a user; there is no user parameter
anywhere in the API. Multi-tenant callers hold one key per tenant.

**Failing to remember is not failing to answer.** Wrap memory calls so a
daemon that is down degrades the turn instead of dropping it.

## Next

- [api.md](../api.md) — every endpoint, every field
- [Python SDK](../sdk-python.md) — if you are in Python, this is less work
- [mcp.md](../mcp.md) — if your framework speaks MCP
