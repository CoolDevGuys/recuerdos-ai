# Changelog

Format loosely follows [Keep a Changelog](https://keepachangelog.com/).
One entry per phase, backfilled as phases land.

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
