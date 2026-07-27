# RecordAgent — operational convenience wrapper.
#
# Everything runs in Docker; the only prerequisite is Docker itself. This
# Makefile covers the operator side — starting the daemon, creating users,
# issuing keys, running consolidation — and delegates the dev-workflow
# targets (check/test/fmt/eval) to the justfile inside the container, so
# there is one source of truth for those rather than two that drift.
#
# Run `make` (or `make help`) for the list.
#
# Note on the first run: `make up` compiles the daemon from scratch, which
# takes a few minutes. Every command after that reuses the cached build.

COMPOSE := docker compose
# A one-off CLI invocation against the shared data volume. `run --rm dev`
# starts a throwaway container that mounts the same `data` volume the
# `up` daemon uses, so a key issued here is visible to the running server
# (docker-compose.yml sets RECORDAGENT_STORAGE__PATH=/data on both).
#
# `-e RECORDAGENT_CONFIG=$(CONFIG)` makes every CLI command read the same
# recordagent.toml the daemon reads, so `make reindex`, `make consolidate`
# etc. honour your configured provider instead of falling back to defaults.
# Scoped to `run` (not the compose `environment:` block) so it never
# reaches `docker compose run ... cargo test`, which must stay env-only.
# Deferred (`=` not `:=`) so $(CONFIG), defined further down, is in scope
# when a recipe expands this.
CLI = $(COMPOSE) run --rm -e RECORDAGENT_CONFIG=$(CONFIG) dev cargo run -q --bin recordagent --

# Defaults, overridable on the command line: `make key-issue HANDLE=sam`.
# HANDLE rather than USER on purpose — `$(USER)` in a Makefile expands to
# the host login name from the environment, which would silently issue
# keys for the wrong account.
HANDLE ?= alex
SCOPES ?= read,write
NAME   ?= default
PREFIX ?=
# The config file the CLI reads, relative to the repo (bind-mounted at
# /app in the container). The compose daemon reads the same path, so
# `make config` reflects what the running daemon uses. A missing file is
# not an error — you fall back to defaults + RECORDAGENT_* env.
CONFIG ?= recordagent.toml

.DEFAULT_GOAL := help

# ---------------------------------------------------------------------------
##@ Lifecycle

.PHONY: up
up: ## Start the daemon in the background (first run compiles; be patient)
	$(COMPOSE) up -d dev
	@echo "daemon starting on http://localhost:7070 — follow it with 'make logs'"

.PHONY: dev
dev: ## Start the daemon in the foreground with live logs (Ctrl-C to stop)
	$(COMPOSE) up dev

.PHONY: down
down: ## Stop and remove the daemon container (keeps data + model volumes)
	$(COMPOSE) down

.PHONY: restart
restart: ## Restart the daemon
	$(COMPOSE) restart dev

.PHONY: logs
logs: ## Follow the daemon's logs
	$(COMPOSE) logs -f dev

.PHONY: ps
ps: ## Show what is running
	$(COMPOSE) ps

.PHONY: health
health: ## Check whether the daemon is answering
	@curl -fsS http://localhost:7070/healthz && echo || \
		echo "not reachable on :7070 — is it up? try 'make up'"

.PHONY: clean
clean: ## Stop everything and DELETE the data + model volumes (irreversible)
	@printf 'This deletes all stored memories, users and keys. Continue? [y/N] '; \
	read ans; [ "$$ans" = "y" ] || { echo "aborted"; exit 1; }
	$(COMPOSE) down -v

# ---------------------------------------------------------------------------
##@ Setup

.PHONY: quickstart
quickstart: up ## Start the daemon, create a user, and issue a key in one go
	@echo "waiting for the daemon to become healthy…"
	@for i in $$(seq 1 120); do \
		curl -fsS http://localhost:7070/healthz >/dev/null 2>&1 && break; \
		sleep 1; \
	done
	@echo "==> creating user '$(HANDLE)' (idempotent — ignore 'already exists')"
	-$(CLI) user add $(HANDLE)
	@echo "==> issuing a key (scopes: $(SCOPES)) — copy it now, it is shown once:"
	$(CLI) key issue --user $(HANDLE) --scopes $(SCOPES) --name $(NAME)

.PHONY: init
init: ## Write a default recordagent.toml and data dir
	$(CLI) init

.PHONY: config
config: ## Show the resolved config (providers, models, transports) — no secrets
	$(CLI) config

.PHONY: migrate
migrate: ## Apply pending SQL migrations (they also run automatically on 'up')
	@echo "migrations run at startup; forcing them now by opening the database…"
	$(CLI) user list >/dev/null && echo "database is at the latest schema version"

.PHONY: warm
warm: ## Download the embedding model into the shared volume
	$(CLI) warm-models

.PHONY: reindex
reindex: ## Re-embed every memory after changing the embedding model (stop the daemon first)
	$(CLI) reindex

# ---------------------------------------------------------------------------
##@ Users & keys

.PHONY: users
users: ## List all users
	$(CLI) user list

.PHONY: user-add
user-add: ## Create a user:            make user-add HANDLE=sam [EMAIL=sam@x.com]
	$(CLI) user add $(HANDLE) $(if $(EMAIL),--email $(EMAIL),)

.PHONY: keys
keys: ## List a user's keys:        make keys HANDLE=sam
	$(CLI) key list --user $(HANDLE)

.PHONY: key-issue
key-issue: ## Issue an API key:         make key-issue HANDLE=sam [SCOPES=read,write] [NAME=laptop]
	$(CLI) key issue --user $(HANDLE) --scopes $(SCOPES) --name $(NAME)

.PHONY: key-revoke
key-revoke: ## Revoke a key by prefix:    make key-revoke PREFIX=b99f884a
	@test -n "$(PREFIX)" || { echo "usage: make key-revoke PREFIX=<prefix from 'make keys'>"; exit 1; }
	$(CLI) key revoke $(PREFIX)

# ---------------------------------------------------------------------------
##@ Memory operations

.PHONY: consolidate
consolidate: ## Run the dedup/merge + decay + expiry job now
	$(CLI) consolidate

.PHONY: consolidate-dry
consolidate-dry: ## Preview what consolidation would merge — changes nothing
	$(CLI) consolidate --dry-run

# ---------------------------------------------------------------------------
##@ Development (delegates to the justfile inside the container)

.PHONY: check
check: ## fmt --check + clippy -D warnings + boundary script + tests
	$(COMPOSE) run --rm dev just check-native

.PHONY: test
test: ## Run the Rust test suite
	$(COMPOSE) run --rm dev just test-native

.PHONY: fmt
fmt: ## Format the code
	$(COMPOSE) run --rm dev cargo fmt

.PHONY: eval
eval: ## Score retrieval quality against the committed baseline
	$(COMPOSE) run --rm dev just eval-native

.PHONY: sdk-test
sdk-test: ## Lint, type-check and test the Python SDK against a real daemon
	$(COMPOSE) --profile sdk run --rm sdk
	$(COMPOSE) --profile sdk down

.PHONY: build
build: ## Build the release Docker image (recordagent:local)
	docker build -f docker/Dockerfile -t recordagent:local .

.PHONY: shell
shell: ## Open a shell in the dev container
	$(COMPOSE) run --rm dev bash

# ---------------------------------------------------------------------------

.PHONY: help
help: ## Show this help
	@echo "RecordAgent — make targets"
	@echo
	@awk 'BEGIN {FS = ":.*##"} \
		/^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5); next } \
		/^[a-zA-Z0-9_-]+:.*##/ { printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2 }' \
		$(MAKEFILE_LIST)
	@echo
	@echo "Variables: HANDLE=$(HANDLE)  SCOPES=$(SCOPES)  NAME=$(NAME)"
	@echo "Example:   make quickstart HANDLE=me"
