# Hermes Agent integration

[Hermes Agent](https://hermes-agent.nousresearch.com/docs/) is a personal
assistant rather than a coding agent, which changes what memory is *for*.
A coding agent needs your conventions. A life agent needs to know you are
vegetarian now, who your dentist is, and that Friday is your mother's
birthday — facts that change, and whose stale versions are worse than
useless.

Two ways to connect, and MCP is the better one.

> **Note on this page.** Recuerdos AI's side of both routes is verified —
> the MCP server, the endpoints and the payloads below are exercised by
> the test suite. The Hermes-side configuration syntax is not: check it
> against [their docs](https://hermes-agent.nousresearch.com/docs/), which
> are authoritative and move faster than this file.

## Option 1 — MCP (recommended)

Recuerdos AI is a standard stdio MCP server. If Hermes can spawn one, this
is the whole integration:

```json
{
  "mcpServers": {
    "recuerdos-ai": {
      "command": "recuerdos-ai",
      "args": ["mcp", "--client", "hermes"],
      "env": {
        "RECUERDOS_AI_API_KEY": "ra_live_…",
        "RECUERDOS_AI_URL": "http://127.0.0.1:7070"
      }
    }
  }
}
```

`--client hermes` is recorded as the source of every memory saved through
it, so the audit trail can tell a memory Hermes wrote from one Claude Code
wrote.

This is better than the REST route because the tool descriptions come with
it, and those descriptions are the thing that decides whether the model
calls the tool at the right moment. They took real tuning — see
[mcp.md](../mcp.md).

The daemon must already be running (`recuerdos-ai serve`); the stdio
process is a thin shim that forwards to it. One daemon, many agents, is
the shape that makes cross-tool memory work at all.

If Hermes runs on a different machine from the daemon — likely, if the
daemon is on a server and Hermes is on your phone — the stdio shim does
not fit (it needs the binary locally). Point Hermes at the daemon's HTTP
MCP endpoint instead, if it supports remote MCP servers: `https://<your
daemon>/mcp` with an `Authorization: Bearer ra_live_…` header. Behind TLS,
the bearer token is what guards it. See [mcp.md](../mcp.md#setup).

## Option 2 — REST tools

If tool definitions are easier to hand-write in your Hermes setup, define
two. The shape below is the generic OpenAI/Anthropic function schema most
frameworks accept.

### Recall

```json
{
  "name": "memory_recall",
  "description": "Search the user's long-term memory for personal facts, preferences, people, routines, and past decisions. Call this before assuming anything about the user — their diet, their schedule, who they know, what they have already told you. It is cheap, and being wrong about something they already said is expensive. An empty result means nothing is stored on the subject, not that the user has no preference.",
  "parameters": {
    "type": "object",
    "properties": {
      "query": {"type": "string", "description": "The question you actually have, in plain words."},
      "categories": {
        "type": "array",
        "items": {"type": "string"},
        "description": "Optional filter, e.g. preference.personal, fact.person."
      }
    },
    "required": ["query"]
  }
}
```

Backed by:

```bash
curl -sS -X POST "$RA/v1/memories/search" \
  -H "Authorization: Bearer $RECUERDOS_AI_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"query": "dietary restrictions", "limit": 5}'
```

### Save

```json
{
  "name": "memory_save",
  "description": "Store something the user said that stays true after this conversation: a preference ('I'm vegetarian now'), a durable fact about them or the people in their life, a decision, a routine. Do NOT store what they asked you to do right now, or transient state — those are true for ten minutes and storing them buries what matters.",
  "parameters": {
    "type": "object",
    "properties": {
      "content": {"type": "string", "description": "What the user said, in their own words where possible."}
    },
    "required": ["content"]
  }
}
```

Backed by:

```bash
curl -sS -X POST "$RA/v1/memories" \
  -H "Authorization: Bearer $RECUERDOS_AI_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"content": "I am vegetarian now", "client": "hermes"}'
```

Returns `202` with a job id. Do not block the reply on it — the user is
waiting for an answer, not for their memory to be filed.

**Send what the user said, not your summary of it.** `/v1/memories` runs
extraction and reconciliation: it decides what is durable, labels it, and
supersedes what it contradicts. A tool that pre-summarises loses the
supersession, which is the entire point — see below.

### Session start

Fetch the profile once and prepend it to the system prompt:

```bash
curl -sS "$RA/v1/profile" -H "Authorization: Bearer $RECUERDOS_AI_API_KEY"
```

## Why supersession matters more here than anywhere else

The project's own scenario:

> Stored 2024: *"User loves steakhouses."*
> Today, over Telegram: *"book dinner Friday — remember I'm vegetarian now."*

Posted to `/v1/memories`, the pipeline classifies this as an UPDATE: the
steakhouse memory is superseded — retained in the audit trail, gone from
recall — and the vegetarian fact becomes the single current answer to
"what does this person eat?".

A store without that step returns both, and the agent books a steakhouse
for a vegetarian. This is the difference between memory that stays *true*
and memory that merely grows, and a life agent is where it bites hardest,
because personal facts change more often than coding conventions do.

Supersession needs `[understanding].provider` configured. Without it,
content is stored verbatim and contradictions sit side by side — see
[configuration.md](../configuration.md).

## Categories worth knowing

The taxonomy leans coding by default, but two categories carry most of a
life agent's weight:

| category | for |
|---|---|
| `preference.personal` | diet, routines, how they like things done |
| `fact.person` | people in their life and what matters about them |
| `experience` | what was tried and what happened |
| `reference` | lookups — account details, addresses, links |

Filter on them to keep a dinner-booking prompt free of the user's
TypeScript opinions:

```bash
-d '{"query": "dietary restrictions", "categories": ["preference.personal"]}'
```

Add your own with `[understanding.taxonomy].extra_categories`.

## Sharing memory with your coding agents

Point Hermes and Claude Code at the same daemon with **the same API key**
and they share one memory. A key *is* a user; two keys for the same person
means two separate memories that never see each other.

That is the cross-tool case the project exists for: a preference stated to
Hermes on your phone is known to Claude Code on your laptop, without
either of them knowing the other exists.

## Next

- [mcp.md](../mcp.md) — the MCP tools and the `memory://profile` resource
- [custom-agents.md](custom-agents.md) — the generic REST recipe, in more depth
- [api.md](../api.md) — every endpoint and field
