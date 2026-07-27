# Recuerdos AI — Technical Implementation Plan

**v0.1 · 2026-07-18 · companion to [project-plan.md](project-plan.md)**

Actionable, phased breakdown of the build. Every phase lives on its own git branch,
ends in a **shippable, usable** state, and every task has a goal, step-by-step
procedure, examples where they apply, and a Definition of Done (DoD).

---

## Table of contents

1. [How to read this plan](#1-how-to-read-this-plan)
2. [Architecture: DDD, vertical slices, screaming architecture](#2-architecture)
3. [Repository layout](#3-repository-layout)
4. [Phase 0 — Foundation & Docker dev environment](#phase-0)
5. [Phase 1 — Identity: users, API keys, isolation](#phase-1)
6. [Phase 2 — Memories: store + hybrid search (REST)](#phase-2)
7. [Phase 3 — MCP server](#phase-3)
8. [Phase 4 — Understanding: extraction, labeling, reconciliation](#phase-4)
9. [Phase 5 — Consolidation, distillation & profile](#phase-5)
10. [Phase 6 — SDK, docs, packaging & release](#phase-6)
11. [Test strategy summary](#11-test-strategy-summary)
12. [Documentation deliverables matrix](#12-documentation-deliverables-matrix)

---

## 1. How to read this plan

### Task format

Every task follows this template:

- **Goal** — what exists after the task that didn't before.
- **Steps** — ordered implementation procedure.
- **Example** — input/output of the delivered behavior (when it applies).
- **DoD** — checklist that must be fully true to close the task.

Sizes: **S** ≤ half a day · **M** ≈ 1 day · **L** ≈ 2–3 days (focused work).

### Global Definition of Done (applies to EVERY task, in addition to its own DoD)

- [ ] `just check` passes **inside the dev container**: `cargo fmt --check`,
      `cargo clippy -- -D warnings`, `cargo test`.
- [ ] New behavior has tests at the appropriate level (unit for domain logic,
      use-case/integration for features — see §11).
- [ ] Public functions/types touched have rustdoc; user-facing changes update the
      relevant doc in `docs/`.
- [ ] No cross-context imports that violate layer rules (see §2 boundaries).
- [ ] Conventional commit(s), e.g. `feat(memories): hybrid search with RRF`.

### Git workflow

- `main` is always releasable. Each phase gets a branch: `phase/0-foundation`,
  `phase/1-identity`, … Tasks are commits (or short-lived task branches merged into
  the phase branch, your call). Phase ends with a PR `phase/N-… → main`, the phase
  exit checklist passing, and an annotated tag `v0.N.0`.
- CI (Phase 0) gates every PR: fmt, clippy, tests, Docker image build.

### "Shippable at end of phase" means

Someone can `git clone && docker compose up` (or run the binary) and get real value
from the feature set of that phase, guided only by the README as it exists on that
branch. Each phase section below states its shippable outcome explicitly.

---

## 2. Architecture

### Bounded contexts (the folders scream the domain)

| Context | Responsibility | Key language |
|---|---|---|
| `identity` | Users, API keys, authentication, `UserContext` | User, ApiKey, Scope |
| `memories` | Storing, indexing, searching, exporting memories | Memory, Category, Tag, Recall |
| `understanding` | LLM pipeline: extract → reconcile → label | Candidate, Reconciliation (ADD/UPDATE/DELETE/NOOP), Taxonomy |
| `providers` | Concrete LLM/embedding implementations (Anthropic, OpenAI-compat, Ollama, local ONNX) | AnthropicChatModel, FastembedEmbedder — implementations of consumer-owned traits |
| `consolidation` | Background jobs: dedup/merge, decay, distillation, profile digest | ConsolidationRun, Distillation, ProfileDigest |
| `shared` | Shared kernel: ids, error type, clock, pagination — **tiny by design** | MemoryId, UserId, RaError |

### Layers inside each context (vertical slice)

```
<context>/
  domain/           # pure: entities, value objects, atomic domain operations, contracts (traits)
                    #   NO tokio, NO sqlite, NO http, NO serde_json on entities*
  application/      # use cases (one file per use case), DTOs, orchestration
  infrastructure/   # adapters: sqlite repos, tantivy, http handlers, mcp tools,
                    #   provider clients — everything that touches the outside world
```

\* serde on DTOs in `application` is fine; domain stays framework-free so it is
trivially unit-testable.

### Boundary rules (enforced by review + a CI lint script)

1. `domain` imports only `shared` and std.
2. `application` imports its own `domain` + `shared` + public traits of other
   contexts' **application** layer (never their infrastructure).
3. `infrastructure` implements the domain-owned traits; only `bootstrap/` wires
   concrete implementations into use cases (composition root = the only place
   that sees everything).
4. Inbound adapters (axum handlers, rmcp tools, CLI commands) live in the
   infrastructure layer of the context that owns the use case.
5. Contracts are owned by consumers: the `Embedder` trait lives in
   `memories/domain/embedder.rs`; the `ChatModel` trait in
   `understanding/domain/chat_model.rs`; the `providers` context supplies the
   concrete implementations (`FastembedEmbedder`, `AnthropicChatModel`, …).

### Naming conventions (enforced in review + boundary script grep)

- **Traits are role names, implementations are technology-prefixed:**
  `UserRepository` ← `SqliteUserRepository`; `Embedder` ← `FastembedEmbedder`;
  `ChatModel` ← `AnthropicChatModel`. The suffixes `*Port`, `*Service`,
  `*Manager`, and `*Helper` are **banned** — they invite logic dumping and say
  nothing about the role.
- **Use cases are atomic doers:** one small file, one struct named after its
  single job (`UserCreator`, `ApiKeyIssuer`, `MemoryRecaller`), exposing one
  public method — `execute`. If a doer grows a second public method, that's a
  second use case: split the file.
- One trait per file, named after the trait (`user_repository.rs`), never a
  catch-all `ports.rs`/`traits.rs`.

---

## 3. Repository layout

```
recuerdos-ai/
├── Cargo.toml
├── justfile                    # dev commands (all run in Docker)
├── docker/
│   ├── Dockerfile              # multi-stage release image (distroless-ish)
│   └── Dockerfile.dev          # dev image: rust + cargo-watch + tooling
├── docker-compose.yml          # dev service + optional ollama profile
├── recuerdos-ai.example.toml
├── migrations/                 # sqlx/refinery SQL migrations, numbered
├── src/
│   ├── main.rs                 # CLI entry: serve | init | user | key | export | mcp
│   ├── bootstrap/              # composition root: config→wiring→server start
│   ├── shared/                 # shared kernel (ids, errors, clock, test fixtures)
│   ├── identity/
│   │   ├── domain/  ├── application/  └── infrastructure/
│   ├── memories/
│   │   ├── domain/  ├── application/  └── infrastructure/
│   ├── understanding/
│   │   ├── domain/  ├── application/  └── infrastructure/
│   ├── providers/
│   │   ├── domain/  ├── application/  └── infrastructure/
│   └── consolidation/
│       ├── domain/  ├── application/  └── infrastructure/
├── tests/                      # black-box use-case tests (one file per scenario group)
│   ├── common/                 # test harness: spawn app on tmp dir, fake providers
│   ├── identity_isolation.rs
│   ├── memories_recall.rs
│   ├── understanding_pipeline.rs
│   └── mcp_tools.rs
├── docs/
│   ├── api.md                  # REST reference (+ generated openapi.json)
│   ├── mcp.md                  # MCP tools/resources reference
│   ├── configuration.md
│   ├── architecture.md         # this section, kept current
│   ├── integrations/           # claude-code.md, opencode.md, hermes.md, langchain.md
│   └── sdk-python.md
├── sdk/python/                 # Phase 6 thin SDK
└── README.md
```

---

<a name="phase-0"></a>
## Phase 0 — Foundation & Docker dev environment

**Branch:** `phase/0-foundation` · **Size:** ~3–4 days total
**Shippable outcome:** `docker compose up` starts a daemon that loads validated TOML
config, serves `/healthz` and `/version`, logs structured traces; CI is green; the
README explains all of it. It's a real (if empty) service.

### Task 0.1 — Repo scaffold with screaming structure (S)

- **Goal:** cargo project whose folder tree is the §3 layout, compiling with empty
  module stubs, so every later task has an unambiguous home.
- **Steps:**
  1. `cargo init --name recuerdos-ai`; set edition, `rust-version`, lints
     (`[lints]` deny warnings-as-errors in CI profile).
  2. Create the context/layer folder tree with `mod.rs` stubs and one-line module
     docs stating each context's responsibility.
  3. Add `shared/`: `RaError` (thiserror), newtype ids (`UserId`, `MemoryId` as
     UUIDv7 wrappers), `Clock` trait + `SystemClock`.
  4. Add `justfile` with placeholder recipes: `dev`, `check`, `test`, `fmt`.
  5. Add `scripts/check-boundaries.sh`: greps for forbidden imports
     (e.g. `use crate::.*::infrastructure` outside `bootstrap`) — the cheap
     architecture test; wire into `just check`.
- **DoD:**
  - [ ] `cargo build` succeeds; tree matches §3.
  - [ ] Boundary script exists, runs in `just check`, and fails on a deliberate
        violation (prove it in a test commit, then revert).
  - [ ] `shared` unit tests for id round-trips pass.

### Task 0.2 — Docker development environment (M)

- **Goal:** all development happens in containers; a contributor needs Docker only.
- **Steps:**
  1. `docker/Dockerfile.dev`: `rust:1-slim` + build deps (`clang`, `pkg-config`,
     `libssl-dev`), `cargo-watch`, `just`, non-root user matching host UID.
  2. `docker-compose.yml`:
     - service `dev`: mounts repo at `/app`, named volumes for
       `/usr/local/cargo/registry` and `/app/target` (fast incremental builds),
       ports `7070:7070`, `command: cargo watch -x run -- serve`.
     - service `ollama` under `profiles: ["llm"]` (image `ollama/ollama`, volume
       for models) — off by default, started with `docker compose --profile llm up`
       when testing local extraction in Phase 4.
  3. `docker/Dockerfile` (release): multi-stage — builder stage compiles
     `--release`; runtime stage is `debian:bookworm-slim` with just the binary,
     a volume at `/data`, `USER nonroot`, `HEALTHCHECK` hitting `/healthz`.
  4. Point `justfile` recipes at compose: `just dev` = `docker compose up dev`,
     `just test` = `docker compose run --rm dev cargo test`, etc. Every recipe
     also works natively if the user has a local toolchain (compose is the
     default path, not a cage).
  5. Document in README: prerequisites (Docker only), first-run walkthrough.
- **Example:**
  - Input: `git clone … && just dev`
  - Output: container builds, daemon starts, `curl localhost:7070/healthz` → `{"status":"ok"}` (after Task 0.4).
- **DoD:**
  - [ ] Fresh clone on a machine with only Docker reaches a running daemon with
        `just dev`; code edit triggers auto-rebuild via cargo-watch.
  - [ ] `just test`/`just check` run fully inside the container.
  - [ ] Release image builds, is < 150 MB, runs as non-root, healthcheck passes.

### Task 0.3 — Config context: TOML + env overrides + validation (M)

- **Goal:** typed `AppConfig` loaded from `recuerdos-ai.toml`, overridable via
  `RECUERDOS_AI_*` env vars, with precise validation errors. (Lives in
  `bootstrap/config.rs` — it's composition-root concern, not a business context.)
- **Steps:**
  1. Define config structs mirroring project-plan §10 (`[server]`, `[storage]`,
     `[embeddings]`, `[understanding]`, `[consolidation]`, `[retrieval]`, `[auth]`)
     with serde defaults for every field.
  2. Load with `figment`: defaults → file → env (`RECUERDOS_AI_SERVER__PORT=8080`
     style nesting).
  3. Post-parse validation pass returning **all** errors at once
     (`config error: [embeddings].model is empty; [server].port is 0`).
  4. `recuerdos-ai init` CLI subcommand writes `recuerdos-ai.example.toml` and a
     default data dir.
  5. Ship `recuerdos-ai.example.toml` fully commented.
- **Example:**
  - Input: `RECUERDOS_AI_SERVER__PORT=9999 recuerdos-ai serve --config recuerdos-ai.toml`
  - Output: log line `listening on 127.0.0.1:9999 (port overridden by env)`.
- **DoD:**
  - [ ] Unit tests: defaults-only load, file load, env override wins, invalid
        values produce aggregated errors naming the TOML path.
  - [ ] `init` produces a file that then loads cleanly.

### Task 0.4 — HTTP skeleton: axum + tracing + health/version (S)

- **Goal:** running daemon with observability baseline.
- **Steps:**
  1. `bootstrap/server.rs`: axum router, graceful shutdown on SIGTERM/SIGINT
     (Docker-friendly), `tower` layers: request-id, trace, timeout.
  2. `GET /healthz` → `{"status":"ok"}`; `GET /version` → crate version + git sha
     (via `vergen`).
  3. `tracing-subscriber` with env-filter; JSON logs when `RECUERDOS_AI_LOG=json`.
- **DoD:**
  - [ ] Integration test (tests/common harness v0): spawn app on random port,
        assert both endpoints.
  - [ ] `docker stop` exits cleanly < 3 s (graceful shutdown verified).

### Task 0.5 — CI pipeline (S)

- **Goal:** every PR is gated; main is provably green.
- **Steps:** GitHub Actions workflow: jobs `check` (fmt, clippy, boundary script),
  `test` (cargo test in the dev image), `docker` (build release image). Cache
  cargo registry. Badge in README.
- **DoD:**
  - [ ] All jobs green on the phase PR; a deliberately failing test blocks merge.

### Task 0.6 — README v0 + docs skeleton (S)

- **Goal:** the repo explains itself at Phase 0 scope.
- **Steps:** README: what Recuerdos AI is (2 paragraphs from project-plan §1),
  status table (phases + checkmarks), quickstart (Docker), config reference link,
  architecture diagram (project-plan §5), contribution basics. Create empty-but-
  titled `docs/` files from §3 so links never 404.
- **DoD:**
  - [ ] A newcomer following README alone reaches the running daemon.
  - [ ] Phase exit checklist passes → PR → tag `v0.1.0-alpha.0`.

---

<a name="phase-1"></a>
## Phase 1 — Identity: users, API keys, isolation

**Branch:** `phase/1-identity` · **Size:** ~4 days
**Shippable outcome:** multi-user daemon: create users, issue/revoke scoped API
keys via CLI, every HTTP route behind auth middleware that yields a `UserContext`.
Usable as-is as an auth'd skeleton; the isolation test suite exists from day one.

### Task 1.1 — Identity domain model (S)

- **Goal:** pure domain: `User`, `ApiKey` (hash, prefix, scopes, revocation),
  `Scope` (`Read`/`Write`/`Admin`), `UserContext` — the capability token every
  other context will require.
- **Steps:**
  1. `identity/domain/`: entities + invariants (`ApiKey::verify(secret)`,
     `is_revoked`, scope checks). Key format decision recorded in rustdoc:
     `ra_<env>_<prefix8><secret32>`; only argon2 hash stored.
  2. `UserContext { user_id, scopes, key_id }` — **not constructible outside
     `identity`** (private field trick): the type-system isolation guarantee from
     project-plan §11.
  3. Traits `UserRepository` and `ApiKeyRepository`, one file each
     (`identity/domain/user_repository.rs`, `api_key_repository.rs`); their
     concrete `SqliteUserRepository` / `SqliteApiKeyRepository` arrive in Task 1.2.
- **DoD:**
  - [ ] Unit tests: verify/reject key, revoked key rejected, scope logic,
        `UserContext` cannot be built from another module (compile-fail test via
        `trybuild` or a doc-test asserting privacy).

### Task 1.2 — SQLite infrastructure & migrations (M)

- **Goal:** persistence for identity + the project's migration mechanism.
- **Steps:**
  1. Add `rusqlite` + `refinery` migrations; `migrations/V1__identity.sql` for
     `users`, `api_keys` per project-plan §8.
  2. Connection manager in `shared` infrastructure-support: WAL mode, busy
     timeout, single writer connection + read pool.
  3. Implement repositories; argon2 hashing behind `ApiKeyHasher` (fast fake for
     tests).
- **DoD:**
  - [ ] Migrations run automatically at startup on an empty data dir.
  - [ ] Repo round-trip tests against tmpdir SQLite.

### Task 1.3 — Application use cases + CLI (M)

- **Goal:** `UserCreator`, `ApiKeyIssuer`, `ApiKeyRevoker`, `KeyAuthenticator`
  use cases, exposed via CLI (server-side admin actions, no HTTP surface yet).
- **Steps:**
  1. One doer per file in `identity/application/` (`user_creator.rs`, …), each
     exposing a single public `execute`.
  2. CLI subcommands (clap): `recuerdos-ai user add --email …`,
     `recuerdos-ai key issue --user … --scopes read,write`, `key revoke`, `key list`.
     Secret printed **once** at issue time.
- **Example:**
  - Input: `recuerdos-ai key issue --user alex --scopes read,write`
  - Output:
    ```
    API key created for alex (scopes: read, write)
    ra_live_7f3a2b1c9d4e5f6a7b8c9d0e1f2a3b4c   ← shown only once, store it now
    ```
- **DoD:**
  - [ ] Use-case tests with in-memory repo fakes; CLI integration test (spawn
        binary against tmp data dir, parse stdout).

### Task 1.4 — Auth middleware (M)

- **Goal:** axum extractor `Authenticated(UserContext)`: bearer key → prefix
  lookup → argon2 verify → scope check per route; 401/403 with stable error JSON.
- **Steps:**
  1. Tower layer + extractor in `identity/infrastructure/http/`.
  2. Error envelope standard for the whole API:
     `{"error": {"code": "unauthorized", "message": "…"}}`.
  3. `last_used_at` updated async (no write on the hot path).
  4. `auth.mode = "none"` config: single-user opt-out that synthesizes a fixed
     `UserContext` for the built-in `default` user — explicit, logged loudly at
     startup.
- **Example:**
  - Input: `curl -H "Authorization: Bearer ra_live_bad" localhost:7070/v1/ping`
  - Output: `401 {"error":{"code":"unauthorized","message":"invalid API key"}}`
  - Input: valid key → `200 {"user":"alex","scopes":["read","write"]}` (temp `/v1/ping` route, removed in Phase 2).
- **DoD:**
  - [ ] Middleware tests: missing/malformed/revoked/wrong-scope/valid paths.
  - [ ] `tests/identity_isolation.rs` created with the harness helper
        `two_users()` — foundation for every later cross-tenant test.

### Task 1.5 — Phase docs (S)

- **Goal:** `docs/configuration.md` (auth section), README quickstart updated to
  include user/key creation; `docs/api.md` starts with auth + error envelope.
- **DoD:** [ ] README walkthrough works end-to-end on a clean clone. Phase PR →
  tag `v0.1.0-alpha.1`.

---

<a name="phase-2"></a>
## Phase 2 — Memories: store + hybrid search (REST)

**Branch:** `phase/2-memories-core` · **Size:** ~7–9 days
**Shippable outcome:** the first genuinely useful release: store memories and
search them semantically + lexically via REST, fully offline (built-in ONNX
embeddings), strictly per-user. This is "OpenMemory-class" functionality with
Rust speed — before any LLM understanding.

### Task 2.1 — Memories domain model (M)

- **Goal:** `Memory` aggregate + value objects per project-plan §8: content,
  `Category` (closed enum + `Custom(String)` for config-extended values), tags,
  confidence, importance, source, timestamps, `superseded_by`; `RecallQuery` and
  `RecallResult` value objects; scoring policy (RRF + recency/confidence boost) as
  a pure `RecallRanker` (one public method, framework-free) — the most
  unit-testable, most decision-heavy logic in the system lives here.
- **Steps:** entities → `MemoryRepository`, `VectorIndex`, `TextIndex`,
  `Embedder` traits (one file each under `memories/domain/`, every method takes
  `&UserContext`) → `RecallRanker` with table-driven tests.
- **DoD:**
  - [ ] `RecallRanker` unit tests: RRF fusion order, recency half-life math,
        confidence boost, tie-breaking — table-driven, ≥ 15 cases.
  - [ ] Every trait method signature requires `&UserContext` (checked by boundary
        script rule).

### Task 2.2 — Storage infrastructure: SQLite + sqlite-vec (L)

- **Goal:** persistent memory storage with ANN vector search, per-user filtered.
- **Steps:**
  1. `migrations/V2__memories.sql`: `collections`, `memories`, `memory_audit`
     tables; sqlite-vec virtual table `vec_memories(embedding float[384])` with
     `user_id`/`memory_id` aux columns.
  2. Load sqlite-vec extension at pool init; version-pin and verify at startup
     (fail fast with a clear error if the extension is missing from the image —
     add it to both Dockerfiles).
  3. Implement `SqliteMemoryRepository` + `SqliteVecIndex`: insert/update/delete
     keep row + vector in one transaction; KNN query always carries
     `WHERE user_id = ?`.
  4. Audit append on every mutation (`memory_audit`).
- **DoD:**
  - [ ] Round-trip + KNN correctness tests (store 3 vectors, nearest returned).
  - [ ] Cross-tenant test: user B's KNN never returns user A's rows, even with
        identical vectors — added to `tests/identity_isolation.rs`.
  - [ ] Transactionality test: induced failure between row and vector write
        leaves no orphan.

### Task 2.3 — Local embeddings adapter (fastembed/ONNX) (M)

- **Goal:** `FastembedEmbedder`, the local `Embedder` implementation requiring
  zero external services — the "works with no API key" default.
- **Steps:**
  1. `providers/infrastructure/embeddings/fastembed_embedder.rs` using `fastembed-rs`
     (`bge-small-en-v1.5`, 384-dim); model files baked into the Docker image at
     build time (no first-run download inside containers) with a documented
     fallback path for bare-metal (`~/.recuerdos-ai/models`, downloaded on first use).
  2. Batch API (`embed(texts: &[String])`), in-process LRU cache keyed by content
     hash for query embeddings.
  3. Collection metadata pins `embedding_model` + `dimensions`; startup check
     refuses to open a collection whose pinned model ≠ configured model, pointing
     to the (future) `reindex` command.
  4. `FakeEmbedder` for tests: deterministic hash-based vectors — makes every
     use-case test offline and reproducible.
- **Example:**
  - Input: `embed(["user prefers pnpm over npm"])`
  - Output: `[[0.0123, -0.0871, …; 384]]` in < 10 ms warm (local, CPU).
- **DoD:**
  - [ ] Adapter integration test (real model, marked `#[ignore]` in CI-fast lane,
        run in nightly CI job).
  - [ ] Model-pin mismatch produces the designed error message.

### Task 2.4 — Full-text index: tantivy per user (M)

- **Goal:** BM25 leg of hybrid search.
- **Steps:** `TextIndex` adapter: one tantivy index dir per user under the data
  dir; fields `content`, `tags`, `category`; commit batching (flush ≤ 500 ms);
  delete-then-add on update; rebuild command (`recuerdos-ai reindex --text`).
- **DoD:**
  - [ ] Exact-token retrieval test: memory containing `useQuery` found by BM25
        when vector search alone misses it (the motivating use case from
        project-plan §7.7) — this exact scenario as an integration test.
  - [ ] Per-user directory isolation verified.

### Task 2.5 — Use cases: SaveMemory (direct), RecallMemories, CRUD, Export (M)

- **Goal:** application layer for everything Phase 2 ships. `DirectMemorySaver`
  stores verbatim (understanding arrives in Phase 4 behind the same interface —
  callers won't change).
- **Steps:**
  1. One doer per file in `memories/application/`: `direct_memory_saver.rs`,
     `memory_recaller.rs`, `memory_finder.rs`, `memory_updater.rs`,
     `memory_forgetter.rs` (soft delete + audit), `memory_exporter.rs`
     (markdown/JSON) — each exposes a single public `execute`.
  2. `MemoryRecaller` orchestrates: embed query → vector KNN + BM25 in parallel
     (`tokio::join!`) → `RecallRanker` → filter (category/tags/since) →
     limit.
  3. Heuristic tagger for degraded mode (regex/keyword rules → provisional tags,
     `confidence: 0.3`) so even verbatim saves get minimal labels.
- **DoD:**
  - [ ] Use-case tests with fakes for all five, including filter combinations and
        `superseded` exclusion by default.

### Task 2.6 — REST API for memories (M)

- **Goal:** the project-plan §9 endpoints live: `POST /v1/memories:direct`,
  `POST /v1/memories/search`, `GET/PATCH/DELETE /v1/memories/{id}`,
  `GET /v1/memories/export`, `GET /v1/audit`.
- **Steps:**
  1. Handlers in `memories/infrastructure/http/`; DTOs with serde; `utoipa`
     annotations → `openapi.json` served at `/v1/openapi.json`.
  2. Pagination (cursor), consistent error envelope, request validation
     (content length caps, limit caps).
  3. Wire into router in `bootstrap` behind `Authenticated` with scope checks
     (write endpoints need `write`).
- **Example — the Phase 2 core loop:**
  - Input:
    ```bash
    curl -X POST localhost:7070/v1/memories:direct \
      -H "Authorization: Bearer $KEY" -d '{
        "content": "User forbids barrel files / index.ts re-exports",
        "category": "preference.coding", "tags": ["typescript","imports"]}'
    ```
  - Output: `201 {"id":"mem_01J…","category":"preference.coding",…}`
  - Input:
    ```bash
    curl -X POST localhost:7070/v1/memories/search \
      -H "Authorization: Bearer $KEY" \
      -d '{"query":"how should I structure typescript imports?","limit":3}'
    ```
  - Output:
    ```json
    {"results":[{"id":"mem_01J…","content":"User forbids barrel files…",
      "category":"preference.coding","score":0.87,
      "match":{"vector_rank":1,"bm25_rank":2}}],"took_ms":9}
    ```
- **DoD:**
  - [ ] `tests/memories_recall.rs`: scenario tests — semantic paraphrase recall,
        exact-token recall, category filter, cross-tenant blindness, soft-deleted
        memory absent from recall but present in audit.
  - [ ] OpenAPI JSON validates; `docs/api.md` regenerated.
  - [ ] Perf smoke (release build, in Docker): P95 recall < 50 ms at 100k seeded
        memories/user — scripted in `scripts/bench-recall.sh` (`oha`), numbers
        recorded in the PR description.

### Task 2.7 — Phase docs + README upgrade (S)

- **Goal:** README now sells the Phase 2 loop (store → search) with copy-paste
  curl examples; `docs/api.md` complete for shipped endpoints.
- **DoD:** [ ] Clean-clone walkthrough: Docker up → user+key → save 3 memories →
  recall returns the right one. Phase PR → tag `v0.2.0-alpha`.

---

<a name="phase-3"></a>
## Phase 3 — MCP server

**Branch:** `phase/3-mcp` · **Size:** ~4–5 days
**Shippable outcome:** Claude Code (and opencode) connect to Recuerdos AI over MCP
(stdio and streamable HTTP), with `memory_save`, `memory_recall`, `memory_forget`
tools and the `memory://profile` resource. The dogfooding threshold: you use it
daily from here on.

### Task 3.1 — MCP infrastructure with rmcp (L)

- **Goal:** MCP surface as a thin adapter over the *same* application use cases —
  zero business logic in the MCP layer.
- **Steps:**
  1. Add `rmcp`; `memories/infrastructure/mcp/` defines the tool handlers mapping
     to `DirectMemorySaver`, `MemoryRecaller`, `MemoryForgetter`.
  2. Transports: (a) `recuerdos-ai mcp` subcommand → stdio (spawned per-client;
     authenticates via `RECUERDOS_AI_API_KEY` env var, talking to the daemon over
     localhost HTTP — stdio process is a *client shim*, keeping one source of
     truth); (b) streamable HTTP mounted at `/mcp` on the main daemon, bearer-key
     authenticated, same middleware as REST.
  3. Tool descriptions engineered for agent triggering (this is product surface,
     not boilerplate): e.g. `memory_save`: *"Store a durable fact, preference, or
     decision the user stated. Call when the user expresses a lasting preference
     ('I prefer…', 'never do…', 'we decided…'). Not for transient task details."*
  4. `memory_forget` returns matched candidates + requires a second confirmed
     call with ids (never blind-deletes) — per project-plan §9.
- **Example:**
  - Input (agent tool call): `memory_recall {"query": "package manager preference", "limit": 3}`
  - Output (tool result): `1. [preference.coding] User prefers pnpm; never use npm or yarn (score 0.91, saved 2026-06-02 via claude-code)`
    — formatted as compact text, not raw JSON: tool output is agent context, tokens matter.
- **DoD:**
  - [ ] `tests/mcp_tools.rs`: drive the server through rmcp client over an
        in-memory/stdio transport — save → recall round-trip, forget's
        two-step confirmation, auth failure on bad key.
  - [ ] MCP Inspector session documented (screenshot in docs/mcp.md).

### Task 3.2 — `memory://profile` resource (v1: assembled, not LLM-digested) (M)

- **Goal:** session-start context injection. Phase 3 version = deterministic
  assembly (`ProfileAssembler`): top-N by importance per category, grouped,
  markdown, ≤ 1500 tokens.
  (LLM-written digest replaces the internals in Phase 5; the resource contract
  stays identical.)
- **Example:**
  - Input: client reads resource `memory://profile`
  - Output:
    ```markdown
    # Memory profile: alex (updated 2026-07-18)
    ## Coding preferences
    - Prefers pnpm; never npm/yarn
    - Forbids barrel files / index.ts re-exports
    ## Decisions
    - SQLite over Postgres for desktop app (installer size) — 2026-03
    ```
- **DoD:** [ ] Resource test: content reflects stored memories, respects user
  scoping, stays under the token budget with 1k+ memories (truncation by
  importance verified).

### Task 3.3 — Client integration recipes + docs (M)

- **Goal:** `docs/mcp.md` (full tool/resource reference, transport setup) and
  `docs/integrations/claude-code.md` + `opencode.md`: exact `.mcp.json` /
  config snippets, plus a Claude Code **SessionStart hook** example that pulls
  the profile and a **PreCompact/session-end hook** posting a summary to
  `POST /v1/memories` (which until Phase 4 lands = direct store; upgrade is
  transparent).
- **DoD:**
  - [ ] Following claude-code.md verbatim on a clean machine: Claude Code lists
        the 3 tools, saves a preference, recalls it in a *new* session.
  - [ ] Phase PR → tag `v0.3.0-alpha`. **Begin daily dogfooding.**

---

<a name="phase-4"></a>
## Phase 4 — Understanding: extraction, labeling, reconciliation

**Branch:** `phase/4-understanding` · **Size:** ~8–10 days
**Shippable outcome:** the differentiator: `POST /v1/memories` (async) runs
extract → reconcile (ADD/UPDATE/DELETE/NOOP) → label through a configurable LLM
provider (Anthropic / OpenAI-compat / Ollama), with job queue, audit, and
degraded verbatim mode when no provider is configured.

### Task 4.1 — Provider contracts & implementations (L)

- **Goal:** `ChatModel` trait (owned by `understanding/domain`) with three
  implementations in `providers/infrastructure/`: `AnthropicChatModel` (Messages
  API, tool-use JSON output), `OpenAiCompatChatModel` (covers
  OpenAI/OpenRouter/Groq), `OllamaChatModel` (`/api/chat`).
- **Steps:**
  1. Trait method: `complete_structured(prompt, json_schema) -> Result<Value>`;
     retry/backoff and timeout policy live in a `RetryingChatModel` decorator so
     the concrete clients stay dumb.
  2. Provider selection + model + key-env from `[understanding]` config;
     `provider = "none"` → pipeline disabled flag.
  3. `ScriptedChatModel` test double: returns queued canned JSON responses;
     records prompts for assertion.
  4. `OllamaChatModel` tested against the compose `llm` profile (manual/nightly).
- **DoD:**
  - [ ] Contract test suite runs identically against all three implementations
        (recorded/replayed HTTP via `wiremock`), asserting schema-conformant
        output handling, retry on 429/5xx, timeout, malformed-JSON recovery
        (one repair attempt then error).

### Task 4.2 — Job queue: async ingestion (M)

- **Goal:** `POST /v1/memories` returns `202 {job_id}` in < 5 ms; workers process
  reliably with retries; jobs survive restart.
- **Steps:**
  1. `migrations/V3__jobs.sql` (`ingest_jobs` per project-plan §8);
     `JobQueue` trait in `understanding/domain`, concrete `SqliteJobQueue`
     (claim via `UPDATE … WHERE status='pending' RETURNING`, attempt counter,
     exponential backoff, dead-letter status after N attempts).
  2. Worker pool (`tokio` tasks, count configurable) started by bootstrap.
  3. `GET /v1/jobs/{id}` for status polling; job outcome links produced memory ids.
- **DoD:**
  - [ ] Tests: happy path, worker crash mid-job (kill task) → retried, poison job
        → dead-letter with error recorded, restart resumes pending jobs.

### Task 4.3 — Extraction use case (L)

- **Goal:** raw content → 0–N atomic memory candidates with category, tags,
  entities, confidence — the project-plan §7.1 structured-output call.
- **Steps:**
  1. `understanding/domain/`: `Candidate`, `Taxonomy` (defaults + config
     `extra_categories`), prompt assembly as pure functions (testable without LLM).
  2. `understanding/application/candidate_extractor.rs`
     (`CandidateExtractor::execute`): build prompt with taxonomy + source hints →
     `ChatModel` → validate/normalize output (unknown category → nearest default
     + flag; clamp confidence; drop empty candidates).
  3. Prompt files under `src/understanding/prompts/*.md`, included via
     `include_str!` — reviewable, diffable prompts.
- **Example:**
  - Input (raw ingest): `"btw we're switching the whole backend to Hetzner, fly.io got too expensive. Also I want you to always write table-driven tests in Go"`
  - Output (candidates):
    ```json
    [{"content":"Backend infrastructure is hosted on Hetzner (migrated from Fly.io because of cost)",
      "category":"fact.project","tags":["infrastructure","hetzner"],
      "entities":[{"name":"Hetzner","type":"service"},{"name":"Fly.io","type":"service"}],
      "confidence":0.9},
     {"content":"User requires table-driven tests in Go",
      "category":"preference.coding","tags":["go","testing"],"confidence":0.95}]
    ```
- **DoD:**
  - [ ] Prompt-assembly unit tests; pipeline tests with `ScriptedChatModel`
        covering: multi-candidate split, zero-candidate small talk, unknown
        category normalization, taxonomy extension from config.

### Task 4.4 — Reconciliation: ADD / UPDATE / DELETE / NOOP (L)

- **Goal:** the Mem0-style update phase, implemented as the `MemoryReconciler`
  use case — candidates checked against similar existing memories;
  contradictions supersede instead of accumulating.
- **Steps:**
  1. For each candidate: recall top-K similar (reuse `MemoryRecaller`, same-user);
     if none above threshold → ADD without LLM call (cost saver).
  2. Otherwise one `ChatModel` call: candidate + neighbors → decision list
     (`ADD` / `UPDATE(id)` / `DELETE(id)` / `NOOP` + rationale).
  3. Apply transactionally via the `memories` use cases (`MemoryUpdater`,
     `MemoryForgetter`): UPDATE =
     new memory + `superseded_by` back-link on the old; DELETE = soft +
     supersede; all decisions audit-logged with rationale.
- **Example — the contradiction scenario (project-plan §12.3):**
  - Existing: `"Backend deploys on Fly.io"`.
  - Input candidate: `"Backend infrastructure is hosted on Hetzner (migrated from Fly.io…)"`
  - Output: decision `UPDATE(mem_fly)` → old memory superseded (still in audit),
    recall for "where do we deploy?" now returns only Hetzner.
- **DoD:**
  - [ ] `tests/understanding_pipeline.rs` end-to-end (fake providers): ingest →
        job → extraction → reconciliation → recall reflects the merge; scenarios:
        fresh ADD, duplicate → NOOP, contradiction → UPDATE+supersede,
        explicit retraction ("I no longer…") → DELETE.
  - [ ] Superseded memories excluded from recall by default, retrievable with
        `include_superseded=true`.

### Task 4.5 — Wire `POST /v1/memories` + MCP `memory_save` to the pipeline (S)

- **Goal:** public surfaces switch from direct store to async understanding when
  a provider is configured; degraded mode falls back to Phase 2 behavior with
  heuristic tags — same endpoints, zero client changes.
- **DoD:**
  - [ ] Mode matrix test: provider configured / `none` × REST / MCP → correct
        behavior in all four cells.
  - [ ] `docs/api.md` + `docs/mcp.md` updated (202 + job polling documented).

### Task 4.6 — Retrieval-quality eval harness (M)

- **Goal:** guard the whole point of the system: a curated eval set
  (`eval/cases.yaml`: needle-in-haystack, paraphrase, contradiction-current-fact,
  exact-token, category-filter cases) scored on recall@k, runnable offline with
  the fake embedder replaced by the real local model.
- **Steps:** `recuerdos-ai eval` hidden subcommand → seeds a tmp instance, runs
  cases, prints table; nightly CI job posts the score; PR job fails if recall@5
  drops > 5 points vs the committed baseline (`eval/baseline.json`).
- **DoD:** [ ] Baseline committed; a deliberate scoring regression fails the job.
  Phase PR → tag `v0.4.0-alpha`.

---

<a name="phase-5"></a>
## Phase 5 — Consolidation, distillation & profile digest

**Branch:** `phase/5-consolidation` · **Size:** ~5–6 days
**Shippable outcome:** memory that stays clean over time: session distillation
endpoint + MCP tool, nightly dedup/merge consolidation, importance decay + TTL
expiry, and the LLM-written `memory://profile` digest.

### Task 5.1 — Session distillation (M)

- **Goal:** `POST /v1/sessions/distill` + MCP `session_distill`, backed by a
  `SessionDistiller` use case: transcript/
  summary in → few durable memories out through the Phase 4 pipeline with a
  distillation-specific prompt ("extract only what stays true after this
  session ends").
- **Example:**
  - Input: 200-message session summary containing a bug fix, a new convention,
    and lots of back-and-forth.
  - Output: 3 memories (`experience`: bug + root cause; `preference.coding`:
    new convention; `fact.project`: feature now implemented) — verified in test
    that chit-chat produces nothing.
- **DoD:** [ ] Use-case tests (scripted fake): selectivity, category assignment;
  Claude Code PreCompact hook recipe updated in docs/integrations and manually
  verified once.

### Task 5.2 — Consolidation job: dedup/merge (L)

- **Goal:** nightly (config `[consolidation]`) clustering of near-duplicates
  (pairwise similarity ≥ threshold within same user+category) → LLM merge →
  merged memory supersedes members; audit `MERGE` entries; dry-run mode.
- **Steps:** scheduler (tokio interval + `recuerdos-ai consolidate --now [--dry-run]`
  CLI); `ClusterBuilder` as a pure domain type (union-find over similarity
  pairs — heavily unit-tested); merge prompt; transactional apply via a
  `MemoryMerger` use case.
- **DoD:**
  - [ ] Seed 5 phrasings of the same preference → post-run: 1 active merged
        memory, 5 superseded, audit trail complete, recall returns exactly one.
  - [ ] Dry-run mutates nothing (asserted).

### Task 5.3 — Decay & expiry (S)

- **Goal:** `expires_at` honored (pruned by the nightly job), `importance`
  recomputed from access recency/frequency; never silently hard-deletes
  (expired → superseded-style tombstone + audit).
- **DoD:** [ ] Time-travel tests with the `Clock` fake: expiry, decay ranking
  drop, `last_accessed_at` bumping on recall.

### Task 5.4 — LLM profile digest (M)

- **Goal:** replace Task 3.2's `ProfileAssembler` internals with a
  `ProfileDigestWriter` use case — an LLM-maintained digest
  per domain (coding / personal), regenerated when the underlying memory set
  changes (dirty flag), cached; resource contract unchanged; falls back to
  assembly in degraded mode.
- **DoD:** [ ] Digest regenerates only when dirty; stays ≤ budget; degraded-mode
  fallback test. Phase PR → tag `v0.5.0-alpha`.

---

<a name="phase-6"></a>
## Phase 6 — SDK, docs, packaging & release

**Branch:** `phase/6-release` · **Size:** ~5–6 days
**Shippable outcome:** `v0.1.0` public release candidate: Python SDK, complete
docs, integration recipes for all four client types, load-test numbers, binaries +
Docker image + install script.

### Task 6.1 — Python SDK (`sdk/python/recuerdos-ai`) (M)

- **Goal:** thin typed client (httpx + pydantic): `save`, `save_direct`,
  `search`, `get/update/forget`, `distill_session`, `wait_for_job`; plus
  `Recuerdos AIRetriever` (LangChain BaseRetriever) and a LangGraph memory
  example.
- **Example:**
  ```python
  from recuerdos-ai import Client
  ra = Client(base_url="http://localhost:7070", api_key="ra_live_…")
  ra.save("User is vegetarian now")                      # async understanding
  hits = ra.search("dietary restrictions", limit=3)
  print(hits[0].content)   # "User is vegetarian (updated from: loves steakhouses)"
  ```
- **DoD:** [ ] SDK test suite against a dockerized server (compose service in CI);
  `docs/sdk-python.md`; published to TestPyPI.

### Task 6.2 — Integration recipes: Hermes, openClaw/custom, LangChain (M)

- **Goal:** `docs/integrations/hermes.md` (REST tool definition + MCP option),
  `langchain.md` (retriever + agent-tool pattern), `custom-agents.md`
  (plain REST recipe with curl for any framework).
- **DoD:** [ ] Each recipe executed once for real (Hermes via its tool config;
  LangChain example script in `sdk/python/examples/` runs green in CI).

### Task 6.3 — Load & soak test (M)

- **Goal:** evidence for the performance claims: scripted `oha`/`k6` runs
  (recall P95 @ 100k and 1M memories; ingest throughput; 24 h soak with
  consolidation firing) against the release Docker image; results committed to
  `docs/performance.md`.
- **DoD:** [ ] Targets from project-plan §6 met or consciously re-stated with
  measured numbers; memory leak check (RSS flat over soak).

### Task 6.4 — Docs site & README final (M)

- **Goal:** README rewritten as the launch page (problem → 90-second quickstart →
  feature table → benchmarks → security/isolation story); `docs/` completed:
  `architecture.md` (contexts + layer rules + diagram), `configuration.md`
  (every key), `api.md` (full, + hosted openapi.json), `mcp.md`, `security.md`
  (isolation guarantees + threat model paragraph), `CONTRIBUTING.md`
  (Docker-only dev flow, boundary rules), `CHANGELOG.md` (backfilled per phase).
- **DoD:** [ ] Docs review pass: a person who has never seen the repo integrates
  Claude Code + saves/recalls memories in < 15 min using docs alone (test on a
  friend or a clean VM).

### Task 6.5 — Release engineering (M)

- **Goal:** tagged `v0.1.0`: GitHub Actions release workflow → cross-compiled
  binaries (linux x86_64/aarch64, macOS arm64), Docker image pushed to GHCR
  (multi-arch), `install.sh`, Apache-2.0 LICENSE, crate name / repo name / domain
  availability check executed (project-plan §16 name-check).
- **DoD:** [ ] `curl -fsSL …/install.sh | sh` works on a clean Linux VM and mac;
  `docker run ghcr.io/…/recuerdos-ai` serves; tag + GitHub release notes published.

---

## 11. Test strategy summary

| Level | Location | Runs | What it proves |
|---|---|---|---|
| Domain unit | `src/**/domain` `#[cfg(test)]` | every `just check` | scoring math, key verification, taxonomy, cluster building, decay |
| Use-case (app layer, fakes) | `src/**/application` tests | every check | each use case's contract, degraded modes |
| Black-box scenario | `tests/*.rs` via harness (spawn app, tmp dir, fake providers) | every check | **the project-plan §12 scenarios, literally**: preference persistence, contradiction supersede, exact-token recall, isolation, distillation selectivity, forget confirmation |
| Cross-tenant suite | `tests/identity_isolation.rs` (+ proptest) | every check | no query path leaks rows across users — grows every phase |
| Provider contract | wiremock-based, shared suite × 3 adapters | every check | identical behavior across Anthropic/OpenAI-compat/Ollama |
| Real-model integration | `#[ignore]`-tagged | nightly CI | local ONNX embeddings, Ollama profile |
| Retrieval eval | `recuerdos-ai eval` + baseline | nightly + PR gate | recall@k doesn't regress |
| Load/soak | scripts + k6/oha | Phase 6 + pre-release | P95 targets, leak-free |

Harness principles: every test gets an isolated tmp data dir; `FakeEmbedder`
(deterministic) and `ScriptedChatModel` make the full pipeline offline and
reproducible; the `Clock` trait makes time testable; **no test may sleep** —
job completion is awaited via status polling with timeout.

## 12. Documentation deliverables matrix

| Doc | Created | Completed |
|---|---|---|
| README | P0 | P6 (launch rewrite) |
| docs/architecture.md | P0 skeleton | P6 |
| docs/configuration.md | P1 | P6 |
| docs/api.md + openapi.json | P1 (auth) | P4 (async ingest) |
| docs/mcp.md | P3 | P4 |
| docs/integrations/claude-code.md, opencode.md | P3 | P5 (hooks) |
| docs/integrations/hermes.md, langchain.md, custom-agents.md | P6 | P6 |
| docs/sdk-python.md | P6 | P6 |
| docs/security.md, performance.md, CONTRIBUTING, CHANGELOG | P6 | P6 |

---

### Phase exit checklist (template — copy into each phase PR)

- [ ] All task DoDs checked; global DoD green in CI.
- [ ] Shippable-outcome statement verified on a clean clone with Docker only.
- [ ] README/docs updated to current scope; no dead links.
- [ ] Tag `v0.N.0[-alpha]` pushed; CHANGELOG entry written.
- [ ] Dogfood note: one paragraph on what felt wrong while using it (feeds the
      next phase's backlog).
