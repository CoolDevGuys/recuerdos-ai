# Architecture

**Status: Phase 0 skeleton.** This doc gets a diagram and worked examples
in Phase 6; today it states the rules that are already enforced.

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

See [implementation-plan.md §2](../implementation-plan.md#2-architecture)
for the full rationale.
