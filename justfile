# Dev commands. Docker is the default path (a contributor needs nothing but
# Docker installed) — every recipe also has a `-native` counterpart that
# shells out to a local toolchain directly, for contributors who have one.

export USER_UID := `id -u`
export USER_GID := `id -g`

# Start the daemon with auto-rebuild on file change.
dev:
    docker compose up dev

# fmt --check + clippy -D warnings + boundary script + test, in the container.
check:
    docker compose run --rm dev just check-native

test:
    docker compose run --rm dev just test-native

fmt:
    docker compose run --rm dev cargo fmt

# Start the optional local Ollama profile (used from Phase 4 onward).
llm:
    docker compose --profile llm up ollama

# Build the release image.
docker-build:
    docker build -f docker/Dockerfile -t recordagent:local .

check-native:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    bash scripts/check-boundaries.sh
    cargo test

test-native:
    cargo test

fmt-native:
    cargo fmt

# Download the embedding model into the shared volume. The test suites
# need it; the dev image does not bake it in (the release image does).
warm:
    docker compose run --rm dev cargo run -q --bin recordagent -- warm-models
