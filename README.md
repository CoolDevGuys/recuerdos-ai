# RecordAgent

A fast, self-hostable long-term memory service for AI agents and coding
assistants — one memory backend consumed by every agent you use, over REST
or MCP. See [project-plan.md](project-plan.md) for the full design and
[implementation-plan.md](implementation-plan.md) for the phased build plan.

> **Status: Phase 0 — Foundation.** The daemon boots, serves `/healthz` and
> `/version`, and loads validated config. No memory features yet — see the
> phase table below.

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
| 0 — Foundation | Docker dev env, config, HTTP skeleton, CI | ✅ in progress |
| 1 — Identity | Users, API keys, per-user isolation | ⬜ |
| 2 — Memories | Store + hybrid search (REST) | ⬜ |
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

## Contributing

See [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) (Docker-only dev flow,
boundary rules).

## License

Apache-2.0.
