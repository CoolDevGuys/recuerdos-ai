# MCP server

Four tools and one resource, over **two transports** — stdio and HTTP.

Recuerdos AI speaks the [Model Context Protocol](https://modelcontextprotocol.io),
so any MCP client — Claude Code, opencode, Hermes Agent, MCP Inspector —
reads and writes the same memory store.

## Setup

Either way you need the daemon running and an API key:

```bash
recuerdos-ai serve &                 # leave it running
recuerdos-ai user add alex
recuerdos-ai key issue --user alex --scopes read,write
```

### Streamable HTTP (`/mcp`) — no local binary

The daemon serves MCP directly at `/mcp`, so a client connects over the
network with the key as a bearer token. This is the easiest path for a
**containerised daemon**: nothing but the daemon has to be installed, and
there is no `docker exec` wrapper.

```
opencode ──HTTP /mcp──▶ recuerdos-ai serve ──▶ SQLite
```

Point the client at `http://<daemon>:7070/mcp` with
`Authorization: Bearer ra_live_…`. It is enabled by default; turn it off
with `[server].mcp.http = false`.

The endpoint honours the MCP spec's DNS-rebinding guard, accepting only
loopback `Host` values (`localhost`, `127.0.0.1`, `::1`) — which covers a
local editor. **Reaching it over the network therefore needs a reverse
proxy that rewrites the `Host` header to a loopback value** — otherwise
`/mcp` returns `403 Forbidden: Host header is not allowed` (a direct,
proxy-less connection from another host hits exactly this). In Caddy:

```
memory.example.com {
    reverse_proxy 127.0.0.1:7070 {
        header_up Host localhost
    }
}
```

Bind the daemon itself to `127.0.0.1` (not `0.0.0.0`) so only the proxy is
exposed, and it terminates TLS — the bearer token should never cross the
network in cleartext. See [deployment.md](deployment.md).

### Allowing hosts directly (proxy-less)

If you connect straight to the daemon without a proxy, list the
hostname(s) clients use in `[server].mcp.allowed_hosts` — they are added to
the loopback defaults, so local access keeps working:

```toml
[server.mcp]
allowed_hosts = ["memory.example.com"]   # or "memory.example.com:7070"
```

A single `"*"` disables the guard entirely (accepts any `Host`) — only for
a trusted private network. Either way this turns off a browser-side
protection, not a substitute for auth: keep `[auth].mode = "api-key"` on,
and put TLS in front, since a proxy-less setup sends the bearer token in
cleartext. A reverse proxy is still the recommended shape for anything
internet-facing.

### stdio (`recuerdos-ai mcp`) — a per-session shim

MCP clients can instead spawn a server process and talk to it over
stdin/stdout. That process is a **shim**: it forwards to the running
daemon over the same HTTP API.

```
Claude Code ──stdio──▶ recuerdos-ai mcp ──HTTP──▶ recuerdos-ai serve ──▶ SQLite
                       (one per session)         (one, shared)
```

| Variable | Required | Default |
|---|---|---|
| `RECUERDOS_AI_API_KEY` | yes | — |
| `RECUERDOS_AI_URL` | no | `http://127.0.0.1:7070` |

Use this when the client only supports local (stdio) MCP servers, or when
the `recuerdos-ai` binary is already on the client's machine.

See [integrations/claude-code.md](integrations/claude-code.md) and
[integrations/opencode.md](integrations/opencode.md) for exact config for
each transport.

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
RECUERDOS_AI_API_KEY=ra_live_… npx @modelcontextprotocol/inspector recuerdos-ai mcp
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
  | RECUERDOS_AI_API_KEY=ra_live_… recuerdos-ai mcp
```

## Troubleshooting

**"RECUERDOS_AI_API_KEY is not set"** — the client isn't passing the
environment. Most clients need it under an `env` key in their config
rather than inheriting your shell.

**"could not reach the Recuerdos AI daemon"** — `recuerdos-ai serve` isn't
running, or is on a different port than `RECUERDOS_AI_URL`.

**The client lists no tools** — check the client's MCP server logs. The
shim writes diagnostics to stderr; stdout is the protocol channel and
carries nothing else.

**A tool returns "not permitted"** — check the key's scopes.
`memory_save` and `memory_forget` need `write`; `memory_recall` and the
profile need `read`. `session_distill` needs `write`.

## Verifying the HTTP transport by hand

The streamable-HTTP endpoint is plain JSON-RPC over POST, so `curl` can
drive it. `initialize` returns an `Mcp-Session-Id` header you pass to
every later call:

```bash
KEY=ra_live_…
BASE=http://localhost:7070
H='-H "Authorization: Bearer '$KEY'" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream"'

# initialize — note the Mcp-Session-Id response header
curl -si -X POST $BASE/mcp $H -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"probe","version":"1"}}}'
# then, with SID=<that header>:
curl -s -X POST $BASE/mcp $H -H "Mcp-Session-Id: $SID" -d '{"jsonrpc":"2.0","method":"notifications/initialized"}'
curl -s -X POST $BASE/mcp $H -H "Mcp-Session-Id: $SID" -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
```

A wrong key gets an authentication error from the tool rather than data —
the endpoint forwards to the same REST auth as everything else.

## Not yet implemented

**Prompts.** No MCP prompts are exposed. The server's `instructions`
field carries the "read the profile first" guidance instead.
