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

1. `domain` imports `shared`, std, and other contexts' `domain` — never
   anyone's `application` or `infrastructure`, and never `bootstrap`.

   Cross-context *domain* imports are the published language between
   contexts. Today that is exactly `identity::domain::UserContext`, which
   every repository contract takes as its first argument so that reaching
   another user's data cannot compile. It must live in `identity` for its
   constructors to stay `pub(in crate::identity)` — moving it to `shared`
   would force them public and throw the guarantee away. The rule
   therefore permits domain→domain and holds the line at the layers that
   actually carry framework and I/O dependencies.

   (This is a Phase 2 amendment. The original rule said "`shared` and std
   only", written before it was clear that the isolation guarantee
   requires a domain type to be shared between contexts.)
2. `application` imports its own `domain` + `shared` + other contexts'
   `application` (never their `infrastructure`).
3. `infrastructure` implements domain-owned traits; only `bootstrap/` wires
   concrete implementations into use cases (composition root = the only
   place that sees everything). One context's infrastructure may not
   import another's — with a single spelled-out exception:
   `identity::infrastructure::http` publishes the
   `Authenticated`/`ReadAccess`/`WriteAccess` extractors, which every
   other context's routes are built on. That is how a handler becomes
   unable to run without the right scope; the alternative is each context
   re-implementing bearer parsing. Identity's repositories and CLI remain
   off limits, and `check-boundaries.sh` encodes the carve-out narrowly
   enough to prove it (a Phase 2 amendment, same reasoning as rule 1).
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
    cli.rs                         `recuerdos-ai user` / `key` subcommands
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

## Recall: three legs, two clocks

Recall in `memories` fuses three independent legs by reciprocal rank —
each is a `domain` trait with an `infrastructure` adapter, wired in
`bootstrap`:

| Leg | Contract | Answers |
|---|---|---|
| Vector | `VectorIndex` | "reads like this query" (semantic) |
| Keyword | `TextIndex` | "contains these tokens" (BM25 — the only leg that can match an identifier like `useQuery`) |
| Graph | `EntityGraph` | "is connected to what this query names" |

The graph leg (Task 7.3, "Strategy B") is what reaches a memory that
answers a question it shares no words with: it scans the query for entity
mentions, keeps the ones some memory of this user actually declared as
seeds, and walks up to `[graph].max_hops` edges to the memories those
seeds connect to. It is additive and self-silencing — `None` graph, or a
query naming no known entity, yields the exact two-leg result. Every
method takes a `&UserContext`, so a hop cannot compile its way into
another user's edges, the same rule the other two indexes follow.

The graph runs on **two clocks**, and keeping them apart is the whole
point of the bi-temporal design:

- **Transaction time** — *when we learned it*. This is `Memory::created_at`,
  and what the `since` filter and the audit trail read.
- **Valid time** — *when it was true in the world*. Each edge is valid from
  its asserting memory's `created_at` and stays valid until a later memory
  contradicts it (same subject and predicate, different object), which
  closes the old interval rather than deleting it.

A recall's `as_of` reads the graph in *valid* time: because a contradicted
edge keeps its closed interval instead of vanishing, a hop dated before a
change still traverses the edge that was true then — "who owned this
before the reorg?" — while the default (now) sees only live edges. Nothing
outside the graph leg consults valid time.
