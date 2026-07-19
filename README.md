# RecordAgent

[![CI](https://github.com/alexromer0/recordagent/actions/workflows/ci.yml/badge.svg)](https://github.com/alexromer0/recordagent/actions/workflows/ci.yml)

> Currently hosted at `alexromer0/recordagent` (private). The project name
> itself isn't finalized — see project-plan.md §16 — so this may move to
> its own org/repo before public launch.

Every AI agent session starts as a blank slate. You re-explain your
architecture preferences to Claude Code, your Hermes agent forgets your
dietary restrictions, and memory saved by one tool is invisible to the
others — your coding assistant learning you prefer `pnpm` doesn't help
your life-assistant bot.

RecordAgent's thesis: memory should be a *service* you own — a single fast
daemon that any agent can read from and write to over REST or MCP, that
*understands* what it stores (extracting facts, labeling, categorizing,
deduplicating, resolving contradictions), and that isolates each user's
memories strictly. See [project-plan.md](project-plan.md) for the full
design and [implementation-plan.md](implementation-plan.md) for the phased
build plan.

> **Status: Phase 2 — Memories.** Store memories and search them
> semantically *and* lexically, fully offline, strictly per user. The LLM
> understanding pipeline (extraction, labelling, contradiction handling)
> arrives in Phase 4; today a memory is stored as given.

## Prerequisites

**Docker only.** All development happens in containers; you don't need a
local Rust toolchain.

## Quickstart

```bash
git clone <repo-url> recordagent && cd recordagent
just dev
```

This builds the dev image (matching your host UID/GID so bind-mounted files
stay yours, not root's), starts the daemon with auto-rebuild on file
change, and publishes it on `localhost:7070`.

```bash
curl localhost:7070/healthz   # {"status":"ok"}
curl localhost:7070/version   # {"version":"0.1.0","git_sha":"..."}
```

Edit any file under `src/`; `cargo-watch` rebuilds and restarts the daemon
automatically.

### Create a user and an API key

Every `/v1` route requires a key. Keys are issued from the CLI — there is
deliberately no HTTP endpoint that hands them out.

With `just dev` still running, in a second terminal:

```bash
alias ra='docker compose run --rm dev cargo run -q --bin recordagent --'

ra user add alex --email alex@example.com
ra key issue --user alex --scopes read,write --name laptop
```

(The CLI runs in its own container but shares the daemon's database
through the `data` volume, so a key issued here works against the server
already running.)

```
API key created for alex (name: laptop, scopes: read,write)

  ra_live_b99f884ae92dd2318af8929b09018970a53acc6c

This is the only time this key is shown. Store it now.
```

Only a hash of the key is stored, so a lost key can be replaced but never
recovered. Use it as a bearer token:

```bash
curl -H "Authorization: Bearer ra_live_..." localhost:7070/v1/ping
# {"user":"alex","scopes":["read","write"]}

curl localhost:7070/v1/ping
# 401 {"error":{"code":"unauthorized","message":"invalid API key"}}
```

### Save and recall

```bash
curl -X POST localhost:7070/v1/memories:direct \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' -d '{
    "content": "User forbids barrel files / index.ts re-exports",
    "category": "preference.coding", "tags": ["typescript"]
  }'

curl -X POST localhost:7070/v1/memories/search \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"query": "how should I structure my typescript imports?"}'
```

```json
{"results": [{"content": "User forbids barrel files / index.ts re-exports",
  "category": "preference.coding", "score": 0.0325,
  "matched": {"vector_rank": 1, "bm25_rank": 2}}], "took_ms": 9}
```

The question shares almost no words with the memory — that's the vector
leg. Ask for `useQuery` and the keyword leg finds the literal token a
vector would blur into its neighbours. Both run on every search and their
rankings are fused.

Nothing left the machine: embeddings are computed in-process by a local
ONNX model baked into the image.

Export everything you've stored, any time:

```bash
curl "localhost:7070/v1/memories/export" -H "Authorization: Bearer $KEY"
```

See [docs/api.md](docs/api.md) for the full surface.

Other key commands:

```bash
ra key list --user alex      # prefixes, scopes, last used, status
ra key revoke b99f884a       # revoke by prefix (the visible half)
ra user list
```

See [docs/api.md](docs/api.md) for the HTTP surface and
[docs/security.md](docs/security.md) for how isolation is enforced.

Don't have `just`? Run the underlying commands directly:
`docker compose up dev`, `docker compose run --rm dev cargo test`, etc. —
see [justfile](justfile).

## Common commands

| Command | What it does |
|---|---|
| `just dev` | Start the daemon with auto-rebuild |
| `just check` | fmt --check + clippy -D warnings + boundary script + tests, in Docker |
| `just test` | Run the test suite in Docker |
| `just fmt` | Format the code |
| `just llm` | Start the optional local Ollama profile (used from Phase 4 onward) |
| `just docker-build` | Build the release image (`docker/Dockerfile`) |

A local-toolchain contributor can run the same checks without Docker via
the `*-native` recipes (`just check-native`, `just test-native`, ...).

## Configuration

See [docs/configuration.md](docs/configuration.md).

## Status

| Phase | Scope | Status |
|---|---|---|
| 0 — Foundation | Docker dev env, config, HTTP skeleton, CI | ✅ |
| 1 — Identity | Users, API keys, per-user isolation | ✅ |
| 2 — Memories | Store + hybrid search (REST) | ✅ |
| 3 — MCP server | Claude Code / opencode integration | ⬜ |
| 4 — Understanding | Extraction, labeling, reconciliation | ⬜ |
| 5 — Consolidation | Dedup/merge, decay, profile digest | ⬜ |
| 6 — Release | SDK, docs, packaging | ⬜ |

## Architecture

Bounded contexts, each a vertical slice of `domain` / `application` /
`infrastructure` — see [docs/architecture.md](docs/architecture.md) and
[implementation-plan.md §2](implementation-plan.md#2-architecture) for the
full boundary rules.

```
identity        users, API keys, authentication, UserContext
memories        storing, indexing, searching, exporting memories
understanding   LLM pipeline: extract → reconcile → label
providers       concrete LLM/embedding implementations
consolidation   background jobs: dedup/merge, decay, distillation, profile
shared          shared kernel: ids, error type, clock
```

The target end-state (most of this arrives in later phases — see the
status table above for what's real today):

```
                                ┌────────────────────────────────────────────┐
  Claude Code ── MCP(stdio) ──▶ │                RECORDAGENT DAEMON          │
  opencode ──── MCP(http) ────▶ │  ┌──────────┐  ┌──────────────────────┐    │
  Hermes ────── REST ─────────▶ │  │ API layer│  │  Memory Engine       │    │
  LangChain ─── REST/SDK ─────▶ │  │ axum+rmcp│─▶│  ingest → understand │    │
                                │  │ auth mw  │  │  → label → store     │    │
                                │  └──────────┘  │  retrieve: hybrid    │    │
                                │        │       │  (vector+BM25+filter)│    │
                                │        ▼       └──────┬───────────────┘    │
                                │  ┌──────────┐         │   ┌─────────────┐  │
                                │  │ Job queue│◀────────┘   │ Provider hub│  │
                                │  │ (async   │             │ anthropic / │  │
                                │  │  ingest, │────────────▶│ openai-compat│ │
                                │  │  consol.)│             │ ollama /    │  │
                                │  └──────────┘             │ onnx-local  │  │
                                │        │                  └─────────────┘  │
                                │        ▼                                   │
                                │  MemoryStore trait                         │
                                │  ├─ embedded: SQLite + sqlite-vec + tantivy│
                                │  └─ scale:    Postgres+pgvector | Qdrant   │
                                └────────────────────────────────────────────┘
```

## Contributing

See [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) (Docker-only dev flow,
boundary rules).

## License

Apache-2.0.
