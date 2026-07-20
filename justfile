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

# Start the optional local Ollama profile, for zero-egress understanding.
llm:
    docker compose --profile llm up ollama

# Score retrieval quality against the committed baseline.
eval:
    docker compose run --rm dev just eval-native

# Re-record the baseline after a deliberate retrieval change.
eval-record:
    docker compose run --rm dev cargo run -q --bin recordagent -- \
        eval --write-baseline eval/baseline.json

# Lint, type-check and test the Python SDK, including its integration
# suite against a real daemon running the release image.
sdk-test:
    docker compose --profile sdk run --rm sdk
    docker compose --profile sdk down

# Build the release image.
docker-build:
    docker build -f docker/Dockerfile -t recordagent:local .

eval-native:
    cargo run -q --bin recordagent -- \
        eval --baseline eval/baseline.json --max-drop 5

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
