# Dev commands. `just dev`/`just test`/`just check` run in Docker by
# default (see Task 0.2); every recipe also works with a local toolchain.

dev:
    cargo watch -x 'run -- serve'

check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    bash scripts/check-boundaries.sh
    cargo test

test:
    cargo test

fmt:
    cargo fmt
