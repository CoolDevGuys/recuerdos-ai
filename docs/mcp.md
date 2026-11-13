# MCP server

Nine tools and one resource, over **two transports** — stdio and HTTP. Both serve the
same tools; they differ in whether the server knows **which project you are in**.

Recuerdos AI speaks the [Model Context Protocol](https://modelcontextprotocol.io),
so any MCP client — Claude Code, opencode, Hermes Agent, MCP Inspector —
reads and writes the same memory store.

## Setup

Either way you need the daemon running somewhere (this machine or a server) and an
API key:

```bash
recuerdos-ai serve &                 # leave it running
recuerdos-ai user add alex
recuerdos-ai key issue --user alex --scopes read,write
```

### Which transport: stdio or HTTP

Memories that apply to one project need the server to know the project. The stdio
shim learns it from the folder your harness runs in; over HTTP there is no folder.

| | **stdio** (`recuerdos-ai mcp`) | **HTTP** (`/mcp`) |
|---|---|---|
| On the client machine | the `recuerdos-ai` binary, run per session by the harness (no daemon: it forwards to `RECUERDOS_AI_URL`, which can be another server) | nothing |
| Tools and `memory://profile` | all | all |
| **Current project** | **detected** from the folder's git `origin` (every clone and worktree is one project), or `RECUERDOS_AI_PROJECT` | **none**, unless the client sends `X-Recuerdos-Project` (and optionally `X-Recuerdos-Project-Remote`) headers, set per project in its MCP config |
| Saving "in this repo we…" | filed under the project; a new project is confirmed once | refused `no_project` unless the headers are set or the agent names `applies_to.project_name` |
| Recall | the current project ranks with your own memories; other projects lower | with no project, every project ranks lower than your own memories (pass `project` to `memory_recall` to say where you are) |
| Profile | yours, this project's, general | yours and general (the headers add the project) |
| A repository renamed on its host | the refusal offers `project_rename`, which learns the new remote | only when the remote header is sent |
| Best for | coding harnesses working in a repository (Claude Code, opencode) | chat assistants with no project folder (Hermes, Open WebUI), and clients that cannot run a local process |

**For coding, use stdio — even when the daemon is on a server.** Install only the
binary on your computer and point it at the server; nothing else runs locally. See
[On a new computer](#on-a-new-computer).

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

**Telling it the project.** HTTP has no folder to detect a project from. For a client
you configure per project (a project's `.mcp.json`, for instance), add the project
headers next to the key, and that client is in that project exactly as the stdio shim
would be:

```json
{
  "mcpServers": {
    "recuerdos-ai": {
      "type": "http",
      "url": "https://memory.example.com/mcp",
      "headers": {
        "Authorization": "Bearer ra_live_…",
        "X-Recuerdos-Project": "quill",
        "X-Recuerdos-Project-Remote": "github.com/acme/quill"
      }
    }
  }
}
```

The remote is optional; with it, a repository renamed later is recognised as the same
project. Without the headers the session is in no project, which is right for a chat
assistant.

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
| `RECUERDOS_AI_URL` | no | `http://127.0.0.1:7070` — set it to your server's address when the daemon runs elsewhere |
| `RECUERDOS_AI_PROJECT` | no | detected: the folder's git `origin`, named after its last segment, else the repository's folder name; outside a git repository, no project |

The shim is the only transport that knows the project for you, so it is the one to
use in a coding harness. It needs the binary on the client machine, not the daemon:
`RECUERDOS_AI_URL` can point at a server on your network. Keep the binary at the same
version as the daemon.

#### On a new computer

No checkout of the repository is needed. Each release publishes the binary for Linux
(x86_64, ARM64) and Apple Silicon macOS, and `install.sh` downloads it and checks its
checksum:

```bash
curl -fsSL https://raw.githubusercontent.com/CoolDevGuys/recuerdos-ai/main/install.sh | sh
```

(`RECUERDOS_AI_VERSION=v0.6.0` before `sh` pins a version, matching your server's.)
Then configure the harness to run it against your server, for example in
`~/.claude.json`:

```json
{
  "mcpServers": {
    "recuerdos-ai": {
      "command": "recuerdos-ai",
      "args": ["mcp", "--client", "claude-code"],
      "env": {
        "RECUERDOS_AI_URL": "https://memory.example.com",
        "RECUERDOS_AI_API_KEY": "ra_live_…"
      }
    }
  }
}
```

The shim talks to the daemon's REST API, not to `/mcp`, so the `Host` guard below does
not apply to it; put TLS in front of the daemon all the same. On Windows or an Intel
Mac there is no release binary yet: use HTTP with the project headers, or build from
source.

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

Stores one memory with its structure: what it is, what it is about, who it is
true for, and where it came from. Every field is explained in
[memory-model.md](memory-model.md); the tool's own description carries the same
guidance, so an agent reads it on every call.

```json
{"content": "User forbids barrel files and index.ts re-exports",
 "kind": "preference", "topic": "coding",
 "applies_to": {"target": "me"}, "origin": "user_said",
 "tags": ["typescript"]}
```

```
Saved as your personal coding preference (applies to you everywhere). (id 019f7c5a-…): User forbids barrel files and index.ts re-exports
It will be available in future sessions.
```

The reply says how it was filed, so the agent can repeat it and you can correct
it on the spot. `applies_to` is what keeps one project's rule out of your
preferences, so an agent should choose it from what you said, and ask when it is
unclear. Left out, it defaults to the current project, else `general`, never
`me`, and the reply asks the agent to confirm it.

**Inside a project.** `recuerdos-ai mcp` runs in the folder your harness works in
and sends that folder's project (from its git remote) with every call, so "in this
repo we never mock the database" is filed under it without the agent naming it.
The first save to a project the daemon does not know is refused until you confirm
it is new; the agent then saves again with `"new_project": true`.

**When a rule refuses the save**, the tool answers with a result beginning
`Not saved (code):` and says what to change: ask you before saving an inferred
preference, use `convention` for a project's way of doing things, name the
project, leave the secret out. An agent should follow it, usually by asking you,
rather than rewording to get past it. The codes are listed in
[memory-model.md](memory-model.md#when-a-save-is-refused).

**When it contradicts a stored memory** for the same scope, the save is refused as
`Not saved (conflict):`, naming the stored memory and its id. The agent asks you
which holds, then:

- you changed your mind: it saves again with `"replaces": "<id>"`, and the old one
  leaves recall but stays in history;
- the stored one was wrong: it fixes it with `memory_update`;
- both hold: it saves again with `"coexists_with": ["<id>"]`.

With a language model configured, the model judges the contradiction, and also
whether text filed as yours or as general is really about one project. Without
one, only a near repeat of a stored memory is stopped. Saying something the store
already knows stores nothing, and the reply says `Already known: …`.

A project memory that contradicts one of your own is not a conflict: a project may
make an exception, and the reply says it overrides your preference there.

`memory_save` always stores as given. Raw text for the service to split and
classify goes through [`POST /v1/memories`](api.md#post-v1memories--ingest-raw-content)
or `session_distill`.

### `memory_save_batch`

Several memories in one call, each with its own structure, exactly as
`memory_save` takes one. Items are saved one by one through the same rules, so a
refused item does not stop the rest, and the reply reports each:

```
Saved 1 memory from 2 items.

- Saved as a coding rule of quill. In this repo migrations are forward-only (id 019f…)

They will be available in future sessions.

1 item was not saved:
- item 1: not saved (needs_confirmation): This is about the user, and it was inferred. …
```

An item the store already knew is listed under "added nothing new".

### `memory_update`

Amends one stored memory: its wording, its structure, or whether a goal is still
open. It keeps its id, and the audit trail keeps what it said before. The same
rules as a save apply to what it becomes; a refusal begins `Not updated (code):`.

```json
{"id": "019f7c5a-…", "content": "The user prefers mocks in unit tests"}
```

```
Updated; filed as your personal coding preference. (id 019f7c5a-…): The user prefers mocks in unit tests
```

Every field but `id` is optional: `content`, `kind`, `topic`, `applies_to` (to move
it to another scope), `subtopic` and `valid_until` (an empty string clears them),
`tags`, `goal_status` and `origin`.

**Closing a goal** is `{"id": …, "goal_status": "achieved", "origin":
"user_confirmed"}` (or `"abandoned"`, or `"user_said"` when you said so yourself).
Only you close a goal: an agent that only noticed it was reached is refused with
`needs_confirmation` and should ask you first. A closed goal leaves recall and the
profile, and stays in history.

Amend when the stored memory is wrong; when you changed your mind, the new memory
is saved with `replaces`, so the old one remains as what you used to think.

### `project_list`, `project_rename`, `project_merge`

The user's projects ([memory-model.md](memory-model.md#projects)).

- **`project_list`** takes nothing and lists each project, its memory count and what
  else finds it:

  ```
  2 projects:
  - fern — 4 memories; also found by github.com/acme/fern
  - quill — 12 memories; also found by github.com/acme/quill, quill-old
  ```
- **`project_rename`** `{"name": "quill", "new_name": "quill-app"}`: the memories
  follow and the old name keeps finding it. Called from the stdio shim inside a
  folder whose project is the new name, that folder's remote is learned too. This is
  the answer when a save in a renamed repository is refused with *"Is it a rename of
  quill (github.com/acme/quill)?"* and the user says yes.
- **`project_merge`** `{"name": "me-quill", "into": "quill"}`: every memory of
  `me-quill` moves to `quill`, whose aliases gain `me-quill`'s name and remotes.

Each answers with one sentence (`Renamed quill to quill-app: …`, `Merged me-quill into
quill: 1 memory moved, …`); a refusal begins `Not renamed:` or `Not merged:`. The tool
descriptions tell the agent to rename or merge only when the user agrees.

### `memory_recall`

Hybrid search — semantic, keyword, and a graph hop over the entity/relation
graph — over this user's memories.

```json
{"query": "how should I structure my typescript imports?", "limit": 3}
```

```
1. [preference · coding · me] User forbids barrel files and index.ts re-exports (saved 2026-06-02, relevance 0.71, typescript)
2. [convention · coding · quill] Use cases are named by intent, no Service suffix (saved 2026-09-14, relevance 0.48)
```

Results are numbered plain text, not JSON: tool output is spent from the
agent's context window, and JSON would spend a chunk of it on
punctuation. The scope comes first because it is the strongest signal
for how much weight a line deserves — your `preference` is an instruction
everywhere, a project's `convention` only in that project.

Optional filters: `kinds`, `topics`, `targets` (`me`, `project`, `general`) and
`projects`, each a list. The stdio shim's folder is the project the recall ranks with
your own memories; other projects' rank lower unless the question names them. Over
HTTP MCP, which has no folder, pass `project`. See
[memory-model.md](memory-model.md#recall-and-the-profile-know-where-you-are).

The graph leg is what lets a question reach a memory it shares no words
with: "who owns the billing service's data?" surfaces the memory naming
`ledger-db` by walking `billing service → ledger-db`, even though that
memory never says "billing". It runs when the graph is on (the default)
and the query names a known entity; otherwise recall is exactly its
semantic + keyword self. An optional `as_of` argument (RFC 3339) reads
those connections as of a past instant — "who was on-call before the
reorg?" — and affects only the graph-connected results.

An empty result says so explicitly rather than returning nothing: a blank
string reads to a model as a failure rather than as an answer.

One thing this text deliberately does not carry is whether an enabled second
stage actually reordered the list. The reranker's outcome is an operator fact
rather than the agent's business, so it lives in the REST response as a top-level
`rerank` field ([api.md](api.md)) and in the daemon's log. If you set
`[rerank].mode = "on"` and want to know whether it is contributing, look there —
[deployment.md](deployment.md#4-confirm-it-is-actually-running) has the one
request that answers it.

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

## About you (everywhere)

- Uses pnpm; never npm or yarn
- No barrel files or index.ts re-exports

## This project: quill

- Use cases are named by intent, no Service suffix
- SQLite over Postgres, chosen for installer size

## Your world (not tied to a project)

- NAS backups are in /mnt/tank/backups
```

The project part is the one the stdio shim's folder is in; read without one (over
HTTP MCP, or `GET /v1/profile` with no `?project=`), the profile has no project part,
and another project's memories are never in it.

With a provider configured, this is **written by a model** from the
user's memories and cached until those memories change — so it
compresses ("uses pnpm, Vitest and Biome" in one line) rather than
listing forty preferences and truncating at eight. There is one cached
digest per scope — yours, the general one, one per project — so saving a
project convention regenerates only that project's.

Without a provider it is **assembled** instead: the highest-value
memories per kind within each part, preferences, rules and conventions
first. Same route, same media type — a client cannot tell which it got,
and does not need to.

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
