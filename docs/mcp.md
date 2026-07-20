# MCP server

**Status: Phase 4.** Three tools and one resource, over stdio.

RecordAgent speaks the [Model Context Protocol](https://modelcontextprotocol.io),
so any MCP client — Claude Code, opencode, Hermes Agent, MCP Inspector —
reads and writes the same memory store.

## Setup

MCP clients spawn a server process and talk to it over stdin/stdout. That
process is a **shim**: it forwards to a running RecordAgent daemon.

```
Claude Code ──stdio──▶ recordagent mcp ──HTTP──▶ recordagent serve ──▶ SQLite
                       (one per session)         (one, shared)
```

So you need two things: the daemon running, and an API key in the
client's config.

```bash
# 1. the daemon (leave it running)
recordagent serve

# 2. a key for this client
recordagent user add alex
recordagent key issue --user alex --scopes read,write
```

Then point the client at `recordagent mcp` with that key in its
environment:

| Variable | Required | Default |
|---|---|---|
| `RECORDAGENT_API_KEY` | yes | — |
| `RECORDAGENT_URL` | no | `http://127.0.0.1:7070` |

See [integrations/claude-code.md](integrations/claude-code.md) and
[integrations/opencode.md](integrations/opencode.md) for exact config.

### Why a shim rather than a self-contained server

An editor with four windows open spawns four MCP server processes. If
each ran the engine, that would be four copies of the embedding model
resident (~130 MB each) and four processes writing one SQLite file. One
daemon with many thin clients is the arrangement that stays correct.

It also means the MCP path has no privileged back door: the shim
authenticates with an ordinary API key and is subject to the same
per-user scoping as any REST client.

If the daemon isn't reachable the shim **fails its handshake** rather
than starting and erroring on every call — a client reports "server
failed to start" far more clearly than it reports a server that looks
healthy and refuses all work.

## Tools

### `memory_save`

Stores a durable fact, preference, or decision.

```json
{"content": "User forbids barrel files and index.ts re-exports",
 "category": "preference.coding", "tags": ["typescript"]}
```

```
Saved as [preference.coding] (id 019f7c5a-…): User forbids barrel files and index.ts re-exports
It will be available in future sessions.
```

`category` is optional; see the [taxonomy](api.md#categories). It is a
hint rather than an instruction — with a provider configured, extraction
may find several memories in one submission and they need not all share it.

**What the tool actually does depends on `[understanding].provider`.**
With one configured, `memory_save` runs the full pipeline synchronously
(it uses `wait: true` on [`POST /v1/memories`](api.md#post-v1memories--ingest-raw-content)),
which means it can answer with more than one memory:

```
Saved 2 memories:

- [fact.project] The backend runs on Hetzner (id 019f7c61-…)
- [preference.coding] User requires table-driven tests in Go (id 019f7c62-…)

They will be available in future sessions.
```

…or with none:

```
Nothing new was stored — either this is already known, or there was nothing
in it that stays true beyond this conversation. Do not tell the user it was
saved.
```

That last one is why the tool does not simply return "saved". A model told
"saved" after a no-op goes on to tell the user something untrue, and the
user only finds out much later when the memory isn't there.

It is also where reconciliation earns its keep: if what the agent saves
contradicts something already stored, the old memory is superseded rather
than kept alongside it. Without a provider the tool stores content
verbatim and this section reduces to the single-memory case.

### `memory_recall`

Hybrid search — semantic and keyword — over this user's memories.

```json
{"query": "how should I structure my typescript imports?", "limit": 3}
```

```
1. [preference.coding] User forbids barrel files and index.ts re-exports (saved 2026-06-02, score 0.03, typescript)
```

Results are numbered plain text, not JSON: tool output is spent from the
agent's context window, and JSON would spend a chunk of it on
punctuation. The category comes first because it is the strongest signal
for how much weight a line deserves — a `preference.coding` is an
instruction, a `fact.project` is background.

An empty result says so explicitly rather than returning nothing: a blank
string reads to a model as a failure rather than as an answer.

### `memory_forget`

Two-step, and the server enforces it.

```json
{"query": "deployment target"}
```

```
Nothing has been deleted yet. These memories match:

- 019f7c5a-… — [fact.project] the backend deploys on flyio

To delete, call memory_forget again with the ids you want removed and
confirm: true. Only do this if the user asked for it.
```

Passing `ids` **without** `confirm: true` still deletes nothing. This is
deliberate: deletion is the only irreversible thing an agent can do here,
the user cannot see what was removed, and a memory deleted by mistake is
gone from every future session.

If a memory is merely out of date, save the corrected version instead.

## Tool: `session_distill`

Reduces a finished session to the few things worth carrying into the next
one. Call it at the end of a working session, or when the conversation is
about to be compacted.

```json
{"content": "…a summary of what happened in this session…",
 "session_id": "s-42"}
```

```
Distilled 2 memories from this session:

- [experience] Session tokens expired early because the refresh timer used local time instead of UTC (id mem_01J…)
- [preference.coding] The team writes table-driven tests for new Go packages (id mem_01K…)

They will be available in future sessions.
```

Pass what actually happened — do not pre-filter to the "important" parts.
Everything about the task itself is discarded on purpose: what was being
built, what got done, what is still failing. What survives is conventions
established, decisions and their reasons, durable facts about the system,
and root causes worth not rediscovering.

**Returning nothing is the normal outcome** and is reported as a success.
Most sessions produce no durable memories.

This is not a replacement for `memory_save`. Save a preference the moment
the user states it rather than batching it up for the end of the session.

Needs a provider: with `[understanding].provider = "none"` the tool
returns an error naming the setting, rather than storing the transcript
whole.

## Resource: `memory://profile`

A markdown briefing on the user, read at the start of a session. It
exists because recall answers a question, and an agent that hasn't asked
one yet still needs to know you forbid barrel files.

```markdown
# Memory profile: alex (updated 2026-07-20)

## How they work

- Uses pnpm; never npm or yarn
- No barrel files or index.ts re-exports
- SQLite over Postgres, chosen for installer size

## About them

- Vegetarian
```

With a provider configured, this is **written by a model** from the
user's memories and cached until those memories change — so it
compresses ("uses pnpm, Vitest and Biome" in one line) rather than
listing forty preferences and truncating at eight. Regeneration happens
per half: saving a coding preference does not rewrite the personal
section.

Without a provider it is **assembled** instead: the highest-value
memories per category, grouped under category headings. Same route, same
media type, same shape — a client cannot tell which it got, and does not
need to.

Capped at roughly 1500 tokens either way. Whatever it returns is rent
paid on every session forever, which is why it is bounded rather than
growing with the corpus.

The same digest is available over REST at `GET /v1/profile`.

## Verifying with MCP Inspector

```bash
RECORDAGENT_API_KEY=ra_live_… npx @modelcontextprotocol/inspector recordagent mcp
```

Inspector lists the tools, shows their schemas, and lets you call them by
hand. It is the fastest way to tell a config problem from a server
problem.

You can also drive the protocol directly, which is what the test suite
does:

```bash
printf '%s\n%s\n%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"probe","version":"1"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | RECORDAGENT_API_KEY=ra_live_… recordagent mcp
```

## Troubleshooting

**"RECORDAGENT_API_KEY is not set"** — the client isn't passing the
environment. Most clients need it under an `env` key in their config
rather than inheriting your shell.

**"could not reach the RecordAgent daemon"** — `recordagent serve` isn't
running, or is on a different port than `RECORDAGENT_URL`.

**The client lists no tools** — check the client's MCP server logs. The
shim writes diagnostics to stderr; stdout is the protocol channel and
carries nothing else.

**A tool returns "not permitted"** — check the key's scopes.
`memory_save` and `memory_forget` need `write`; `memory_recall` and the
profile need `read`. `session_distill` needs `write`.

## Not yet implemented

**Streamable HTTP (`/mcp` on the daemon).** Only stdio ships today.
rmcp's session factory cannot see request headers, so per-user
authentication needs a per-call path that would complicate keeping the
tool definitions identical across transports — and no target client
requires it: Claude Code, opencode and Hermes all speak stdio. The
`MemoryToolbox` trait exists so adding it later is one new
implementation rather than a second copy of the tools.

**Prompts.** No MCP prompts are exposed. The server's `instructions`
field carries the "read the profile first" guidance instead.

**`session_distill`.** Submitting a whole transcript for distillation is
Phase 5 work. Until then, a session-end hook can POST the summary to
[`POST /v1/memories`](api.md#post-v1memories--ingest-raw-content), which
runs the same extraction pipeline over it.
