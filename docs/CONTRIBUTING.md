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
| `just eval` | Score retrieval quality against the committed baseline |
| `just fmt` | Format the code |

Every recipe has a `*-native` counterpart (`just check-native`, ...) for
contributors who do have a local toolchain — Docker is the default path,
not a cage.

## Before opening a PR

Run `just check` and make sure it's green. CI runs the same commands
inside the same dev image, so a green `just check` locally means CI will
pass too (see `.github/workflows/ci.yml`).

## Architecture & boundary rules

Recuerdos AI is organized as bounded contexts, each a vertical slice of
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
- Changing anything retrieval touches — the embedder, the tokenizer, the
  ranker, candidate depth — means running `just eval`. `cargo test` will
  not notice a ranking regression; that is the whole reason the eval
  exists. See [evaluation.md](evaluation.md).

## Releasing

Releases are tag-driven. Pushing a `v*` tag runs
[`.github/workflows/release.yml`](../.github/workflows/release.yml), which:

- builds native binaries (linux x86_64/aarch64, macOS arm64), checksums
  them, and drafts a GitHub release for you to review and publish;
- builds and pushes the multi-arch Docker image to
  `ghcr.io/cooldevguys/recuerdos-ai`;
- publishes the Python SDK to PyPI (final releases only — not `-rc` tags).

```bash
# bump versions first: Cargo.toml, and sdk/python/pyproject.toml if the SDK changed
git tag v0.1.0 && git push origin v0.1.0
```

### One-time PyPI setup (Trusted Publishing)

The SDK is published with [PyPI Trusted Publishing](https://docs.pypi.org/trusted-publishers/),
so there is **no API token to store**. Before the first publish, configure
it once on PyPI — for a project that does not exist yet, use
*Your account → Publishing → Add a pending publisher*:

| Field | Value |
|---|---|
| PyPI Project Name | `recuerdos-ai` |
| Owner | `CoolDevGuys` |
| Repository name | `recuerdos-ai` |
| Workflow name | `release.yml` |
| Environment name | `pypi` |

Then create a GitHub **Environment** named `pypi` (repo *Settings →
Environments → New environment*); the publish job runs in it. That's all —
no secrets. The SDK version comes from `sdk/python/pyproject.toml`, and the
publish step is idempotent (`skip-existing`), so a tag that didn't bump the
SDK is a harmless no-op rather than a failure.

Prefer an API token instead? Drop the `id-token`/`environment` lines from
the `pypi` job and add `password: ${{ secrets.PYPI_API_TOKEN }}` to the
publish step, with the token stored as a repo secret.
