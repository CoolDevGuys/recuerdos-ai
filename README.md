# 🧠 Recuerdos AI

[![CI](https://github.com/CoolDevGuys/recuerdos-ai/actions/workflows/ci.yml/badge.svg)](https://github.com/CoolDevGuys/recuerdos-ai/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

**Long-term memory for your AI agents — one service, all of them, on your
own machine.**

Every agent session starts as a blank slate. You re-explain your
architecture preferences to Claude Code. Your life-assistant bot forgets
you went vegetarian. And memory saved by one tool is invisible to the
others: your coding assistant learning you prefer `pnpm` does nothing for
anything else you run.

**Recuerdos AI** ("*recuerdos*" is Spanish for *memories*) is one daemon
that any agent reads from and writes to, over REST or MCP. It doesn't just
store what you tell it — it works out what is worth keeping, labels it, and
when you contradict yourself it *replaces* the old answer instead of
returning both.

```
                    ┌───────────────┐
Claude Code ─MCP──▶ │               │  extract → reconcile → store
opencode ────MCP──▶ │  Recuerdos AI │  recall: vector + BM25, fused
Hermes ─────REST──▶ │     daemon    │  nightly: merge, decay, expire
LangChain ──SDK───▶ │               │
                    └───────────────┘
                       SQLite + local ONNX embeddings
                       no external services required
```

- 🔒 **Yours** — SQLite on your disk, runs offline, no account, Apache-2.0.
- 🧩 **Any agent, any LLM** — REST + MCP, provider-agnostic, one shared memory.
- 🧠 **Understands, not just stores** — splits, labels, and resolves contradictions.
- ⚡ **Fast** — hybrid recall in tens of milliseconds at 100k memories.

---

## 🤔 Why not just use ChatGPT or Claude memory?

Built-in assistant memory is genuinely good — for **one person, one
assistant, stable facts**. It starts to pinch the moment you're building a
*product* or wiring up *several* tools.

**vs ChatGPT Memory** — ChatGPT remembers your name, preferences and
writing style. But that memory belongs to the *ChatGPT product*, not to
you. You can't inspect the database, tag or export memories, change how
they're ranked, or reuse them from your own app or a different model.
Recuerdos AI is a memory layer **your application owns**.

**vs Claude Code Memory** — Claude Code already has project memory, and
it's good — but it's *assistant* memory, centred on helping Claude code.
Recuerdos AI aims to be a memory **database**: many users, many agents,
semantic search, aging, deduplication, importance scoring — shared across
every tool and LLM you run.

| | Built-in assistant memory | Recuerdos AI |
|---|:---:|:---:|
| Own the raw database | ❌ | ✅ SQLite on your disk |
| Export, inspect, back up | ❌ | ✅ |
| Tags, categories, custom metadata | ❌ | ✅ |
| Semantic **and** keyword search | opaque | ✅ hybrid, fused |
| Dedup & contradiction resolution | ❌ | ✅ |
| Importance scoring & memory aging | ❌ | ✅ |
| Multiple users & agents, one shared store | ❌ | ✅ |
| Works with any LLM — or none | ❌ | ✅ |
| Runs on your machine, offline | ❌ | ✅ |

> In short: assistant memory is *assistant* memory. Recuerdos AI is a
> memory **database** you can point anything at.

---

## 🎯 What you can build with it

A memory layer earns its keep once you have **many conversations, many
users, many agents, or long-running workflows**. A few things people build:

- 🧑‍💻 **Coding agents** that never relearn your architecture — DDD
  conventions, folder layout, naming, deploy steps and past bug fixes
  carried into every session instead of an empty context.
- 🤝 **Personal assistants** that recall the *relevant* slice — preferred
  airlines, family members, favourite restaurants, recurring health notes —
  not just "likes dark mode".
- 🎧 **Customer support** agents that remember past tickets, tone, products
  owned and open issues, without resending the whole history to the model.
- 💼 **Sales assistants** with customer interests, budget, objections and
  who the decision-makers are.
- 🏥 **Healthcare assistants** tracking medications, symptoms *over time*
  and allergies — temporal, not one static fact.
- 🕸️ **Multi-agent systems** where a research agent, a coding agent and an
  email agent all read and write one shared memory layer.
- 🦾 **Robotics / spatial** — object locations, routines and room layouts
  that persist instead of being rebuilt each day.

<details>
<summary>A closer look at two of these</summary>

**Coding agent** — imagine a Hermes or Claude Code agent that, instead of
relearning your project every session, permanently remembers your DDD
conventions, folder structure, naming, deployment process, common mistakes
and previous fixes. Every future session starts with accumulated knowledge.
This is arguably the strongest use case.

**Multi-agent** — instead of isolated memories, agents share one layer:

```
Research agent ─┐
Coding agent  ──┼──▶  Recuerdos AI  ──▶ any agent can retrieve later
Email agent   ─┘        (shared memory)
```

Each agent contributes knowledge that all the others can later retrieve.
</details>

> For a single ChatGPT user, the built-in Memory is usually enough. For an
> AI *product*, it becomes limiting fast — that's the gap this fills.

---

## 🚀 Install & run

> **What's in a release?** A **Docker image**, standalone **binaries** for
> Linux/macOS, and the Python **client SDK** on PyPI. `install.sh` just
> fetches the right binary for your machine and checksum-verifies it — it
> *is* the binary release, in one command. Docker *is* the image release.
> Pick whichever fits; **Docker is the easiest and most portable.**

### 🐳 Docker — recommended

```bash
docker run -d --name recuerdos-ai -p 7070:7070 \
  -v recuerdos-ai-data:/data \
  -e RECUERDOS_AI_AUTH__MODE=none \
  ghcr.io/cooldevguys/recuerdos-ai
```

The embedding model is baked into the image, so it works offline and on
air-gapped hosts — no toolchain, no first-run download. Running it on a
real server? Follow the four-step [deployment guide](docs/deployment.md)
(HTTPS, auth, a key) and **drop `AUTH__MODE=none`**.

### 📦 Prebuilt binary — no Docker

```bash
curl -fsSL https://raw.githubusercontent.com/CoolDevGuys/recuerdos-ai/main/install.sh | sh
recuerdos-ai init && recuerdos-ai serve
```

Downloads the release binary for your OS (Linux x86_64/arm64, macOS arm64),
**verifies its checksum**, and puts it on your PATH — nothing else. Prefer
to do it by hand? Grab the tarball straight from the
[Releases page](https://github.com/CoolDevGuys/recuerdos-ai/releases).

### 🔨 From source

```bash
git clone https://github.com/CoolDevGuys/recuerdos-ai && cd recuerdos-ai
cargo build --release --bin recuerdos-ai
```

Needs a Rust toolchain (1.85+); the Docker path avoids that.

### 🐍 Python client

`pip install recuerdos-ai` gives you the **client SDK** — not the server.
Point it at a daemon you started with one of the methods above.

```python
from recuerdos_ai import Client

ra = Client(api_key="ra_live_…")
ra.save("We moved the backend to Hetzner; fly.io got too expensive")

for hit in ra.search("where do we deploy?"):
    print(hit.content)
```

[Full SDK reference →](docs/sdk-python.md)

---

## ⚡ Try it in 60 seconds

Once the daemon is running, store something:

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

**Nothing left the machine.** 🔒 Embeddings are computed in-process by an
ONNX model baked into the image.

---

## 🔌 Connect your tools

Point Claude Code at it with an `.mcp.json`:

```jsonc
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

It recalls the preference without being told. ✨

**Recipes for every client:**
[Claude Code](docs/integrations/claude-code.md) ·
[opencode](docs/integrations/opencode.md) ·
[Hermes](docs/integrations/hermes.md) ·
[LangChain](docs/integrations/langchain.md) ·
[any REST client](docs/integrations/custom-agents.md)

> 🌐 Running the daemon on a server? Clients connect over **HTTP MCP** at
> `https://your-host/mcp` with a bearer token — no local binary. See each
> recipe and [docs/mcp.md](docs/mcp.md).

---

## 🧠 What it does

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

---

## 📊 Performance

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

---

## 🔒 Your data stays yours

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

---

## ⚙️ Configuration

Settings come from three layers, each overriding the one before:

**defaults → `recuerdos-ai.toml` → `RECUERDOS_AI_*` environment variables**

So an env var always wins over the file, and the file always wins over the
built-in defaults. [`recuerdos-ai.example.toml`](recuerdos-ai.example.toml)
lists every key with its default and a comment; copy it (or run
`recuerdos-ai init`) and change only what you need.

**The file is read only when you point at it** — with `--config PATH`, or
by setting `RECUERDOS_AI_CONFIG=PATH`. There is no automatic discovery:
`recuerdos-ai serve` on its own uses defaults + env and *ignores* a
`recuerdos-ai.toml` sitting next to it.

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
the models, endpoints, MCP transports and storage path. It's the fastest
way to confirm your file is being used, and it prints **no secrets**.

**API keys never live in the file.** The file names the env var that holds
one; the value comes from the environment:

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

```bash
# .env  (auto-loaded by Docker Compose; on bare metal, export it)
GEMINI_API_KEY=AIza…
```

Full reference: [docs/configuration.md](docs/configuration.md).

---

## 📚 Docs

| | |
|---|---|
| 🚀 [deployment.md](docs/deployment.md) | Run it on a server, with HTTPS and auth |
| 🌐 [api.md](docs/api.md) | Every REST endpoint and field |
| 🔌 [mcp.md](docs/mcp.md) | MCP tools and the `memory://profile` resource |
| ⚙️ [configuration.md](docs/configuration.md) | Every config key |
| 🐍 [sdk-python.md](docs/sdk-python.md) | The Python client |
| 🔒 [security.md](docs/security.md) | Isolation guarantees and threat model |
| 📊 [performance.md](docs/performance.md) | Benchmarks and how they were run |
| 🏛️ [architecture.md](docs/architecture.md) | Bounded contexts and layer rules |
| 🎯 [evaluation.md](docs/evaluation.md) | How retrieval quality is measured |
| 📝 [CHANGELOG.md](docs/CHANGELOG.md) | What landed, per phase |

---

## 🗺️ Status

`v0.1.0` — a release candidate. Used daily by its author since Phase 3;
the API is stable enough to build on, and the storage format has migrated
cleanly across every phase.

| Phase | Scope | |
|---|---|:---:|
| 0 — Foundation | Docker dev env, config, HTTP skeleton, CI | ✅ |
| 1 — Identity | Users, API keys, per-user isolation | ✅ |
| 2 — Memories | Store + hybrid search (REST) | ✅ |
| 3 — MCP server | Claude Code / opencode integration | ✅ |
| 4 — Understanding | Extraction, labeling, reconciliation | ✅ |
| 5 — Consolidation | Dedup/merge, decay, profile digest | ✅ |
| 6 — Release | SDK, docs, packaging | ✅ |

**On the roadmap:** a web dashboard, Postgres/Qdrant storage backends,
user-visible collections/workspaces, and a knowledge-graph layer. See
[project-plan.md](project-plan.md) for where it's headed.

---

## 🤝 Contributing

**Docker is the only prerequisite.** No local Rust toolchain needed.

```bash
git clone https://github.com/CoolDevGuys/recuerdos-ai && cd recuerdos-ai
just dev          # daemon with auto-rebuild on localhost:7070
just check        # fmt + clippy -D warnings + boundary script + tests
just sdk-test     # the Python SDK against a real daemon
just eval         # retrieval quality against the committed baseline
```

That last one matters more than it looks: retrieval quality emerges from
the embedder, the tokenizer, the RRF constant and the recency multiplier,
and a one-line change to any of them can make recall worse with every unit
test still green. `just eval` is the only check that would notice.

Prefer `make`? A `Makefile` wraps the same dev commands and adds the
operational ones — starting the daemon, creating users, issuing keys,
running consolidation. `make help` lists them; `make quickstart` brings the
daemon up and prints a ready-to-use API key.

Architecture is bounded contexts, each a vertical slice of `domain` /
`application` / `infrastructure`, with the layer rules enforced by a CI
script. Read [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) and
[docs/architecture.md](docs/architecture.md) before a first PR.

---

## 📄 License

Apache-2.0. See [LICENSE](LICENSE). Made with care by
[CoolDevGuys](https://github.com/CoolDevGuys). 💙
