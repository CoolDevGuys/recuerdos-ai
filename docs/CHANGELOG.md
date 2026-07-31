# Changelog

Format loosely follows [Keep a Changelog](https://keepachangelog.com/).
One entry per phase, backfilled as phases land.

## v0.1.1

- **`[server].mcp.allowed_hosts`.** The streamable-HTTP `/mcp` endpoint's
  DNS-rebinding guard accepted only loopback `Host` headers, so reaching it
  over the network needed a reverse proxy that rewrites `Host`. This option
  lets an operator allow the hostname(s) clients connect with directly
  (added to the loopback defaults, so local access still works); a single
  `"*"` disables the guard for a trusted private network. Empty — the
  default — is unchanged.
- **Docs & install hardening** since v0.1.0: reverse-proxy setups must
  rewrite `Host` for `/mcp` (Caddy `header_up Host localhost`); the Linux
  binary's glibc-2.38 floor (imposed by ONNX Runtime's prebuilt) is
  documented and `install.sh` now checks it up front and points to Docker
  below it; the GHCR image name is lowercased; and the README gained
  Updating and Uninstalling sections plus a comparison and use cases.

## v0.1.0 — Phase 6: SDK, docs & release

The first release candidate. Mostly about making everything from Phases
0–5 installable, documented, and usable from something other than curl —
plus the provider choice and operational features a self-hosted
deployment needs.

- **Python SDK** (`pip install recuerdos-ai`): a thin typed client over
  the REST API — one method per endpoint, pydantic models, and no
  caching or retry opinions of its own. The two things it does add are
  the two a raw `httpx` call makes awkward: errors keyed off the API's
  stable error `code` so callers branch on a type, and job polling.
  Ships a LangChain `BaseRetriever` behind a `[langchain]` extra.
- **Integration recipes** for the client types without one: Hermes,
  LangChain/LangGraph, and a plain-REST recipe for anything else. Every
  request and response in them was captured from a running daemon.
- **Benchmarks** in `docs/performance.md`, measured against the release
  image rather than a dev build, with the methodology and the hardware
  written down beside the numbers — including the targets that were
  missed.
- **`install.sh`**, which downloads a binary, verifies its checksum, and
  refuses to install anything unverified. Tested against a tampered
  tarball and a missing checksum, not just the happy path.
- **Release workflow**: native builds per architecture (linux
  x86_64/aarch64, macOS arm64), multi-arch image to GHCR, drafted
  release notes.
- **Apache-2.0**, and a recorded name-availability check.
- **README** rewritten as a launch page.

And the deployment-facing features that landed alongside the release work:

- **External embedding providers.** Beyond the built-in local ONNX model,
  embeddings can come from a **native Gemini client** (task-type-aware —
  `RETRIEVAL_DOCUMENT` on store, `RETRIEVAL_QUERY` on search, for better
  recall), any **OpenAI-compatible** endpoint, or **Ollama**. Chosen with
  `[embeddings].provider`; the model's dimensionality is discovered by a
  real request at startup, so there is no width to configure.
- **`recuerdos-ai reindex`.** Change the embedding model or provider and
  re-embed every memory in place — in one transaction — instead of
  needing a fresh data directory. The store pins its model, and the daemon
  now **refuses to start on a mismatch** with a message naming the fix,
  rather than failing later on the first recall.
- **Streamable-HTTP MCP transport** at `/mcp` (previously deferred, see
  Phase 3). A client connects over HTTP with a bearer token — no local
  binary, no `docker exec` shim — sharing the same four tools and the same
  per-user auth as the stdio path. This is the natural way to reach a
  daemon running on another host.
- **Gemini as a reasoning provider** for `[understanding]` — a preset over
  the OpenAI-compatible client, since Gemini's chat API needs no native
  client the way its embeddings do.
- **`recuerdos-ai config`** prints the effective, secret-free configuration
  (which providers, models and transports are actually in force), and
  `RECUERDOS_AI_CONFIG` points the whole CLI at one file without repeating
  `--config`.

## v0.5.0-alpha — Phase 5: Consolidation

Memory that stays clean over time. Until now the store only grew; this
phase is everything that makes it shrink again.

- **`POST /v1/sessions/distill`** and the **`session_distill` MCP tool**
  reduce a finished session to the few things that outlive it. Same
  pipeline as ordinary ingestion, asked a stricter question — *what is
  still true after this session ends?* — so the conventions and root
  causes survive and the task chatter does not. Most sessions distil to
  nothing, which is the correct answer.
- **A nightly consolidation job**, also runnable as
  `recuerdos-ai consolidate [--dry-run]`. Near-duplicates within a user's
  category are grouped by similarity, and the model writes the one memory
  that says what all of them said. Five phrasings of one preference
  become one active memory and five superseded ones, each with a `merge`
  audit entry carrying the reasoning.
- **The threshold proposes, the model disposes.** Similarity gets
  memories into a cluster but cannot tell whether they mean the same
  thing — "prefers pnpm" and "prefers Vitest" are very close in embedding
  space and are two facts. The merge prompt argues *against* merging, and
  every unusable answer parses as keep-separate: a duplicate surviving
  one more night is cheap, a merged-away fact is not.
- **TTL expiry.** Memories past their `expires_at` are retired with an
  audit entry naming the date. Retired, not erased — `expires_at` is a
  promise a memory stops being used, not that it stops existing.
- **Importance decay.** Memories are rescored from how recently and how
  often they were actually recalled, feeding recall ranking. It only ever
  demotes, and never below a floor well above zero: an architecture
  decision nobody has read this year has to lose ties, not vanish.
- **An LLM-written `memory://profile`.** Generated per half (how they
  work / about them) and cached until the memories under it change, so it
  compresses forty preferences into a paragraph instead of truncating at
  eight. Staleness is derived from the memories themselves rather than a
  dirty flag every write path has to remember to set.
- **Graceful degradation throughout.** Expiry and decay need no provider
  and run everywhere. Merging and the written digest need one and are
  simply skipped without it — the profile falls back to assembly, and the
  resource contract does not change. Distillation is the single exception:
  it refuses rather than storing a transcript whole, because that memory
  would be unrecallable and would cost a context window on every match.

## v0.4.0-alpha — Phase 4: Understanding

The differentiator: raw text in, understood memories out. A memory store
stops being a log.

- **`POST /v1/memories`** accepts raw content and returns `202` with a job
  id; **`GET /v1/jobs/{id}`** reports what became of it. The work is a
  model call that takes seconds — holding the request open for it would
  make a client's timeout our problem and lose the work on a disconnect.
- **Extraction** splits content into atomic, separately-recallable
  memories with a category, tags and entities. One sentence about moving
  hosts *and* a testing preference becomes two memories, not one blob.
- **Reconciliation** decides ADD / UPDATE / DELETE / NOOP against the
  memories most similar to each candidate. A contradiction supersedes
  what it replaces instead of accumulating beside it, so recall stops
  returning last quarter's answer alongside this quarter's. Superseded
  memories are retained for audit and reachable with
  `include_superseded=true`.
- **Three providers** — Anthropic, any OpenAI-compatible endpoint, and
  Ollama — behind one `ChatModel` contract, with retry, timeout and
  malformed-JSON recovery shared across all of them so behaviour cannot
  drift between them.
- **A durable job queue and worker pool.** Jobs survive a restart, a
  crashed worker's job is reclaimed rather than lost, and a poison job
  dead-letters with its error instead of retrying forever.
- **Still zero-egress by default.** `[understanding].provider` defaults to
  `none`, and in that mode the same endpoints store content verbatim,
  inferring only the category from unambiguous phrasing. Turning a
  provider on requires no client changes.
- **MCP `memory_save` runs the pipeline**, so an agent's save is
  reconciled against what is already stored — and the tool says so
  honestly, including when nothing was stored because the store already
  knew it.
- **`recuerdos-ai eval`** scores retrieval quality against a committed
  baseline, and CI fails a PR that drops recall@5 by more than five
  points. Nothing else in the suite would notice a ranking regression.
  See [evaluation.md](evaluation.md).

Known limitation: one eval case is committed failing — a query with no
vocabulary in common with its memory ("book a restaurant" → "user is
vegetarian") does not reach the top 5. It is kept as a real target rather
than removed to flatter the score.

## v0.3.0-alpha — Phase 3: MCP server

Claude Code, opencode and any other MCP client can now read and write the
same memory store.

- **Three tools** — `memory_save`, `memory_recall`, `memory_forget` — and
  the **`memory://profile` resource**, served by `recuerdos-ai mcp`.
- **The stdio server is a shim**, forwarding to the daemon over the same
  authenticated REST API any client uses. Four editor windows would
  otherwise mean four resident embedding models and four writers on one
  SQLite file.
- **Tool descriptions carry the trigger logic.** A model decides from the
  description alone whether to save something, so they name concrete
  phrasings ("I prefer", "we decided") *and* say when not to call —
  without which a memory store fills with transient task chatter.
- **`memory_forget` is two-step and the server enforces it**: ids without
  `confirm: true` delete nothing, and the candidate listing states
  plainly that nothing has been deleted yet.
- **`GET /v1/profile`** exposes the same digest over REST, which is what
  the shim reads and what a Claude Code SessionStart hook can curl.
- Integration recipes for [Claude Code](integrations/claude-code.md)
  (including SessionStart and PreCompact hooks) and
  [opencode](integrations/opencode.md).

Deferred at the time: the streamable-HTTP MCP transport. rmcp's session
factory cannot see request headers, so per-user auth would need a per-call
path that complicates keeping tool definitions identical across transports
— and every target client then spoke stdio. **Shipped later in v0.1.0**
(see above): the HTTP handler forwards the request's bearer to the same
per-user REST auth, so both transports share one set of tool definitions.

308 tests.

## v0.2.0-alpha — Phase 2: Memories

Store memories and search them semantically and lexically, fully offline,
strictly per user.

- **Hybrid recall.** A vector leg (sqlite-vec) and a keyword leg
  (tantivy BM25) run on every search, fused by reciprocal rank. The
  keyword leg is what finds `useQuery` as a literal token; the vector leg
  is what answers a question sharing no words with the memory.
- **Local embeddings**, in-process ONNX (bge-small-en-v1.5, 384d), baked
  into the Docker image so a container never downloads at runtime and an
  air-gapped host works. `recuerdos-ai warm-models` pre-populates a cache.
- **Taxonomy**: eight built-in categories, extensible via config. Unknown
  names are rejected rather than silently created.
- **REST**: `POST /v1/memories:direct`, `POST /v1/memories/search`,
  `GET/PATCH/DELETE /v1/memories/{id}`, `GET /v1/memories/export`,
  `GET /v1/audit`.
- **Export** as markdown (grouped by category, one memory per line, so it
  stays greppable) or JSON — your memories are yours and portable.
- **Soft delete and supersede** both retain the row, so the audit trail
  stays truthful.
- Isolation extends to memories: `user_id` is a vec0 *partition key*
  (a query never scans another user's vectors), each user has their own
  tantivy index directory, and the cross-tenant suite now tests recall,
  by-id reads, edits, deletes, export and audit.

**A 12x latency fix, found by measuring rather than assuming.** The first
benchmark showed 251ms end-to-end searches against 8ms of actual work:
argon2id was running on every request and accounted for 96% of the
response. `VerifiedKeyCache` caches the verification (SHA-256 digest of
the presented secret, constant-time compared) while still reading the key
row every time, so revocation and scope changes take effect immediately.

Measured after, at 2,000 memories: **p95 29ms end-to-end**, 16ms
server-side, 55ms per ingest.

Known limitations, stated rather than buried:

- The plan's 100k-memory target is unproven. sqlite-vec 0.1.x has no ANN
  index — KNN is a brute-force scan — so recall latency grows linearly
  with corpus size. `scripts/bench-recall.sh` takes the scale as an
  argument.
- Search filters are applied after the indexes answer, so a very
  selective filter over a large corpus can return fewer results than
  asked for.
- No generated OpenAPI spec yet; `docs/api.md` is hand-written. The spec
  belongs with Phase 6's SDK, which is what consumes it.
- Ingest commits the text index synchronously (read-after-write beats
  throughput for an agent that just saved something), which is most of
  the 55ms.

The release image grows from ~112 MB to ~287 MB: the embedding model is
baked in (~130 MB) so a container never downloads at runtime, plus the
C++ runtime libraries ONNX needs. Both build stages now pin the same
Debian release — `rust:1-slim` had drifted to trixie while the runtime
stage was still bookworm, which produced a binary the runtime's glibc
could not load.

297 tests.

## v0.1.0-alpha.1 — Phase 1: Identity

Multi-user authentication. Every `/v1` route now resolves to exactly one
user, and the isolation guarantee is enforced by the type system.

- **Users and API keys.** `recuerdos-ai user add|list`,
  `key issue|list|revoke`. Keys are `ra_live_<prefix><secret>`; only an
  argon2id hash is stored, and the key is displayed once at issue time.
- **Scopes**: `read`, `write`, `admin`. `write` does not imply `read`, so
  a write-only ingestion key is expressible. `admin` implies everything.
- **`UserContext`**, the capability token every later context will require.
  Its constructors are `pub(in crate::identity)`, so no other context can
  invent one for a user it did not authenticate — cross-tenant access is a
  compile error. `check-boundaries.sh` fails if that visibility is widened.
- **Auth middleware**: `Authenticated`, `ReadAccess`, `WriteAccess`
  extractors. The argon2 verify runs on `spawn_blocking`; `last_used_at`
  is recorded off the request path.
- **Uniform rejections**: missing, malformed, unknown, wrong-secret and
  revoked keys are byte-identical 401s, and the verify runs before the
  revocation check so timing doesn't leak revocation either.
- **Error envelope** `{"error":{"code","message"}}` for all contexts;
  `internal` responses never carry their detail.
- **`[auth].mode = "none"`** for single-user deployments: a real persisted
  `default` user (so data isn't orphaned if auth is re-enabled), a loud
  startup warning, and fail-closed parsing of the mode value.
- **SQLite storage**: WAL, enforced foreign keys, refinery migrations run
  at startup.
- `tests/identity_isolation.rs`, the cross-tenant suite later phases
  extend. Its sharpest case splices one user's public prefix onto
  another's secret and asserts 401.
- Docs: `api.md`, `security.md` (isolation + threat model),
  `configuration.md` auth/storage sections, `architecture.md` worked
  example.

130 tests.

## v0.1.0-alpha.0 — Phase 0: Foundation

- Screaming-architecture skeleton (identity/memories/understanding/
  providers/consolidation, each domain/application/infrastructure) plus
  the shared kernel (`RaError`, UUIDv7 ids, `Clock`).
- `scripts/check-boundaries.sh` boundary check, wired into `just check`.
- Docker-only dev flow: `Dockerfile.dev` (non-root, host-UID-matched),
  `docker-compose.yml`, and a multi-stage release `Dockerfile` (~112 MB,
  non-root, healthchecked).
- `AppConfig`: TOML + `RECUERDOS_AI_*` env overrides via figment,
  aggregated validation errors, `recuerdos-ai init`.
- axum HTTP skeleton: `/healthz`, `/version`, graceful shutdown on
  SIGINT/SIGTERM, tracing (plain or JSON via `RECUERDOS_AI_LOG=json`).
- GitHub Actions CI: check / test / docker-build-and-smoke-test.
