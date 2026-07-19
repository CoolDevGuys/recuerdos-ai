# Changelog

Format loosely follows [Keep a Changelog](https://keepachangelog.com/).
One entry per phase, backfilled as phases land.

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
