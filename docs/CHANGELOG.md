# Changelog

Format loosely follows [Keep a Changelog](https://keepachangelog.com/).
One entry per phase, backfilled as phases land.

## v0.2.0-alpha — Phase 2: Memories

Store memories and search them semantically and lexically, fully offline,
strictly per user.

- **Hybrid recall.** A vector leg (sqlite-vec) and a keyword leg
  (tantivy BM25) run on every search, fused by reciprocal rank. The
  keyword leg is what finds `useQuery` as a literal token; the vector leg
  is what answers a question sharing no words with the memory.
- **Local embeddings**, in-process ONNX (bge-small-en-v1.5, 384d), baked
  into the Docker image so a container never downloads at runtime and an
  air-gapped host works. `recordagent warm-models` pre-populates a cache.
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

297 tests.

## v0.1.0-alpha.1 — Phase 1: Identity

Multi-user authentication. Every `/v1` route now resolves to exactly one
user, and the isolation guarantee is enforced by the type system.

- **Users and API keys.** `recordagent user add|list`,
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
- `AppConfig`: TOML + `RECORDAGENT_*` env overrides via figment,
  aggregated validation errors, `recordagent init`.
- axum HTTP skeleton: `/healthz`, `/version`, graceful shutdown on
  SIGINT/SIGTERM, tracing (plain or JSON via `RECORDAGENT_LOG=json`).
- GitHub Actions CI: check / test / docker-build-and-smoke-test.
