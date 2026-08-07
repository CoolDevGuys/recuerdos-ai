# Recuerdos AI — Technical Implementation Plan

**v0.2 · updated 2026-08-04 · companion to [project-plan.md](project-plan.md)**

Actionable, phased breakdown of the build. Every phase lives on its own git branch,
ends in a **shippable, usable** state, and every task has a goal, step-by-step
procedure, examples where they apply, and a Definition of Done (DoD).

> **Status (2026-08-04): Phases 0–6 are ✅ SHIPPED.** The build described below is
> complete and released as `v0.1.0` (release candidate) and `v0.1.1`. Those phase
> sections are kept as the **build record** — a faithful account of what was
> designed and delivered, not a to-do list. Forward work lives in
> [Phase 7 — Memory-science performance & capability upgrades](#phase-7); its
> **Tasks 7.1 and 7.2 are now ✅ shipped**, and Task 7.3 (the graph layer) is
> **planned and broken down** — its deferral condition is met, and Task 7.3.0 is the
> eval that decides whether the rest of it gets built.
>
> **Why this file and [project-plan.md](project-plan.md) both exist.** They are not
> merged on purpose: `project-plan.md` is the *product & architecture reference*
> (problem, competitive positioning, strategy choice, data model, the rationale
> behind each decision) and stays a living document; this file is the *execution
> record + forward task breakdown*. Combining them would produce a ~1,500-line
> document and mix "why we chose SQLite" with "the DoD for the tantivy adapter."
> Where they touch the same topic (architecture, data model, taxonomy), this file
> links to `project-plan.md` rather than restating it.

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
11. [Phase 7 — Memory-science performance & capability upgrades (planned)](#phase-7)
12. [Test strategy summary](#12-test-strategy-summary)
13. [Documentation deliverables matrix](#13-documentation-deliverables-matrix)

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
## Phase 0 — Foundation & Docker dev environment ✅ SHIPPED (`v0.1.0-alpha.0`)

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
## Phase 1 — Identity: users, API keys, isolation ✅ SHIPPED (`v0.1.0-alpha.1`)

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
## Phase 2 — Memories: store + hybrid search (REST) ✅ SHIPPED (`v0.2.0-alpha`)

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
## Phase 3 — MCP server ✅ SHIPPED (`v0.3.0-alpha`)

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
## Phase 4 — Understanding: extraction, labeling, reconciliation ✅ SHIPPED (`v0.4.0-alpha`)

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
## Phase 5 — Consolidation, distillation & profile digest ✅ SHIPPED (`v0.5.0-alpha`)

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
## Phase 6 — SDK, docs, packaging & release ✅ SHIPPED (`v0.1.0`, hardened in `v0.1.1`)

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

<a name="phase-7"></a>
## Phase 7 — Memory-science performance & capability upgrades

**Branch:** `phase/7-memory-science` · **Status:** 🚧 IN PROGRESS — Tasks 7.1 & 7.2 ✅ shipped, Task 7.3 planned (breakdown below)
**Shippable outcome (target):** consolidation that spends its LLM budget on the
memories that actually need it, an optional finer label users can filter on, and a
clear, de-risked path to the graph layer — without regressing the dedup guarantees
Phase 5 shipped.

### Provenance & how this supersedes `framework_implementation_plan.md`

This phase absorbs the salvageable parts of `framework_implementation_plan.md`
(a memory-research → computational-analogue exploration) and rejects the parts that
don't survive contact with the code. That file used its own `Phase 1/2/3` numbering
which **collides** with both this file's `Phase 0–6` and `project-plan.md §15`'s
`Phase 0–4`; it is folded in here as `Phase 7` and should be treated as **superseded
by this section.** Keep it only as background reading.

**The reframe that drives every task below.** The framework plan's headline metric —
"25×/100×/125× fewer cosine comparisons" — optimizes the cheap axis. Pairwise cosine
within a category is bounded at `MAX_MEMORIES_PER_CATEGORY = 2000`
(`consolidation_runner.rs`), and the code's own note puts 2 M comparisons at *"a
second or two"* of local CPU. The real cost of a consolidation run is the **LLM merge
call per cluster** (network, latency, tokens); in a future graph phase it's the **LLM
extraction call per memory.** So Phase 7 measures success in *LLM calls avoided and
retrieval precision gained*, never in cosine math.

### What was rejected from the framework plan, and why

- **"Rank/skip by `importance`" (its Task 1.1/1.2).** Fresh memories start at
  `importance = 1.0` (`memory.rs`: *"Starts at 1.0"*, pinned by a ranker test), and
  importance only drops after a nightly rescore. Skipping `importance > 0.80` from
  clustering would skip **every newly-created memory** — exactly where duplicates come
  from (the canonical test saves the same line 5×). It would silently regress the
  Phase 5 dedup guarantee. The spaced-repetition analogy is mapped onto the wrong job:
  consolidation here is *deduplication*, whose value tracks *recency of creation*, not
  *decay*. Task 7.1 below keeps the useful half (budget + skip unchanged work) and
  drops the importance ordering.
- **SVD-RAG digests + hierarchical beam search (its Phase 2).** The motivating example
  ("Personal contains family, health, career…") describes a taxonomy that doesn't
  exist: the real set is 8 already-granular types (`preference.personal`,
  `fact.person`, …) that is *already user-extensible* via `Category::Custom` /
  `[understanding.taxonomy].extra_categories`. Pulling in `linfa`/LAPACK for SVD also
  cuts against the single-static-binary, zero-egress identity. And a `profile_digests`
  table already exists (migration V5). Task 7.2 keeps only the cheap, high-value core.
- **Its "zero LLM cost / 100% saved" and "2×/3× precision" numbers.** These compare
  against a RAPTOR/graph baseline that isn't in the codebase (today's baseline is
  *zero*), so the percentages are framing, not measurement. Replaced with the eval
  harness (`recuerdos-ai eval`, Phase 4.6) as the arbiter — no capability lands here
  without a recall@k number beside it.

### Task 7.1 — Budget-aware consolidation + skip-unchanged work (M) ✅ DONE

- **Goal:** a nightly run that never blows past a configured LLM/time budget and never
  re-does work on memories nothing has touched since the last pass — cutting merge-call
  volume **without** the freshness regression the framework plan's importance-skip
  would cause.
- **Steps:**
  1. `[consolidation].budget` in config: `max_llm_calls` (default 100),
     `max_duration_secs` (300), `max_memories` (5000). `ConsolidationRunner::execute`
     tracks consumed budget and stops cleanly at the limit.
  2. Report `budget_exhausted: bool` + `reason` in `ConsolidationReport` (extends the
     existing report struct; surfaced in the CLI/dry-run output).
  3. **Skip heuristic that is safe by construction:** exclude a group's memories
     from re-clustering only when *nothing in that group changed since the last
     successful run* (compare against a stored max `updated_at`). New and edited
     memories always enter clustering; a stable, untouched group is skipped whole.
     This is the genuine win — it removes redundant LLM merge attempts on clusters
     already resolved — and it *cannot* skip a fresh duplicate, because a fresh save
     bumps the group's max `updated_at`.
  4. Order remaining work **largest-group-first** so the budget is spent where
     duplicates are densest, not by importance.
- **DoD:**
  - [x] Budget-exhaustion test: induce the cap mid-run, assert a partial-but-consistent
        report and no half-applied merge.
        (`budget_exhaustion_stops_the_run_and_reports_why`)
  - [x] **Regression guard (the load-bearing test):** save the same line 5×, run
        consolidation → still exactly 1 merged memory, 5 retired (Phase 5's guarantee
        holds under the new skip path).
        (`fresh_duplicates_still_merge_with_a_state_store`, plus the original
        `duplicates_become_one_memory_and_the_originals_are_retired`)
  - [x] Skip test: a group untouched since the last run makes **zero** LLM calls on
        the next run; touching one memory in it re-enables clustering for that group.
        (`an_unchanged_group_is_skipped_on_the_next_run`, `touching_a_group_re_enables_it`)
  - [x] No change to merge/cluster *logic* — only order and scope. Existing
        `consolidation_runner` tests pass unmodified.

- **As built (2026-08-05):**
  - **Watermark grain is `(user_id, category, subcategory)`**, not the plan's original
    `(user_id, category)` — because Task 7.2 clusters within a `(category,
    subcategory)` group, so the skip watermark has to match that grain or it would
    compare a subcategory's max against a whole category's. Migration
    `V6__consolidation_state.sql` was updated accordingly (empty-string sentinel for
    "no subcategory" so the primary key behaves).
  - **`updated_at` is the change signal, and it is trustworthy here:** nightly
    rescoring (`set_importance`) and recall bookkeeping (`touch_accessed`) deliberately
    do **not** bump it (verified in `sqlite_memory_repository`), so only a create /
    edit / supersede lifts a group's maximum above its watermark.
  - **The watermark is written only after a successful pass:** a group whose merge
    errored, or any group when the run stopped early on the budget, is left
    unrecorded so its duplicates are retried next run rather than skipped forever.
    Guarded by `a_failed_merge_is_retried_next_run_not_skipped`.
  - **New code:** `consolidation/domain/consolidation_state.rs` (`ConsolidationStateStore`
    trait), `consolidation/infrastructure/sqlite_consolidation_state_store.rs`
    (+ its own unit tests), skip/record logic and the budget struct in
    `consolidation/application/consolidation_runner.rs`, wired in
    `bootstrap/consolidation_wiring.rs`. `just check` (fmt, clippy `-D warnings`,
    boundary script, full test suite) is green.

### Task 7.2 — Optional finer sub-label (rescoped "chunking") (M) ✅ DONE

- **Goal:** let a memory carry an optional, open-ended sub-label under its category
  (`preference.coding / testing`, `fact.person / family`) that recall and consolidation
  can scope to — the useful 20% of the framework plan's Phase 2, with none of the SVD,
  beam-search, or new-table machinery.
- **Steps:**
  1. A dedicated `subcategory: Option<String>` on `Memory` + a nullable column
     (migration `V7__subcategory.sql`), normalized (lowercase/trim/empty → `None`) and
     length-capped in the domain model.
  2. Understanding pipeline (Phase 4) *suggests* a sub-label during extraction; it is
     never required and never blocks a save. Degraded/verbatim mode leaves it `None`.
  3. `MemoryRecaller` gains an optional sub-label filter (`RecallQuery.subcategories`,
     OR-ed), and `ConsolidationRunner` clusters within `(category, subcategory)`
     groups (smaller n, tighter merges), falling back to category-level when absent.
- **DoD:**
  - [x] Eval case: a query targeting one sub-label returns its memories ahead of
        sibling sub-labels (`subcategory filter: tooling only` in `eval/cases.toml`,
        with seeded `tooling`/`structure`/`testing` sub-labels).
  - [x] Consolidation with sub-labels never merges across two different sub-labels of
        the same category (`memories_with_different_subcategories_are_never_clustered_together`).

- **As built:** the `subcategory` column was shipped rather than deferred behind a
  "tags first" experiment — a deliberate choice, since the sub-label is a distinct
  filter/cluster axis from free-form tags (tags AND-narrow; a sub-label is the single
  grain consolidation groups by). Wired end to end: `memories/domain/memory.rs`,
  `recall_query.rs`, `memory_recaller.rs`, `sqlite_memory_repository.rs`,
  `sqlite_reindexer.rs`, the understanding extraction/reconciliation/merge prompts,
  the HTTP DTOs/handlers, and the eval harness.

### Task 7.3 — Graph layer (Strategy B): bi-temporal entity graph + graph-hop recall (L)

- **Goal:** the entity/relation graph the framework plan's "method of loci" describes is
  **already the roadmap's Strategy B** (`project-plan.md §4, §15 "Phase 3 — Scale &
  moat"`). This section is the guardrails *and* the task breakdown — not a parallel
  design. 7.1 and 7.2 have landed, so the deferral condition is met; **7.3.0 below is
  the eval that decides whether the rest is built at all.**
- **Guardrails for whoever builds it:**
  1. **Keep bi-temporality.** Strategy B's differentiation is `valid_from`/`invalid_at`
     facts that answer *"what DB did we use **before** the migration?"*. The framework
     plan's schema dropped the temporal columns, leaving a commodity co-occurrence
     graph. Don't. Model time.
  2. **Build on the `Entity` that already exists.** `memory.rs` already carries
     `Entity { name, kind }` on every memory, added deliberately so the graph layer
     doesn't require re-running an LLM over the whole corpus. The framework plan's
     normalized `memory_entities` table ignored it — reconcile with the existing field
     instead of duplicating it.
  3. **Budget the extraction.** `project-plan.md` flags that graph extraction *"burns
     significantly more LLM tokens per ingest"* — "one-time cost" understates it. Reuse
     Phase 7.1's budget machinery; extract in the background job, not on the hot path.
  4. **SQLite-first, measured.** Start with SQLite edge tables + app-side PPR; only
     reach for Neo4j/Memgraph if a hop-latency eval demands it (Strategy B already says
     so).
- **DoD (the phase-level bar, decomposed into 7.3.0–7.3.6 below):** graph-hop retrieval
  beats semantic-only on a relational-query eval set by a *measured* margin, ingest stays
  inside its token budget, and bi-temporal supersedence is covered by a time-travel test.

**Branch:** `phase/7.3-graph` · **Size:** ~8–12 days · **Migrations:** V8 (next free)

#### Decisions taken before the breakdown (and why)

These six are settled so the tasks below have unambiguous homes. Each one is a fork that
would otherwise be re-litigated mid-build.

1. **The graph lives inside the `memories` context — no new bounded context.**
   `EntityGraph` is a *third retrieval index*, and its two siblings (`VectorIndex`,
   `TextIndex`) are already consumer-owned traits in `memories/domain/` with adapters in
   `memories/infrastructure/`. A peer `graph/` context would have exactly one consumer
   (recall) and would force `memories/application` to import another context's
   application layer for the hot read path. Relation *extraction* stays in
   `understanding`, which is where entity extraction already lives.
2. **Relations ride the extraction call that is already being made.** Guardrail 3 says
   budget the extraction; the cheapest budget is zero. `CandidateExtractor` already makes
   one structured call per ingest that returns `entities` — adding an optional
   `relations` field to that same schema costs no additional round trip. A separate
   extraction pass per memory is the design the framework plan implied and the one
   `project-plan.md` warns "burns significantly more LLM tokens per ingest."
3. **`memories.entities` (JSON) stays the source of truth; V8 adds a *derived* index.**
   This is guardrail 2 made concrete. `memory_entities` is a projection of the JSON
   column, rebuildable at any time with **zero** LLM calls — which is precisely what
   makes the existing corpus backfillable (7.3.5) and what carrying `Entity` since
   Phase 4 was for. The framework plan's normalized table replaced the field; this one
   indexes it.
4. **Graph hits enter ranking as a third RRF leg, guarded by an empty-leg identity
   proof.** RRF generalizes to N lists for free (`sum 1/(k+rank)`), so the leg needs no
   new constant and no retune of `RRF_K`/`MULTIPLIER_FLOOR` — whose calibration
   `recall_ranker.rs`'s module docs warn is load-bearing. The safety property is that
   **when a query produces no entity seeds the graph leg is empty and the ranking is
   bit-identical to today's**, which makes "non-relational recall cannot regress" a test
   rather than a hope.
5. **Hop expansion before PPR.** Guardrail 4 names app-side PPR; this plan deliberately
   starts one notch simpler with depth-bounded hop expansion. PPR is the answer to *which
   of thousands of reachable nodes matter*, and at this scale (bounded by
   `MAX_MEMORIES_PER_CATEGORY = 2000` and a 2-hop cap) that set is small enough to rank
   by hop distance and edge count. PPR lands only if 7.3.6's eval shows hop expansion
   returning too much.
6. **Two clocks, never conflated.** `memories.created_at/updated_at/superseded_by` is
   *transaction* time — when we learned it. `memory_relations.valid_from/invalid_at` is
   *valid* time — when the fact was true. Strategy B's entire differentiation is the
   second one; collapsing it into the first is how a bi-temporal graph quietly becomes a
   commodity co-occurrence graph (exactly what guardrail 1 forbids).

---

#### Task 7.3.0 — Relational eval set + the go/no-go measurement (S) ✅ DONE

- **Goal:** a `relational` slice of `eval/cases.toml` and a recorded semantic-only score,
  so the rest of 7.3 is justified — or cancelled — by a number instead of by the roadmap.
  **This task ships and is evaluated on its own; nothing below starts until its number is
  on the table.**
- **Steps:**
  1. Extend `SeedMemory` in `src/bootstrap/eval.rs` with `entities` (and `relations`,
     parsed but unused until 7.3.2). Today's seeds carry only
     `content/category/subcategory/tags`, so no eval case *can* exercise a graph.
  2. Add ~10 corpus memories forming a chain worth hopping — the Fly.io → Hetzner
     migration, who reviews auth, which service the query wrapper belongs to — entity-
     linked rather than lexically overlapping.
  3. Add 6–8 `kind = "relational"` cases whose answer needs a hop and whose wording
     shares no useful tokens with the target: *"what did we run on before the current
     host"*, *"who reviews changes to the thing Sam owns"*.
  4. Run `recuerdos-ai eval`; commit the resulting `by_kind.relational` into
     `eval/baseline.json`.
- **DoD:**
  - [x] `by_kind.relational` recorded in the committed baseline. **71.4%** (5 of 7
        cases already answered by semantic-only recall).
  - [x] The existing cases score exactly as before — verified by a before/after run on
        the pre-7.3.0 corpus: every existing kind is identical (`needle` 66.7%, all
        others 100%).
  - [x] **Go/no-go recorded (below).** 71.4% is under the ~80 shelf-it line, so it is a
        (marginal) **GO** — but the number is inflated by relational cases the small
        corpus lets semantic recall solve anyway. The real graph gap is the two clean
        two-hop, vocabulary-disjoint cases that miss.

- **As built (2026-08-07):**
  - **The measurement.** `recall@5` overall 87.5% across 24 cases; `relational` 71.4%.
    The two failing relational cases are the genuine two-hop ones whose target shares no
    vocabulary with the query — *"who should I ask about a billing service decision"* →
    `Nadia is the tech lead of the Meridian team` (billing service → Meridian team →
    Nadia), and the Lisbon-office sibling. The other five "relational" cases pass under
    semantic-only recall because at a 33-memory corpus the target is usually already in
    the top 5. **Read the 71.4% as "the gap is real but narrow at personal scale, and
    concentrated in true multi-hop, disjoint-vocabulary queries"** — not as a green light
    on its own. A larger/harder relational corpus would sharpen the signal before
    committing to 7.3.4.
  - **`SeedMemory` now carries `entities` and `relations`** (`src/bootstrap/eval.rs`);
    entities flow onto the seeded `Memory` (retrieval ignores them today — tantivy
    indexes only content/tags/category and the embedding is over content, so seeding them
    cannot move a score), relations are parsed but unread until 7.3.2. The corpus-parse
    test now also asserts every relation endpoint is a declared entity of its memory —
    the anchoring rule 7.3.2 will enforce in the pipeline, applied here to the
    hand-written seeds so a `Fly.io`/`Flyio` typo fails `cargo test`, not the eval.
  - **Corpus:** +11 entity-linked memories in four chains (billing/Meridian/Nadia/
    Lisbon/ledger-db, mobile-app/notifications/Theo, checkout/Stripe/PCI,
    analytics-pipeline/Kafka→Kinesis/Priya) and +7 `kind = "relational"` cases in
    `eval/cases.toml`.
  - **One production change was required to hold "existing cases unchanged", and it is
    not cosmetic:** adding the corpus pushed the vague `tag filter: typescript` case
    (query `"conventions"`, a weak match for its target) out of the post-fetch candidate
    window, dropping it 100%→0%. This is the over-fetch limitation
    `memory_recaller.rs` documents, triggered by the corpus crossing `candidate_depth`.
    Fixed at the source: `RecallQuery::candidate_depth` floor raised 20→40 (`* 4`→`* 8`)
    in `src/memories/domain/recall_query.rs`. Widening the window only adds lower-ranked
    candidates to ranking — it can never displace a top result — so every existing
    unfiltered case is byte-unchanged and filtered recall strictly improves; the
    before/after run confirms it. The scalable fix (filters pushed into both indexes)
    stays future work; this floor is the honest interim at personal scale.
  - `just check` (fmt, clippy `-D warnings`, boundary script, full suite) is green.

#### Task 7.3.1 — Graph schema + domain contract (M) ✅ DONE

- **Goal:** the storage and the contract, wired but inert — `[graph].enabled = false`,
  recall byte-identical to today.
- **Steps:**
  1. `migrations/V8__entity_graph.sql`:
     - `memory_entities(user_id, memory_id, entity_key, name, kind)` — the derived
       projection of `memories.entities`. Unique on `(user_id, memory_id, entity_key)`,
       indexed on `(user_id, entity_key)`.
     - `memory_relations(id, user_id, memory_id, subject_key, predicate, object_key,
       subject_name, object_name, valid_from, invalid_at, invalidated_by)`. `valid_from`
       NOT NULL; `invalid_at IS NULL` means *currently true*. Indexed on
       `(user_id, subject_key, invalid_at)` and `(user_id, object_key, invalid_at)`.
     - Both `ON DELETE CASCADE` from `users`, matching V2.
  2. `memories/domain/entity_key.rs` — the canonicalization value object: lowercase,
     trim, collapse whitespace, strip trailing punctuation and possessives.
     **The highest-risk piece in the whole task**: if `Fly.io`, `fly.io` and `Fly` don't
     land on one key, every hop below is noise. Pure, framework-free, table-driven tests.
     No alias table in this task — record it as the known limitation.
  3. `memories/domain/entity_graph.rs` — the consumer-owned trait, beside
     `vector_index.rs` and `text_index.rs`. Every method takes `&UserContext` (the
     boundary script enforces this):
     `record(context, memory_id, entities, relations, valid_from)`,
     `remove(context, memory_id)`,
     `neighbours(context, seeds, hops, as_of, limit) -> Vec<MemoryId>`,
     `invalidate(context, edges, at, by)`.
  4. `memories/infrastructure/sqlite_entity_graph.rs`. Graph writes join the **same
     transaction** as the memory row and its vector — the rule Task 2.2 set, for the same
     reason.
  5. Wire in `bootstrap/memories_wiring.rs` behind `[graph].enabled`, default **false**.
- **DoD:**
  - [x] Round-trip and 1-/2-hop tests against SQLite
        (`a_memorys_entities_and_edges_round_trip...`,
        `a_two_hop_neighbour_is_out_of_reach_at_one_hop`).
  - [x] `EntityKey` table-driven tests, 12 cases plus three more, including the `Fly.io`
        family and a case pinning that `Fly`/`flyio`/`fly.io` stay separate (the known
        alias-table limitation).
  - [x] Cross-tenant test — **relocated to `storage_tests.rs`, not
        `tests/identity_isolation.rs`** (see as-built): user B's hop never reaches A's
        edges even when both store the identical entity name, and `remove` is scoped too.
  - [x] Orphan test: `remove` clears both tables; a temp-trigger–induced failure on the
        edge insert rolls back the entity rows written before it
        (`a_failed_edge_write_rolls_back_the_entities_written_before_it`).
  - [x] Flag off ⇒ recall unchanged: default build has no graph
        (`the_default_build_has_no_graph_and_enabling_it_wires_one`), recall never
        references it, and the 7.3.0 eval baseline is untouched.

- **As built (2026-08-07):**
  - **Migration `V8__entity_graph.sql`** adds `memory_entities` (PK
    `(user_id, memory_id, entity_key)`, index `(user_id, entity_key)`) and the bi-temporal
    `memory_relations` (indexed `(user_id, subject_key, invalid_at)` and the object mirror),
    both `ON DELETE CASCADE` from `users`. **No FK to `memories`** — a memory is
    soft-deleted so a cascade would never fire; the rows are dropped explicitly by
    `EntityGraph::remove`, the same reasoning `memory_audit` records.
  - **`EntityKey`** (`memories/domain/entity_key.rs`) normalizes case, whitespace, a
    trailing possessive (`'s`/curly) and trailing sentence punctuation, while keeping an
    internal dot (`fly.io`) intact. It deliberately does **not** unify `Fly`/`flyio` with
    `fly.io` — that needs the alias table held as the escape hatch — and a test pins that
    so a future alias step is a deliberate change, not an accident.
  - **`EntityGraph` trait** (`memories/domain/entity_graph.rs`, beside `vector_index.rs`)
    with `record`/`remove`/`neighbours`/`invalidate`, each taking `&UserContext`. The
    concrete `SqliteEntityGraph` traverses **app-side, one hop at a time** rather than a
    recursive CTE: correct and readable at a 2-hop, personal-scale corpus, and 7.3.4 is
    where the CTE earns its cost on the hot path. `invalidate` (a 7.3.3 driver) and the
    `as_of` bi-temporal read are implemented and tested now so 7.3.3 is just the use case
    that calls them.
  - **Two deviations from the written steps, both deliberate:**
    - **The "same transaction as the memory row" (step 4) is not literal, because the
      codebase's write path is not literal either.** `DirectMemorySaver` already writes the
      row, the vector and the text index as *separate* `with_connection` calls with
      compensating rollback — there is no single memory-row-plus-vector transaction to join.
      So `record`/`remove` are each atomic **within themselves** (entities + edges in one
      transaction), and the write-path integration — where the saver/ingestor calls
      `record` and compensates like it does for the vector — lands in **7.3.2** alongside
      the relations it will carry. In 7.3.1 the store is constructed but not yet called
      outside tests.
    - **The cross-tenant test lives in `storage_tests.rs`, not `tests/identity_isolation.rs`.**
      The latter is a black-box HTTP suite (`TestApp`, `reqwest`) and cannot forge a
      `UserContext` or reach an inert store with no HTTP surface. The vector index's
      isolation test lives in `storage_tests.rs` for exactly this reason; the graph's joins
      it. A black-box case joins `identity_isolation.rs` in 7.3.4, when recall exposes the
      hop over HTTP.
  - **Because the store is inert (its callers are 7.3.2/7.3.4), the crate build sees the
    subsystem as dead code.** Rather than pass `&[]` through a half-wired saver — which
    would still leave `neighbours`/`invalidate` dead — the three new graph files carry a
    documented module-level `#![allow(dead_code)]` that comes off as each caller lands.
    The `Memories.graph` field is `Option<Arc<dyn EntityGraph>>`, `Some` only when
    `[graph].enabled` (default off), `#[allow(dead_code)]` until recall reads it.
  - **Config:** `[graph]` (`enabled` default false, `max_hops` 2, `hop_limit` 50), with
    validation that rejects a zero hop budget only when enabled; documented in
    `recuerdos-ai.example.toml`.
  - `just check` (fmt, clippy `-D warnings`, boundary script, full suite — 9 new graph
    tests) is green.

#### Task 7.3.2 — Relations from the extraction call already being made (M)

- **Goal:** every new ingest produces edges at **zero additional LLM calls**.
- **Steps:**
  1. Add an **optional** `relations: [{subject, predicate, object}]` to the extraction
     schema (`understanding/domain/extraction_prompt.rs`) and a bullet beside the
     existing `entities` one in `prompts/extraction.md` and `prompts/distillation.md`.
     Optional in the strict sense: a model that omits it must still validate — the same
     reasoning already recorded for tags and entities.
  2. `RawRelation → Relation` normalization in `understanding/domain/candidate.rs`,
     mirroring `normalise_entities`: drop when subject or object is empty, snake-case the
     predicate, cap per candidate, and **drop relations whose endpoints aren't in the
     candidate's own `entities`** — that anchoring is what keeps a hallucinated third
     party out of the graph.
  3. Carry `Candidate.relations` through `memory_ingestor` to the edge write.
  4. Config `[graph]`: `enabled`, `extract_relations`, `max_relations_per_memory`,
     `max_hops`, `hop_limit`, `backfill_budget` (7.3.5).
- **Example:**
  - Input: `"we're switching the whole backend to Hetzner, fly.io got too expensive"`
  - Output (added to the existing candidate):
    ```json
    "relations": [{"subject":"Hetzner","predicate":"hosts","object":"backend"},
                  {"subject":"backend","predicate":"migrated_from","object":"Fly.io"}]
    ```
- **DoD:**
  - [ ] `ScriptedChatModel` tests: relations present, relations absent, dangling endpoint
        dropped, predicate normalized, per-memory cap enforced.
  - [ ] **Token cost measured, not assumed:** prompt + completion tokens over the eval
        corpus, before vs after, recorded in the PR. If completion tokens rise > 20%,
        `extract_relations` ships default-off and the number says why.
  - [ ] Degraded/verbatim mode produces no relations and still saves.
  - [ ] `tests/understanding_pipeline.rs` passes **unmodified** — extraction quality did
        not move.

#### Task 7.3.3 — Bi-temporality: valid time, and who invalidates whom (M)

- **Goal:** the property that makes this Strategy B and not a co-occurrence graph —
  *"what did we deploy on **before** the migration?"* answerable.
- **Steps:**
  1. Record decision 6 (the two clocks) in rustdoc on both `Memory` and the edge type.
  2. `valid_from` defaults to the asserting memory's `created_at`. An LLM-supplied
     explicit validity date is **out of scope** here — note it as the follow-up.
  3. `memories/application/edge_invalidator.rs` — one use case, one `execute`. When
     memory B supersedes memory A (Phase 4 reconciliation `UPDATE`/`DELETE`, Phase 5
     merge), every edge A asserted whose `(subject_key, predicate)` B re-asserts with a
     **different** `object_key` gets `invalid_at = B.valid_from`, `invalidated_by = B.id`.
     Edges A asserted that B does not contradict stay valid — superseding a *memory* is
     not blanket invalidation of everything it ever said, and treating it as such is the
     easy way to erase history the audit trail promises to keep.
  4. `as_of` on the read path: an edge is live at `T` when
     `valid_from <= T AND (invalid_at IS NULL OR invalid_at > T)`.
- **Example:**
  - `mem_fly` asserts `backend —deploys_on→ Fly.io` (valid from 2026-01).
  - `mem_hetzner` supersedes it, asserting `backend —deploys_on→ Hetzner` (valid from
    2026-06). The Fly.io edge gets `invalid_at = 2026-06`, not deleted.
  - `as_of = 2026-03` → Fly.io. Bare query → Hetzner.
- **DoD:**
  - [ ] **Time-travel test (load-bearing)**, driven by the `Clock` fake: the Fly.io edge
        is live before the migration instant, invalid after, and an `as_of` read from
        before it returns the Fly.io memory.
  - [ ] A non-contradicting edge survives its memory being superseded.
  - [ ] Invalidation is idempotent — re-running reconciliation never moves an existing
        `invalid_at`.

#### Task 7.3.4 — The graph-hop leg in recall (L)

- **Goal:** graph evidence can lift the right memory into the top 5 — the only version of
  this that moves 7.3.0's number.
- **Steps:**
  1. **Seeding costs no LLM call:** scan the query text for `memory_entities.entity_key`
     matches for this user, using the same `EntityKey` canonicalization the writer used.
     No seeds → empty leg → today's behaviour, exactly.
  2. `EntityGraph::neighbours`: depth-bounded recursive CTE from the seeds, up to
     `[graph].max_hops` (default 2), `as_of`-filtered, ordered by hop distance then edge
     count, capped at `hop_limit`.
  3. Generalize `RecallRanker::rank` from `(vector, keyword)` to N ranked lists;
     `MatchDetail` gains `graph_rank: Option<usize>`. `RRF_K` and `MULTIPLIER_FLOOR` are
     **not** touched: this task adds a leg, it does not retune the bands.
  4. `MemoryRecaller::execute` runs the third leg alongside the other two, unions its ids
     into `candidate_ids`, and degrades exactly as the keyword leg does — warn and
     continue, because two thirds of a hybrid search still answers the question.
  5. `RecallQuery.as_of`, surfaced on `POST /v1/memories/search` and MCP `memory_recall`;
     `match.graph_rank` in the response DTO.
- **DoD:**
  - [ ] **Empty-leg identity test (load-bearing):** with no seeds, `rank` returns
        identical ordering *and* identical scores to the two-leg call. This is what makes
        "non-relational recall cannot regress" provable.
  - [ ] The existing `recall_ranker` table-driven tests pass **unmodified**.
  - [ ] Hop test: a memory that neither vector nor BM25 ranks at all, reachable only over
        two relations, enters the top 5.
  - [ ] Cross-tenant: the graph leg never seeds from, nor hops into, another user's rows.
  - [ ] Perf: P95 recall stays < 50 ms at 100k memories/user with the leg **on**
        (`scripts/bench-recall.sh`), numbers in the PR — the Phase 2 target holds or the
        leg does not ship on by default.

#### Task 7.3.5 — Backfill, budgeted (M)

- **Goal:** an existing corpus gains a graph without a surprise provider bill.
- **Steps:**
  1. `recuerdos-ai graph backfill --entities` — rebuilds `memory_entities` from the
     `entities` JSON already on every memory. **Zero LLM calls.** Every existing install
     gets seeds and one-hop co-occurrence for free; this is the dividend for having
     carried `Entity` since Phase 4.
  2. `--relations` — re-extracts relations for memories that have none, as a background
     job through the existing queue, reusing 7.1's `BudgetLimits` / `ConsolidationBudget`
     verbatim (`[graph].backfill_budget`) and resuming from a watermark in the same
     spirit as `ConsolidationStateStore`.
  3. `--dry-run` reports memories, edges and model calls it *would* make, before anything
     is spent.
- **DoD:**
  - [ ] Entity backfill over a seeded corpus makes zero provider calls — asserted with a
        chat model that panics if invoked.
  - [ ] Relation backfill stops cleanly at its budget and resumes where it stopped on the
        next invocation.
  - [ ] Dry-run mutates nothing (same assertion style as Phase 5's).

#### Task 7.3.6 — Measure, document, decide (M)

- **Goal:** the number that justifies the phase, and the docs that make it usable.
- **Steps:** re-run `recuerdos-ai eval`; record the relational delta **and** that no other
  `by_kind` moved; update `docs/api.md` (`as_of`, `graph_rank`), `docs/mcp.md`,
  `docs/configuration.md` (`[graph]`), `docs/architecture.md` (third leg + the two
  clocks); flip Strategy B's status in `project-plan.md` §4 and §15.
- **DoD:**
  - [ ] Relational recall@5 beats the 7.3.0 baseline by a **measured** margin, recorded in
        the PR and in `eval/baseline.json`.
  - [ ] Every other `by_kind` stays inside the existing `--max-drop 5` gate.
  - [ ] `[graph].enabled` defaults to true **only if** both hold; otherwise it ships
        default-off with the number stated plainly.
  - [ ] Phase exit checklist → PR → tag `v0.2.0`.

#### 7.3 sequencing & risk

| Task | Effort | Risk | Blocks | Real payoff |
|---|---|---|---|---|
| 7.3.0 Relational eval + go/no-go | S | Low | everything | ✅ Done — relational 71.4% (marginal GO; gap concentrated in true 2-hop cases) |
| 7.3.1 Schema + `EntityGraph` contract | M | Low–Med | 7.3.2–5 | ✅ Done — V8 tables, `EntityKey`, `EntityGraph`, `SqliteEntityGraph`; inert behind `[graph].enabled=false` |
| 7.3.2 Relations in the existing call | M | Med | 7.3.3, 7.3.4 | Edges at zero extra LLM calls per ingest |
| 7.3.3 Bi-temporality | M | Med | 7.3.4 | The actual Strategy B differentiator |
| 7.3.4 Graph-hop leg | L | **High** | 7.3.6 | Graph evidence reaches the top 5 |
| 7.3.5 Budgeted backfill | M | Low | — | Existing corpora get a graph, cheaply |
| 7.3.6 Measure + docs | M | Low | — | The margin, in writing |

**The four risks worth naming up front:**

1. **Entity resolution is the sleeper.** `Fly.io` / `fly.io` / `Fly` / `flyio` must
   canonicalize together or every hop is noise, and no test above fails loudly when they
   don't — the eval just quietly doesn't improve. Mitigated by `EntityKey`'s table-driven
   tests; the alias table is the escape hatch if 7.3.6 blames resolution.
2. **The ranker is calibrated and the docs say so.** Contained by the empty-leg identity
   test and by touching neither constant.
3. **Extraction quality regression from a fatter schema.** Contained by the optional
   field, the unmodified pipeline test suite, and the measured token delta.
4. **Scope creep to Neo4j.** Out of scope by construction: SQLite tables and a 2-hop CTE
   until a hop-latency eval says otherwise (guardrail 4).

**What would make us stop:** 7.3.0 scoring high (the gap isn't real); the 7.3.2 token
delta being large enough that ingest cost dominates; or 7.3.6 showing a relational gain
that costs a regression elsewhere. Any of the three is a legitimate outcome, and the
eval slice from 7.3.0 stays in the repo either way as the tripwire.

### Sequencing & risk

| Task | Effort | Risk | Status | Real payoff |
|---|---|---|---|---|
| 7.1 Budget + skip-unchanged | M | Low | ✅ Done | Fewer LLM merge calls; bounded run time — the honest version of the framework plan's Phase 1 |
| 7.2 Optional sub-label | M | Low–Med | ✅ Done | Tighter clusters + a subcategory recall filter, exercised by the eval harness |
| 7.3 Graph (Strategy B) | L | Med–High | 📋 Planned | Relational recall — `project-plan.md`'s moat item, now broken into 7.3.0–7.3.6 with 7.3.0 as the eval gate that justifies (or cancels) the rest |

Do **not** carry over the framework plan's cumulative "125× / 3×" projections. Each
task above ships behind the Phase 4.6 eval gate and is described by the number it
actually moves.

---

## 12. Test strategy summary

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

## 13. Documentation deliverables matrix

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
