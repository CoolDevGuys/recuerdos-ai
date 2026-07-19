# Architecture

**Status: Phase 1.** This doc gets a diagram and fuller examples in
Phase 6; today it states the rules that are already enforced, with
`identity` as the worked example.

## Bounded contexts

| Context | Responsibility | Key language |
|---|---|---|
| `identity` | Users, API keys, authentication, `UserContext` | User, ApiKey, Scope |
| `memories` | Storing, indexing, searching, exporting memories | Memory, Category, Tag, Recall |
| `understanding` | LLM pipeline: extract → reconcile → label | Candidate, Reconciliation, Taxonomy |
| `providers` | Concrete LLM/embedding implementations | AnthropicChatModel, FastembedEmbedder |
| `consolidation` | Background jobs: dedup/merge, decay, distillation, profile | ConsolidationRun, Distillation, ProfileDigest |
| `shared` | Shared kernel: ids, error type, clock — tiny by design | MemoryId, UserId, RaError |

## Layers inside each context (vertical slice)

```
<context>/
  domain/           # pure: entities, value objects, contracts (traits)
                    #   no tokio, no sqlite, no http, no serde_json on entities
  application/      # use cases (one file per use case), DTOs, orchestration
  infrastructure/   # adapters: sqlite repos, tantivy, http handlers, mcp tools,
                    #   provider clients — everything that touches the outside world
```

## Boundary rules

1. `domain` imports only `shared` and std.
2. `application` imports its own `domain` + `shared` + other contexts'
   `application` (never their `infrastructure`).
3. `infrastructure` implements domain-owned traits; only `bootstrap/` wires
   concrete implementations into use cases (composition root = the only
   place that sees everything).
4. Inbound adapters (axum handlers, rmcp tools, CLI commands) live in the
   infrastructure layer of the context that owns the use case.
5. Contracts are owned by consumers: e.g. the `Embedder` trait lives in
   `memories/domain/`, not in `providers/`.

`scripts/check-boundaries.sh` greps for the common violations of rules 1–3
plus the banned `*Port`/`*Service`/`*Manager`/`*Helper` naming suffixes,
and runs in `just check`. It's a cheap architecture test, not a substitute
for review.

## Naming conventions

- **Traits are role names, implementations are technology-prefixed:**
  `UserRepository` ← `SqliteUserRepository`; `Embedder` ← `FastembedEmbedder`.
- **Use cases are atomic doers:** one small file, one struct named after
  its single job, exposing one public method — `execute`.
- One trait per file, named after the trait — never a catch-all
  `ports.rs`/`traits.rs`.

## Worked example: `identity`

The first complete slice, and the shape every later context follows.

```
identity/
  domain/                          pure, no framework
    user.rs                        User + handle/email validation
    api_key.rs                     ApiKey, ApiKeyToken (parse/generate/redact)
    scope.rs                       Scope: read | write | admin
    user_context.rs                the capability token (see docs/security.md)
    user_repository.rs             ─┐
    api_key_repository.rs           ├ contracts, one trait per file
    api_key_hasher.rs              ─┘
  application/                     one doer per file, one public `execute`
    user_creator.rs
    api_key_issuer.rs
    api_key_revoker.rs
    api_key_lister.rs
    key_authenticator.rs           raw key → UserContext
    default_user_resolver.rs       the [auth].mode = "none" user
  infrastructure/                  everything touching the outside world
    sqlite_user_repository.rs      implements UserRepository
    sqlite_api_key_repository.rs   implements ApiKeyRepository
    argon2_api_key_hasher.rs       implements ApiKeyHasher
    cli.rs                         `recordagent user` / `key` subcommands
    http/authenticated.rs          Authenticated / ReadAccess / WriteAccess
```

Two things to copy from it:

- **The domain has no async and no I/O.** Use cases are synchronous and
  take their collaborators as `Arc<dyn Trait>`. Async lives only at the
  edges: the HTTP extractor wraps the blocking call in `spawn_blocking`.
  This is what makes the layer testable without a runtime *and* keeps
  argon2 off the async workers.
- **Concrete types are chosen in exactly one place.** `bootstrap/wiring.rs`
  is the only file that names both `UserRepository` and
  `SqliteUserRepository`.

## Where the composition root lives

`bootstrap/` — `config.rs` (typed config), `wiring.rs` (builds the object
graph), `state.rs` (axum state), `server.rs` (router + shutdown). If a
module needs to know which implementation it's talking to, that knowledge
belongs here instead.

See [implementation-plan.md §2](../implementation-plan.md#2-architecture)
for the full rationale.
