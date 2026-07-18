# RecordAgent — Long-Term Memory Service for AI Agents

**Project plan · v0.1 · 2026-07-18**

A fast, self-hostable long-term memory service for AI agents and coding assistants.
One memory backend, consumed by every agent you use — Claude Code, opencode, Hermes
Agent, LangChain agents, custom bots — via **REST API** or **MCP**.

---

## Table of contents

1. [Problem statement](#1-problem-statement)
2. [Target users](#2-target-users)
3. [Competitive landscape & research findings](#3-competitive-landscape--research-findings)
4. [Implementation strategies (options analysis)](#4-implementation-strategies-options-analysis)
5. [Recommended architecture](#5-recommended-architecture)
6. [Tech stack](#6-tech-stack)
7. [Feature analysis — each brainstormed feature, researched](#7-feature-analysis)
8. [Data model](#8-data-model)
9. [API design (REST + MCP)](#9-api-design-rest--mcp)
10. [Configuration](#10-configuration)
11. [Multi-user isolation & security](#11-multi-user-isolation--security)
12. [Usage examples & scenarios](#12-usage-examples--scenarios)
13. [Quality, testability, scalability](#13-quality-testability-scalability)
14. [Limitations (honest list)](#14-limitations)
15. [Roadmap & possible upgrades](#15-roadmap--possible-upgrades)
16. [Open source vs. SaaS strategy](#16-open-source-vs-saas-strategy)
17. [Open questions](#17-open-questions)

---

## 1. Problem statement

Every AI agent session starts as a blank slate. The pain shows up differently per
audience but has one root cause — no durable, queryable memory shared across tools:

- **Coding agents** (Claude Code, opencode, Cursor): you re-explain your architecture
  preferences ("we use hexagonal architecture", "never use default exports", "tests go
  in `__tests__/`"), what was implemented last week, and why a decision was made.
  CLAUDE.md / AGENTS.md files help but are flat, unversioned-in-spirit, per-repo,
  manually curated, and get stale.
- **Life/assistant agents** (Hermes Agent, openClaw, custom LangChain bots): they
  forget your preferences, routines, people, and past conversations, or each agent
  keeps its own incompatible silo of memory.
- **Cross-tool fragmentation**: memory saved by one tool is invisible to the others.
  Your Claude Code session learns you prefer `pnpm`; your Hermes agent doesn't know.

**RecordAgent's thesis:** memory should be a *service* you own — a single fast daemon
that any agent can read from and write to, that *understands* what it stores
(extracting facts, labeling, categorizing, deduplicating, resolving contradictions),
and that isolates each user's memories strictly.

---

## 2. Target users

| Persona | Tools they use | What memory does for them |
|---|---|---|
| **AI-powered developer** | Claude Code, opencode, Cursor, Zed | Remembers coding style, architecture prefs, project decisions, implemented features, past bug fixes, tool/infra conventions |
| **Agent power-user** | Hermes Agent, openClaw, Telegram/Discord bots | Remembers personal facts, preferences, routines, people, ongoing projects, past tasks |
| **Agent builder** | LangChain / LangGraph, custom stacks, Claude Agent SDK | Drop-in memory API so they don't build extraction + vector store + dedup themselves |
| **(SaaS phase) small teams** | Same tools, shared org | Team-shared project memory with per-user private memory |

---

## 3. Competitive landscape & research findings

Research summary of the main existing systems and how they work internally. This
matters both for design inspiration and for positioning if this becomes OSS/SaaS.

### 3.1 Mem0 (~47k★, cloud-first + OSS SDK)

- **Two-phase LLM pipeline.** *Extraction phase*: an LLM reads the last M messages
  and extracts candidate memories (salient facts). *Update phase*: each candidate is
  compared against semantically similar existing memories and the LLM classifies the
  operation: **ADD / UPDATE / DELETE / NOOP**. This is the key idea that makes memory
  "understood" rather than raw-logged.
- **Storage:** vector store (many pluggable backends) + optional graph layer
  ("Mem0g") storing entity–relationship triples (`Alice —works_at→ Google`) with
  embeddings on nodes; retrieval is hybrid vector + graph traversal.
- **Caveats found:** the polished experience is the paid cloud; OSS gives mostly the
  vector layer. Graph memory is behind the Pro plan. Scored ~49% on LongMemEval
  (GPT-4o config) vs Zep's ~64%.

### 3.2 Zep / Graphiti (temporal knowledge graph)

- Built on **Graphiti**, a temporally-aware knowledge graph engine. Three-level
  hierarchy: *episode subgraph* (raw messages/events), *semantic entity subgraph*
  (extracted entities/facts), *community subgraph* (clusters/summaries).
- **Bi-temporal model:** every edge carries `t_valid` / `t_invalid` plus ingestion
  time. "I moved from London to Tokyo" *invalidates* the London fact instead of
  leaving two contradictory facts — outdated facts are superseded, not deleted.
- Conflict detection uses semantic + keyword (BM25) + graph search. P95 retrieval
  ~300 ms. Requires Neo4j/FalkorDB — heavy for a personal self-hosted deploy.

### 3.3 Letta (MemGPT)

- "Memory as an OS": in-context **core memory** blocks (RAM) the agent edits itself
  via tools, plus out-of-context **archival memory** (disk) it pages in. The *agent*
  decides what to remember; the framework hosts the agent. It's an agent runtime more
  than a memory service — different shape than what we want (we want a service
  *consumed by* existing agents, not a new agent host).

### 3.4 MCP memory servers (OpenMemory MCP, mcp-memory-service, MemPalace, engram_mcp…)

- Prove strong demand for **local-first, self-hosted memory over MCP** for Claude
  Code specifically. Most are thin: store + vector-search, little understanding
  (no extraction/consolidation), single-user, Python/TS (slow cold start), no REST
  API for non-MCP consumers. `engram_mcp` shows the Rust + SQLite + fastembed
  combination already works in practice.

### 3.5 Positioning gap RecordAgent fills

No existing OSS project combines **all** of: (1) Rust-fast single binary,
(2) Mem0-style LLM understanding pipeline (extract → classify ADD/UPDATE/DELETE →
label), (3) both REST **and** MCP surfaces from one daemon, (4) real per-user
isolation with API keys, (5) pluggable LLM/embedding providers incl. fully-local,
(6) config-file driven. That combination is the product.

---

## 4. Implementation strategies (options analysis)

Three strategies were evaluated. **Strategy A is recommended for the POC**, with a
deliberate upgrade path to Strategy B.

### Strategy A — "Fast Local Brain": Rust single binary, embedded hybrid search, Mem0-style pipeline ✅ recommended

- **Storage:** SQLite (WAL mode) as the system of record + `sqlite-vec` for ANN
  vector search + `tantivy` (Rust Lucene) for BM25 full-text — hybrid retrieval with
  reciprocal-rank fusion, all embedded in-process. Zero external services.
- **Understanding:** async extraction pipeline (LLM extracts facts → classifies
  ADD/UPDATE/DELETE/NOOP against similar existing memories → assigns labels/category).
- **Interfaces:** one daemon exposes REST (axum) and MCP (rmcp, stdio + streamable
  HTTP) sharing the same core.
- **Pros:** single ~30 MB binary, `curl | sh` install, sub-10 ms retrieval at
  personal scale (≤ ~1M memories/user), trivially testable, cheapest to run, best
  OSS adoption story ("no Docker Compose with 4 containers").
- **Cons:** SQLite is single-writer (fine at this scale; writes are queued);
  horizontal scale requires the storage-trait swap (planned, see below).

### Strategy B — "Temporal Graph Memory": Zep-style bi-temporal knowledge graph

- Entities + relations extracted into a property graph (embedded: `oxigraph` or
  SQLite edge tables; scaled: Neo4j/FalkorDB/Memgraph). Facts carry
  `valid_from`/`invalid_at`; contradictions supersede rather than duplicate.
  Retrieval = vector + BM25 + graph-hop expansion.
- **Pros:** best answer quality for temporal/relational questions ("what DB did we
  use *before* the migration?"), strongest technical differentiation, benchmark-
  proven approach (Zep's LongMemEval results).
- **Cons:** 3–4× implementation complexity; graph extraction burns significantly
  more LLM tokens per ingest; hard to keep in a single binary at scale; ingestion
  latency higher. **Wrong first step for a POC**, right second step for a moat.

### Strategy C — "Thin & Pluggable": orchestration layer over existing infra (Qdrant/Redis + provider APIs)

- RecordAgent is only the pipeline + API layer; storage is external Qdrant (vectors)
  and Redis (hot cache/metadata).
- **Pros:** least storage code to write; Qdrant gives best-in-class ANN at huge
  scale immediately; Redis gives sub-ms hot reads.
- **Cons:** self-hosters must run 3 services; you own the least IP (harder to
  monetize); ops burden contradicts the "easy personal deploy" goal.

### Decision

**Ship Strategy A** behind a `MemoryStore` trait, so Strategy C's backends (Qdrant,
pgvector) are alternative trait implementations for the SaaS/scale phase, and
Strategy B's graph layer is an additive v2 feature (the schema below already
reserves entity/relation tables and supersedence links so no migration pain later).

---

## 5. Recommended architecture

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

### Key flows

**Ingest (write path — async by design):**
1. Client calls `POST /v1/memories` (or MCP `memory_save`) with raw content
   (a message, an observation, a session summary) + optional hints.
2. API validates auth → resolves `user_id` → enqueues an *ingestion job* →
   returns `202` immediately (agents must never block on memory writes).
3. Worker runs the **understanding pipeline**:
   a. *Extract*: LLM turns raw content into 0–N atomic memory candidates.
   b. *Embed*: embedding provider vectorizes each candidate.
   c. *Reconcile*: top-K similar existing memories fetched; LLM classifies
      ADD / UPDATE / DELETE / NOOP per candidate (Mem0-style).
   d. *Label*: LLM assigns category (fixed taxonomy) + free-form tags +
      confidence + optional `expires_at`.
   e. *Store*: transactional write to SQLite; vector + FTS indexes updated.
4. Every mutation is appended to an **audit log** (supports undo, debugging,
   and the "show me every edit" trust story Letta popularized).

**Retrieve (read path — hot, no LLM required):**
1. `GET/POST /v1/memories/search` (or MCP `memory_recall`) with a query +
   optional category/tag/time filters.
2. Query embedded (cached) → parallel vector ANN + BM25 → reciprocal-rank fusion →
   metadata filter → optional cross-encoder-free rerank via recency/confidence
   boost → return in < ~15 ms locally (excluding embedding call if remote).
3. Optional `synthesize=true`: an LLM compresses the top hits into a compact
   context paragraph (costs a call; off by default).

**Maintenance (background):**
- Periodic **consolidation**: cluster near-duplicate memories, merge, re-label.
- **Decay/expiry**: TTL'd memories pruned; unused low-confidence memories demoted.
- **Session distillation**: accept whole session transcripts and distill into a few
  durable memories (mirrors Claude Code's compaction moments — best hook point).

---

## 6. Tech stack

| Layer | Choice | Why |
|---|---|---|
| Language | **Rust** (edition 2024) | Performance goal; single static binary; Qdrant-class perf ceiling |
| Async runtime | tokio | Standard |
| HTTP API | axum + tower middleware | Ergonomic, fast, middleware for auth/rate-limit |
| MCP | **rmcp** (official Rust MCP SDK) — stdio + streamable HTTP transports | One codebase serves Claude Code, opencode, Hermes |
| System of record | SQLite via `rusqlite` (WAL) | Embedded, transactional, backup = copy a file |
| Vector index | **sqlite-vec** | In-process ANN, no service; proven with fastembed in engram_mcp |
| Full-text/BM25 | **tantivy** | Rust Lucene; hybrid search is a measurable quality win over vector-only |
| Local embeddings | **fastembed-rs** (ONNX: bge-small-en-v1.5 / all-MiniLM-L6-v2, 384-dim) | Zero-config default — service works with NO external provider |
| LLM providers | Provider trait: `anthropic`, `openai-compat` (covers OpenAI/OpenRouter/Groq), `ollama` | User-selected; all four requested options covered |
| Job queue | SQLite-backed queue table + tokio workers (POC) → trait allows Redis/NATS later | No extra infra |
| Config | TOML (`recordagent.toml`) + env-var overrides (`RECORDAGENT_*`) + hot-reload for provider keys | Requested: config-file driven |
| Observability | `tracing` + optional Prometheus `/metrics` | SaaS-readiness |
| CLI | `recordagent` binary: `serve`, `init`, `user add`, `key issue`, `export`, `import`, `consolidate` | Ops & onboarding |
| Distribution | cargo, Homebrew tap, single-file installer, Docker image (optional, not required) | OSS adoption |

**Performance targets (POC acceptance):** recall P95 < 50 ms end-to-end with local
embeddings at 100k memories/user; ingest ack < 5 ms (async); cold start < 300 ms;
RSS < 150 MB idle with ONNX model loaded.

---

## 7. Feature analysis

Each feature from the brainstorm, researched: how to implement it and whether it's
truly necessary.

### 7.1 "Understand memories, create labels/categories" — NECESSARY (core differentiator)

- **How:** the extraction step assigns each memory a `category` from a fixed,
  config-extensible taxonomy plus free-form `tags`. Fixed taxonomy matters:
  free-form-only labels fragment ("prefs", "preferences", "user-prefs") and ruin
  filtered retrieval. Proposed default taxonomy:
  - `preference.coding` (style, tooling, patterns: "prefers pnpm", "no default exports")
  - `preference.personal` (life prefs: "vegetarian", "prefers morning meetings")
  - `decision` (architecture/product decisions + the *why*)
  - `fact.project` (implemented features, stack facts, constraints)
  - `fact.person` (people, relationships, roles)
  - `experience` (what happened: "the pgvector migration failed because…")
  - `skill` (learned procedures — aligns with Hermes Agent's skill files)
  - `reference` (URLs, tickets, dashboards)
- **Implementation:** single structured-output LLM call per candidate (JSON schema
  with `category`, `tags[]`, `confidence`, `entities[]`). Entities stored now even
  though the graph layer is v2 — future-proofing at near-zero cost.
- **Necessity verdict:** yes — this is precisely what thin MCP memory servers lack
  and what makes retrieval granular.

### 7.2 LLM providers for understanding/compaction/embeddings — NECESSARY

- **How:** two Rust traits, `ChatProvider` and `EmbeddingProvider`, configured
  independently (e.g., Anthropic for extraction + local ONNX for embeddings — the
  cheapest good default, since Anthropic has no embeddings API).
- **Critical design rule:** *pin embedding model per collection.* Mixing embedding
  models silently corrupts similarity search. Store `embedding_model` +
  `dimensions` in metadata; changing it triggers an explicit `reindex` command.
- **Degraded mode:** with no chat provider configured, the service still works as
  store + hybrid search (extraction disabled, content stored verbatim with
  heuristic tags). Never hard-depend on an LLM to accept writes.

### 7.3 Config-file driven — NECESSARY, cheap

TOML file (full example in §10). Validated at startup with precise error messages
(`figment` or hand-rolled). Env-var overrides so Docker/SaaS deploys need no file.

### 7.4 Per-user isolation — NECESSARY (and cheap now, expensive later)

- **How:** every row carries `user_id`; every query begins from an
  auth-middleware-resolved `UserContext` — handlers physically cannot construct a
  query without it (enforced by the type system: store methods take
  `&UserContext`, not a raw id). API keys are hashed (argon2) with prefix lookup
  (`ra_live_xxxx…`). One tantivy index per user (also speeds up per-user search);
  sqlite-vec queries always carry the `user_id` filter.
- **Verdict:** requested and SaaS-critical; retrofitting later touches everything,
  so it ships in the POC.

### 7.5 Redis — NOT necessary for POC

Researched role: hot cache + queue. At personal scale SQLite+memory cache wins on
ops simplicity, and the daemon already keeps an in-process LRU for query
embeddings. Redis returns as an optional cache/queue backend in the SaaS phase.
Recommendation: **skip for POC**, keep behind traits.

### 7.6 Simple file storage — REJECTED as engine, KEPT as interchange

Flat markdown files (the CLAUDE.md model) don't scale past hundreds of memories,
can't do semantic search, can't enforce isolation. But **markdown export/import**
(`recordagent export --format md`) is kept: it's the trust feature — "your
memories are yours, readable, greppable, portable." Also enables git-versioned
memory backups.

### 7.7 Vector DB / RAG — YES, as described

Hybrid (vector + BM25 + metadata filter) rather than pure vector: literal
identifiers matter enormously for the coding persona (`useQuery`, `pnpm`,
`RA-1234`) and pure-vector retrieval is weak on exact tokens. This is a
retrieval-quality decision backed by common RAG benchmark practice.

### 7.8 Memory compaction/consolidation — NECESSARY (v1, not v0)

Without it, memories accumulate near-duplicates and contradictions. Nightly job:
cluster by similarity ≥ threshold → LLM merges clusters → originals linked as
`superseded_by` (never hard-deleted; audit trail preserved). The ADD/UPDATE/DELETE
reconcile step at ingest time (7.1) already prevents ~80% of the mess; the nightly
job catches drift.

---

## 8. Data model

```sql
users(id, email, display_name, created_at)
api_keys(id, user_id, key_hash, prefix, name, scopes, last_used_at, revoked_at)
collections(id, user_id, name, embedding_model, dimensions)   -- default: "main"
memories(
  id, user_id, collection_id,
  content TEXT,                -- the atomic memory sentence(s)
  category TEXT,               -- taxonomy value (§7.1)
  tags JSON,                   -- free-form labels
  entities JSON,               -- [{name, type}] reserved for graph v2
  confidence REAL,             -- extractor confidence 0..1
  importance REAL,             -- decay-weighted score
  source JSON,                 -- {client:"claude-code", session_id, raw_ref}
  created_at, updated_at, last_accessed_at, expires_at,
  superseded_by,               -- consolidation chain
  embedding BLOB               -- via sqlite-vec virtual table
)
memory_audit(id, memory_id, user_id, op, actor, diff JSON, at)  -- ADD/UPDATE/DELETE/MERGE
ingest_jobs(id, user_id, payload, status, attempts, error, created_at)
```

Retrieval score = `rrf(vector_rank, bm25_rank) × recency_boost × confidence`.
`last_accessed_at` feeds importance decay (rarely-recalled memories rank lower,
never silently deleted).

---

## 9. API design (REST + MCP)

### REST (v1, JSON, bearer API key)

| Endpoint | Purpose |
|---|---|
| `POST /v1/memories` | Ingest raw content (async understanding) — `202 {job_id}` |
| `POST /v1/memories:direct` | Store verbatim, skip extraction (agent already distilled it) |
| `POST /v1/memories/search` | Hybrid search: `{query, categories?, tags?, since?, limit?, synthesize?}` |
| `GET/PATCH/DELETE /v1/memories/{id}` | CRUD (delete = soft + audit) |
| `POST /v1/sessions/distill` | Submit a whole transcript for distillation |
| `GET /v1/memories/export` | Markdown/JSON export |
| `GET /v1/audit` | Mutation history |
| `GET /healthz`, `GET /metrics` | Ops |

### MCP tools (same engine, thin adapter)

- `memory_save(content, category?, tags?)` — descriptions written so agents call it
  when the user states a preference/decision/durable fact.
- `memory_recall(query, categories?, limit?)`
- `memory_forget(query_or_id)` — returns candidates for confirmation, never blind-deletes.
- `session_distill(transcript_summary)` — for compaction/session-end hooks.
- MCP *resource*: `memory://profile` — a compact auto-maintained "who this user is"
  digest agents can pull at session start (cheapest way to get value on turn 1).

**Client integration recipes to ship in docs:** Claude Code (`.mcp.json` +
SessionStart/PreCompact hooks that call `session_distill`), opencode (MCP config),
Hermes Agent (REST tool or MCP), LangChain (`RecordAgentMemory` Python retriever,
thin HTTP wrapper — the one place Python appears).

---

## 10. Configuration

```toml
# recordagent.toml
[server]
host = "127.0.0.1"
port = 7070
mcp  = { stdio = true, http = true }

[storage]
backend = "embedded"            # embedded | postgres | qdrant (post-POC)
path    = "~/.recordagent/data"

[embeddings]
provider = "local"              # local | openai-compat | ollama
model    = "bge-small-en-v1.5"  # pinned per collection; change requires `reindex`

[understanding]
provider    = "anthropic"       # anthropic | openai-compat | ollama | none
model       = "claude-haiku-4-5"
api_key_env = "ANTHROPIC_API_KEY"
reconcile   = true              # ADD/UPDATE/DELETE/NOOP pass
[understanding.taxonomy]        # extend defaults
extra_categories = ["fact.homelab"]

[consolidation]
enabled  = true
schedule = "daily"
similarity_threshold = 0.92

[retrieval]
hybrid = true                   # vector + BM25 RRF
default_limit = 8
recency_half_life_days = 90

[auth]
mode = "api-key"                # api-key | none (explicit single-user opt-out)
```

---

## 11. Multi-user isolation & security

- Auth middleware → `UserContext`; storage API is uncallable without it (compile-time
  guarantee, plus tests that assert cross-user queries return zero rows).
- API keys: argon2-hashed, prefixed, scoped (`read`, `write`, `admin`), revocable,
  `last_used` tracked. CLI: `recordagent key issue --user alex --scopes read,write`.
- All PII stays local by default (local embeddings + optional local Ollama
  extraction = fully offline mode).
- When a remote LLM is configured, the config makes the data flow explicit;
  `understanding.provider = "none"` guarantees zero egress.
- SaaS additions later: per-tenant encryption at rest, rate limits per key, org
  namespaces, SSO. POC keeps the door open, doesn't build them.

---

## 12. Usage examples & scenarios

### Scenario 1 — Coding preferences persist across tools
> **Session (Claude Code):** "Stop using barrel files, I hate index.ts re-exports."
> → hook/agent calls `memory_save`. Pipeline stores:
> `{category: "preference.coding", tags:["typescript","imports"], content:"User forbids barrel files / index.ts re-exports; import directly from source modules."}`
>
> **Next week (opencode, different repo):** agent pulls `memory://profile` at start
> → generates direct imports without being told.
> **Problem solved:** zero re-explaining across tools and repos.

### Scenario 2 — Architecture decision recall
> **March:** "We chose SQLite over Postgres for the desktop app because installer
> size matters more than concurrent writes." → stored as `decision` with the *why*.
> **July:** dev asks agent "why aren't we on Postgres again?" → `memory_recall`
> returns the decision + rationale + date.
> **Problem solved:** decision archaeology without grepping old chats/ADRs.

### Scenario 3 — Contradiction handling (the Mem0 UPDATE case)
> Stored: "User deploys on Fly.io." Later: "We migrated everything to Hetzner."
> Reconcile step classifies UPDATE → Fly.io memory superseded (kept in audit),
> Hetzner is now the single current fact. A naive store would return both and the
> agent would guess.
> **Problem solved:** memory that stays *true*, not just *big*.

### Scenario 4 — Daily-life agent (Hermes)
> Telegram: "book dinner Friday — remember I'm vegetarian now" → REST ingest →
> `preference.personal` UPDATE (supersedes "loves steakhouses", stored 2024).
> Any future food task, in any connected agent, filters by
> `category=preference.personal` and gets current dietary truth.

### Scenario 5 — Session distillation at compaction
> Claude Code PreCompact hook posts the session summary to `/v1/sessions/distill`.
> Pipeline extracts 3 durable memories from a 200-message session: a fixed bug +
> root cause (`experience`), a new convention (`preference.coding`), a feature now
> implemented (`fact.project`). The other 197 messages die with the session —
> **selective** memory is the feature, not total recall.

### Scenario 6 — Isolation
> Alex's key searches "database preferences" → only Alex's rows. Sam's key, same
> query, same server → only Sam's. Verified by cross-tenant test suite (§13).

---

## 13. Quality, testability, scalability

**Testing strategy (OSS credibility depends on this):**
- Unit: taxonomy classification, RRF math, auth middleware, config parsing.
- Provider mocks: `ChatProvider`/`EmbeddingProvider` fakes → the full pipeline is
  testable offline and deterministically in CI.
- Integration: spin real SQLite in tmpdir, run ingest→recall golden tests.
- **Cross-tenant suite:** property-based tests asserting no query path leaks rows
  across users (this is the test suite that sells trust).
- **Retrieval quality harness:** a small curated eval set (needle-in-haystack,
  contradiction, temporal questions) scored on recall@k — run on PRs so quality
  regressions are visible. Post-POC: run LongMemEval subset for public benchmarks.
- Load: `oha`/`k6` scripts for the P95 targets in §6.

**Scalability path:**
1. POC: embedded, one node, ~1M memories/user — covers every self-hoster.
2. `MemoryStore` trait → Postgres+pgvector implementation (multi-node SaaS).
3. Qdrant implementation for >10M-vector tenants; Redis cache layer.
4. Job queue trait → NATS/Redis for worker pools.
The trait boundaries are designed in the POC precisely so none of this is a rewrite.

---

## 14. Limitations

Honest list — also belongs in the future README:

- **Extraction quality is bounded by the configured LLM.** Local-model extraction
  (Ollama 7–8B) misses nuance; taxonomy misfiles happen. Mitigation: confidence
  scores + audit + easy manual re-label.
- **Ingest costs LLM tokens** (one small call per save, one per reconcile batch).
  Mitigation: batching, Haiku-class models, degraded verbatim mode.
- **Async ingestion means a save is not instantly recallable** (typically < 2 s).
  Acceptable for memory semantics; documented.
- **Embedding model is pinned** — switching models requires reindexing (explicit
  command, but it's a real cost at large N).
- **SQLite single-writer**: fine to ~dozens of concurrent agents per node; the
  Postgres backend is the answer beyond that, not SQLite tuning.
- **No temporal-graph reasoning in v1** ("what was true in March?") — the schema
  reserves supersedence + entities for the Strategy-B upgrade; the audit log gives
  primitive time-travel meanwhile.
- **English-first taxonomy prompts** initially.
- **Memory ≠ truth**: the service stores what agents/users assert; garbage in,
  confidently-labeled garbage out. Audit + forget tools are the remedy.

---

## 15. Roadmap & possible upgrades

**Phase 0 — POC (target ~3–4 weekends of work)**
Daemon + embedded store + local embeddings + hybrid search + API keys + REST +
MCP stdio + basic extraction (Anthropic) + TOML config + export + Claude Code
integration recipe. *Exit criterion: you use it daily from Claude Code AND one
other client, and it's noticeably useful.*

**Phase 1 — Understanding depth**
Reconcile (ADD/UPDATE/DELETE), consolidation job, `memory://profile` digest,
session distillation hooks, opencode + Hermes recipes, retrieval eval harness.

**Phase 2 — OSS launch**
Docs site, Homebrew/installer, Docker image, LangChain retriever, benchmark
blog post (LongMemEval subset), README with the isolation test story.

**Phase 3 — Scale & moat**
Postgres/Qdrant backends, temporal graph layer (Strategy B: bi-temporal facts,
entity graph, graph-hop retrieval), Redis cache, multi-node workers.

**Phase 4 — SaaS (conditional on traction)**
Hosted control plane, orgs/teams (shared project memory + private personal
memory), SSO, per-tenant encryption, usage-based billing, dashboard UI
(memory browser/editor — surprisingly, the feature non-technical users pay for).

**Upgrade ideas parked for later:** proactive memory surfacing (push relevant
memories into context before the agent asks), memory sharing/export between users
by explicit grant, mobile-friendly read API, WASM embedding fallback, encrypted
sync between personal devices.

---

## 16. Open source vs. SaaS strategy

- **License:** Apache-2.0 for maximum adoption (memory infra wins on trust;
  restrictive licenses kill the self-host audience that gives you distribution).
  If SaaS materializes, moat = hosted convenience + team features + dashboard,
  not license restrictions (the Mem0 playbook, but with a more generous OSS core —
  that generosity *is* the differentiation, since Mem0's OSS tier is deliberately
  thin).
- **What people would pay for** (based on the landscape): zero-ops hosted memory,
  team/org shared memory, the web dashboard, SLAs, and cross-device sync — *not*
  the engine itself. So the engine can be fully open without cannibalizing SaaS.
- **Name check before launch:** verify crate/repo/domain availability and trademark
  conflicts for the final name ("RecordAgent" is a working title).

---

## 17. Open questions

1. Default remote extraction model: Haiku-class (cheap, good enough?) vs
   Sonnet-class (better labels, 5–10× cost) — decide with the eval harness.
2. Should `memory://profile` be one digest or per-domain digests
   (coding vs personal)? Leaning per-domain, selected by client type but memories are not limited to only one domain, they can be interdomain.
3. Python SDK at POC time or Phase 2? (LangChain users can't consume Rust
   directly; a 200-line `httpx` wrapper may be worth shipping early.) - Ship a python wrapper that can potentially grow in the future.
4. Telemetry for OSS builds: none vs opt-in anonymous — affects roadmap data
   but risks trust. Leaning none.

---

## Appendix — Research sources

- Landscape: [Mem0 vs Zep vs Letta comparisons](https://particula.tech/blog/agent-memory-frameworks-tested-mem0-zep-letta-cognee-2026), [vectorize.io framework roundup](https://vectorize.io/articles/best-ai-agent-memory-systems), [niteagent showdown](https://niteagent.com/blog/ai-agent-memory-comparison-2026/)
- Mem0 internals: [Mem0 paper (arXiv 2504.19413)](https://arxiv.org/html/2504.19413v1), [architecture breakdown](https://medium.com/@zeng.m.c22381/mem0-overall-architecture-and-principles-8edab6bc6dc4), [graph memory deep-dive](https://deepwiki.com/mem0ai/mem0/4-graph-memory)
- Zep/Graphiti: [Zep paper (arXiv 2501.13956)](https://arxiv.org/abs/2501.13956), [Graphiti overview (Neo4j blog)](https://neo4j.com/blog/developer/graphiti-knowledge-graph-memory/)
- MCP memory ecosystem: [Mem0 Claude Code setup](https://mem0.ai/blog/claude-code-memory), [OpenMemory MCP](https://mem0.ai/blog/introducing-openmemory-mcp), [self-hosted mem0 MCP walkthrough](https://dev.to/n3rdh4ck3r/how-to-give-claude-code-persistent-memory-with-a-self-hosted-mem0-mcp-server-h68), [mcp-memory-service](https://pypi.org/project/mcp-memory-service/)
- Storage: [vector DB comparison](https://encore.dev/articles/best-vector-databases), [embedded vector DBs](https://shaharia.com/blog/choosing-embeddable-vector-database-go-application/), [sqlite-vec](https://dev.to/aairom/embedded-intelligence-how-sqlite-vec-delivers-fast-local-vector-search-for-ai-3dpb), [Qdrant](https://qdrant.tech/)
- Rust building blocks: [rmcp — official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk), [engram_mcp (SQLite + fastembed precedent)](https://lib.rs/crates/engram_mcp)
- Hermes Agent: [docs](https://hermes-agent.nousresearch.com/docs/), [repo](https://github.com/nousresearch/hermes-agent)
