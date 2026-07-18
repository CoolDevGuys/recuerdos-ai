# Contributing

## Dev flow: Docker only

You don't need a local Rust toolchain. `just dev` builds a dev image
(matching your host UID/GID so bind-mounted files stay yours) and starts
the daemon with auto-rebuild on file change. See the root
[README](../README.md#quickstart) for the full quickstart.

| Command | What it does |
|---|---|
| `just dev` | Start the daemon with auto-rebuild |
| `just check` | fmt --check + clippy -D warnings + boundary script + tests |
| `just test` | Run the test suite |
| `just fmt` | Format the code |

Every recipe has a `*-native` counterpart (`just check-native`, ...) for
contributors who do have a local toolchain — Docker is the default path,
not a cage.

## Before opening a PR

Run `just check` and make sure it's green. CI runs the same commands
inside the same dev image, so a green `just check` locally means CI will
pass too (see `.github/workflows/ci.yml`).

## Architecture & boundary rules

RecordAgent is organized as bounded contexts, each a vertical slice of
`domain`/`application`/`infrastructure` — see
[docs/architecture.md](architecture.md) before adding new modules.
`scripts/check-boundaries.sh` (part of `just check`) greps for the common
layering violations; if it fails, read the rule it's citing before working
around it.

## Commits

Conventional commits (`feat(memories): hybrid search with RRF`,
`fix(auth): reject revoked keys`, ...). Each phase in
[implementation-plan.md](../implementation-plan.md) lives on its own
branch and ends with a PR into `main`.

## Tests

- Unit tests live next to the code they test (`#[cfg(test)] mod tests`).
- Black-box scenario tests live in `tests/*.rs` and use the harness in
  `tests/common/` — they spawn the real binary and drive it over HTTP,
  the way a real client would.
- No test may sleep a fixed duration to "wait for" something async; poll
  with a timeout instead (see `tests/common/mod.rs` for the pattern).
