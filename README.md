# Recuerdos AI

[![CI](https://github.com/CoolDevGuys/recuerdos-ai/actions/workflows/ci.yml/badge.svg)](https://github.com/CoolDevGuys/recuerdos-ai/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

**Long-term memory for your AI agents — one service, all of them, on your
own machine.**

Every agent session starts as a blank slate. You re-explain your
architecture preferences to Claude Code. Your life-assistant bot forgets
you went vegetarian. And memory saved by one tool is invisible to the
others: your coding assistant learning you prefer `pnpm` does nothing for
anything else you run.

Recuerdos AI is one daemon that any agent reads from and writes to, over
REST or MCP. It doesn't just store what you tell it — it works out what is
worth keeping, labels it, and when you contradict yourself it *replaces*
the old answer instead of returning both.

```
                    ┌──────────────┐
Claude Code ─MCP──▶ │              │  extract → reconcile → store
opencode ────MCP──▶ │  Recuerdos AI │  recall: vector + BM25, fused
Hermes ─────REST──▶ │    daemon    │  nightly: merge, decay, expire
LangChain ──SDK───▶ │              │
                    └──────────────┘
                       SQLite + local ONNX embeddings
                       no external services required
```

## 90-second quickstart

```bash
docker run -d --name recuerdos-ai -p 7070:7070 \
  -v recuerdos-ai-data:/data \
  -e RECUERDOS_AI_AUTH__MODE=none \
  ghcr.io/cooldevguys/recuerdos-ai
```

`AUTH__MODE=none` makes every request the built-in `default` user — fine
on a laptop, and one less step for a first look. [Turn it on](docs/configuration.md#authentication)
before anything else can reach the port.

> **Deploying on a real server?** Don't use `AUTH__MODE=none` there.
> [docs/deployment.md](docs/deployment.md) is a four-step guide — run it,
> put HTTPS in front, issue a key, connect your tools — written for a
> personal memory server on a VPS.

Store something:

```bash
curl -X POST localhost:7070/v1/memories:direct -H 'Content-Type: application/json' \
  -d '{"content": "User forbids barrel files / index.ts re-exports",
       "category": "preference.coding"}'
```

Then ask a question that shares almost none of its words:

```bash
curl -X POST localhost:7070/v1/memories/search -H 'Content-Type: application/json' \
  -d '{"query": "how should I structure my typescript imports?"}'
```

```json
{"results": [{"content": "User forbids barrel files / index.ts re-exports",
  "category": "preference.coding", "score": 0.0325,
  "matched": {"vector_rank": 1, "bm25_rank": 2}}], "took_ms": 9}
```

That's the semantic leg. Search for `useQuery` and the keyword leg finds
the literal token a vector would blur into its neighbours. Both run on
every query and their rankings are fused.

**Nothing left the machine.** Embeddings are computed in-process by an
ONNX model baked into the image.

### Point Claude Code at it

```jsonc
// .mcp.json
{
  "mcpServers": {
    "recuerdos-ai": {
      "command": "recuerdos-ai",
      "args": ["mcp", "--client", "claude-code"],
      "env": { "RECUERDOS_AI_API_KEY": "ra_live_…" }
    }
  }
}
```

Then, in one session:

> Remember that I forbid barrel files — no index.ts re-exports.

And in a **new** session, in a **different** project:

> How should I structure imports here?

It recalls the preference without being told.

Recipes: [Claude Code](docs/integrations/claude-code.md) ·
[opencode](docs/integrations/opencode.md) ·
[Hermes](docs/integrations/hermes.md) ·
[LangChain](docs/integrations/langchain.md) ·
[any REST client](docs/integrations/custom-agents.md)

### Or install the binary

```bash
curl -fsSL https://raw.githubusercontent.com/CoolDevGuys/recuerdos-ai/main/install.sh | sh
recuerdos-ai init && recuerdos-ai serve
```

### Or from Python

```bash
pip install recuerdos-ai
```

```python
from recuerdos-ai import Client

ra = Client(api_key="ra_live_…")
ra.save("We moved the backend to Hetzner; fly.io got too expensive")

for hit in ra.search("where do we deploy?"):
    print(hit.content)
```

[Full SDK reference](docs/sdk-python.md).

## What it does

| | |
|---|---|
| **Hybrid recall** | Semantic (ONNX embeddings) + keyword (BM25), fused by reciprocal rank. Paraphrases *and* exact identifiers. |
| **Understands what it stores** | Raw text in, atomic labelled memories out. One sentence with two unrelated facts becomes two separately recallable memories. |
| **Resolves contradictions** | "We moved to Hetzner" supersedes "we deploy on Fly.io" — the old one is retained for audit, gone from recall. |
| **Session distillation** | Hand over a finished session; keep the two or three things that outlive it. |
| **Stays clean over time** | A nightly job merges duplicates, retires expired memories, and demotes what nobody reads. |
| **Strict per-user isolation** | Enforced by the type system, not by remembering to write `WHERE user_id`. |
| **Works offline** | Embeddings, storage and search need no external service. LLM understanding is opt-in. |
| **Yours** | SQLite on your disk. `GET /v1/memories/export` any time. Apache-2.0. |

## Performance

Measured on the release image — see [docs/performance.md](docs/performance.md)
for methodology, hardware and the raw numbers.

| Corpus | Recall p50 | Recall p95 |
|---|---|---|
| 2,000 memories | 26 ms | 34 ms |
| 20,000 memories | 24 ms | 36 ms |
| 100,000 memories | 70 ms | 83 ms |

Ingest acknowledgement is 4.5 ms at p95 and stays flat at 100k — the
pipeline runs off the request path. Cold start is ~300 ms; idle RSS is
~190 MiB, most of it the resident ONNX model.

Recall is flat from 2k to 20k, so at the scale anyone actually reaches it
is effectively instant. It rises to 83 ms at 100k, which **misses the
50 ms target** the project set for itself. That, the two other missed
targets, and the fact that the 1M run and the 24-hour soak were never
executed are all written up in [docs/performance.md](docs/performance.md)
rather than left out.

Measured on Docker Desktop / Apple Silicon, which is the pessimistic
runtime — a VM with a virtualised filesystem, for a workload dominated by
SQLite writes and ONNX inference.

## Your data stays yours

**Nothing leaves the machine by default.** `[understanding].provider`
defaults to `none`: embeddings, storage, indexing and search are all
local, and no network call happens at all. Configuring a provider is what
turns on extraction and reconciliation — an explicit, reversible choice,
and the only thing that ever sends your memories anywhere.

**Isolation is a compile-time property.** Every repository method takes a
`UserContext`, and a `UserContext` cannot be constructed outside the
`identity` context — so reaching another user's memories does not
typecheck. A grep in CI guards the constructors, and a cross-tenant test
suite has grown with every phase since Phase 1.

**Keys are argon2 hashes.** A lost key can be replaced, never recovered.
Deleting is soft by default, so "what happened to that memory?" stays
answerable in the audit trail.

Details, plus the threat model: [docs/security.md](docs/security.md).

## Configuration

Settings come from three layers, each overriding the one before:

**defaults → `recuerdos-ai.toml` → `RECUERDOS_AI_*` environment variables**

So an env var always wins over the file, and the file always wins over the
built-in defaults. [`recuerdos-ai.example.toml`](recuerdos-ai.example.toml)
lists every key with its default and a comment; copy it (or run
`recuerdos-ai init`) and change only what you need.

**The file is read only when you point at it** — with `--config PATH`, or
by setting `RECUERDOS_AI_CONFIG=PATH`. There is no automatic discovery:
`recuerdos-ai serve` on its own uses defaults + env and *ignores* a
`recuerdos-ai.toml` sitting next to it. This trips people up, so it is
worth saying plainly:

```bash
recuerdos-ai serve  --config recuerdos-ai.toml     # bare metal
recuerdos-ai config --config recuerdos-ai.toml     # print what that resolves to

# …or point the whole CLI at one file, no --config to repeat:
export RECUERDOS_AI_CONFIG=recuerdos-ai.toml
recuerdos-ai serve
recuerdos-ai reindex
```

`recuerdos-ai config` (or `make config`) prints the **effective**
configuration — which embeddings and understanding provider is selected,
the models, endpoints, MCP transports and storage path — after all three
layers are merged. It is the fastest way to confirm your file is actually
being used. It prints no secrets.

### With Docker / the Makefile

The dev daemon (`make up`) and `make config` both read `./recuerdos-ai.toml`
from the repo (it is bind-mounted into the container), so what `make
config` prints is what the running daemon uses. Edit the file, then:

```bash
make config      # confirm the providers you set are the ones in effect
make restart     # apply them to the running daemon
```

The container also sets a few `RECUERDOS_AI_*` env vars of its own
(`STORAGE__PATH=/data`, `EMBEDDINGS__CACHE_DIR=/models`). Because env wins,
those paths override whatever the file says — which is intended, so data
lands on the Docker volume regardless of the file.

### API keys never live in the file

Provider keys are supplied through the **environment**, and the file only
names the variable that holds one:

```toml
[embeddings]
provider    = "gemini"
model       = "text-embedding-004"
api_key_env = "GEMINI_API_KEY"   # the NAME of an env var — not the key itself

[understanding]                  # the reasoning provider
provider    = "gemini"           # a preset over the OpenAI-compat protocol
model       = "gemini-2.0-flash"
api_key_env = "GEMINI_API_KEY"
```

Then put the actual key in the environment (a `.env` file next to
`docker-compose.yml` is auto-loaded by Compose; for bare metal, export it):

```bash
# .env
GEMINI_API_KEY=AIza…
```

Pasting the key straight into `api_key_env` does **not** work — the daemon
reads it as the name of a variable to look up, finds nothing, and behaves
as if no key were set. `recuerdos-ai config` shows `GEMINI_API_KEY (NOT SET)`
when the named variable is missing, which is the quickest way to catch this.

Full reference: [docs/configuration.md](docs/configuration.md).

## Docs

| | |
|---|---|
| [deployment.md](docs/deployment.md) | Run it on a server, with HTTPS and auth |
| [api.md](docs/api.md) | Every REST endpoint and field |
| [mcp.md](docs/mcp.md) | MCP tools and the `memory://profile` resource |
| [configuration.md](docs/configuration.md) | Every config key |
| [sdk-python.md](docs/sdk-python.md) | The Python client |
| [security.md](docs/security.md) | Isolation guarantees and threat model |
| [performance.md](docs/performance.md) | Benchmarks and how they were run |
| [architecture.md](docs/architecture.md) | Bounded contexts and layer rules |
| [evaluation.md](docs/evaluation.md) | How retrieval quality is measured |
| [CHANGELOG.md](docs/CHANGELOG.md) | What landed, per phase |

## Status

`v0.1.0` — a release candidate. Used daily by its author since Phase 3;
the API is stable enough to build on, and the storage format has migrated
cleanly across five phases.

| Phase | Scope | |
|---|---|---|
| 0 — Foundation | Docker dev env, config, HTTP skeleton, CI | ✅ |
| 1 — Identity | Users, API keys, per-user isolation | ✅ |
| 2 — Memories | Store + hybrid search (REST) | ✅ |
| 3 — MCP server | Claude Code / opencode integration | ✅ |
| 4 — Understanding | Extraction, labeling, reconciliation | ✅ |
| 5 — Consolidation | Dedup/merge, decay, profile digest | ✅ |
| 6 — Release | SDK, docs, packaging | ✅ |

Not yet: a web dashboard, Postgres/Qdrant backends, or the graph layer.
See [project-plan.md](project-plan.md) for where it is going.

> The project name is a working title — see
> [docs/name-check.md](docs/name-check.md). The repo may move before
> public launch.

## Contributing

**Docker is the only prerequisite.** No local Rust toolchain needed.

```bash
git clone <repo-url> recuerdos-ai && cd recuerdos-ai
just dev          # daemon with auto-rebuild on localhost:7070
just check        # fmt + clippy -D warnings + boundary script + tests
just sdk-test     # the Python SDK against a real daemon
just eval         # retrieval quality against the committed baseline
```

That last one matters more than it looks: retrieval quality emerges from
the embedder, the tokenizer, the RRF constant and the recency multiplier,
and a one-line change to any of them can make recall worse with every
unit test still green. `just eval` is the only check that would notice.

Prefer `make`? A `Makefile` wraps the same dev commands and adds the
operational ones — starting the daemon, creating users, issuing keys,
running consolidation. `make help` lists them; `make quickstart` brings
the daemon up and prints a ready-to-use API key.

Architecture is bounded contexts, each a vertical slice of `domain` /
`application` / `infrastructure`, with the layer rules enforced by a CI
script. Read [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) and
[docs/architecture.md](docs/architecture.md) before a first PR.

## License

Apache-2.0. See [LICENSE](LICENSE).
