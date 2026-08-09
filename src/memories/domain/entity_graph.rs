//! The graph leg of recall: a third index, beside `VectorIndex` and
//! `TextIndex`.
//!
//! Where those two answer "which memories *read like* this query", the
//! graph answers "which memories are *connected to* the things this query
//! names" — so a question whose answer never shares a word with the
//! memory that holds it ("who leads the team that owns the billing
//! service") becomes reachable by walking entity to entity.
//!
//! Like the other two indexes, the contract is owned here by the consumer
//! (`memories`), the implementation lives in `infrastructure`, and every
//! method takes a `&UserContext` so a hop cannot compile its way into
//! another user's edges.
//!
//! Inert until Task 7.3.4 teaches recall to consult it; Task 7.3.1 only
//! builds the store and its contract.

// The contract is defined and tested now; its callers arrive in Tasks
// 7.3.2 (record on ingest), 7.3.3 (invalidate on supersede) and 7.3.4
// (neighbours in recall), so a non-test build sees it unused until then.
#![allow(dead_code)]

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::entity_key::EntityKey;
use crate::memories::domain::memory::Entity;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use chrono::{DateTime, Utc};

/// A directed edge asserted by a memory: `subject —predicate→ object`.
///
/// Subject and object are entity *names* as written; the store
/// canonicalises them to [`EntityKey`]s on the way in, so a caller never
/// has to remember to. The same shape drives invalidation
/// ([`EntityGraph::invalidate`]): a superseding memory's re-assertions are
/// just relations.
///
/// A `Relation` carries no time — it is the *assertion*. The stored edge
/// gains two: it becomes valid at the asserting memory's `created_at` and
/// stays valid until some later memory contradicts it, at which point its
/// `invalid_at` closes. That *valid* time is a different clock from the
/// memory's *transaction* time (`Memory::created_at`, "when we learned
/// it") — the distinction is what lets a query ask what was true *before*
/// a change, and it is Strategy B's whole point (implementation-plan.md
/// Task 7.3, decision 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub subject: String,
    pub predicate: String,
    pub object: String,
}

/// The canonical form of a predicate — the join key invalidation matches on.
///
/// `"deploys on"`, `"Deploys-On"`, `"deploys_on"` all become `deploys_on`:
/// runs of non-alphanumeric characters collapse to a single underscore, the
/// result is lowercased, and leading/trailing underscores are trimmed.
///
/// It lives here, beside [`Relation`] and mirroring [`EntityKey`] for
/// endpoints, so that *every* side agrees on it: the candidate path
/// normalises a predicate once on the way in, and the store re-normalises
/// defensively on both `record` and `invalidate`. Because it is idempotent
/// (`normalise_predicate("deploys_on") == "deploys_on"`), passing an
/// already-canonical predicate through again is a no-op — which is exactly
/// what keeps a writer that files `deploys_on` and an invalidator that looks
/// up `"deploys on"` from ever missing each other.
pub fn normalise_predicate(raw: &str) -> String {
    let mut out = String::new();
    let mut pending_underscore = false;
    for ch in raw.chars() {
        if ch.is_alphanumeric() {
            if pending_underscore && !out.is_empty() {
                out.push('_');
            }
            pending_underscore = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_underscore = true;
        }
    }
    out
}

pub trait EntityGraph: Send + Sync {
    /// Records a memory's entities and relations, replacing any it had
    /// before — recording is idempotent, so a re-ingest or an edit leaves
    /// the projection matching the memory rather than accumulating stale
    /// rows. `valid_from` is the point the relations became true (the
    /// asserting memory's `created_at`); their `invalid_at` starts open.
    fn record(
        &self,
        context: &UserContext,
        memory_id: MemoryId,
        entities: &[Entity],
        relations: &[Relation],
        valid_from: DateTime<Utc>,
    ) -> Result<()>;

    /// Drops a memory's entities and edges — called when a memory is
    /// forgotten, so the graph never hops through a memory the user
    /// removed.
    fn remove(&self, context: &UserContext, memory_id: MemoryId) -> Result<()>;

    /// Projects only a memory's entities, replacing the ones it had and
    /// leaving its relations untouched (Task 7.3.5). This is what the
    /// entity backfill uses: it rebuilds `memory_entities` from the
    /// `entities` JSON already on every memory, at zero model cost, without
    /// disturbing any edges a later relation backfill or ingest wrote.
    fn record_entities(
        &self,
        context: &UserContext,
        memory_id: MemoryId,
        entities: &[Entity],
    ) -> Result<()>;

    /// Projects only a memory's relations, replacing its still-open edges
    /// and leaving its entities untouched (Task 7.3.5) — the mirror of
    /// [`record_entities`](Self::record_entities), used by the relation
    /// backfill. Closed edges are history and are preserved exactly as
    /// [`record`](Self::record) preserves them.
    fn record_relations(
        &self,
        context: &UserContext,
        memory_id: MemoryId,
        relations: &[Relation],
        valid_from: DateTime<Utc>,
    ) -> Result<()>;

    /// Whether this memory already has any relation edge (open or closed).
    /// The relation backfill uses it to skip a memory ingest already gave
    /// edges, so it never re-pays a model call for one that has them.
    fn has_relations(&self, context: &UserContext, memory_id: MemoryId) -> Result<bool>;

    /// Of `candidates`, the entity keys this user's graph actually knows —
    /// the seeds a hop may start from (Task 7.3.4).
    ///
    /// Recall scans the query text for entity mentions and canonicalises
    /// each to an [`EntityKey`] (no model call); this narrows that list to
    /// the keys some memory of this user actually declared. A query naming
    /// nothing known yields no seeds, so the graph leg stays silent and
    /// recall is exactly its two-leg self. Deduplicated; order unspecified.
    fn seeds(&self, context: &UserContext, candidates: &[EntityKey]) -> Result<Vec<EntityKey>>;

    /// The memories reachable from `seeds` within `hops` edges, live at
    /// `as_of` (or now, if `None`), best-first by hop distance then by how
    /// many of the query's entities reach them, and capped at `limit`.
    ///
    /// Traversal is undirected over each edge: arriving at an entity by
    /// either endpoint lets the walk continue from the other, because
    /// "the team that owns X" and "the person who leads that team" are the
    /// same chain read forwards and backwards. Empty `seeds` yields
    /// nothing — the property that keeps a query naming no known entity
    /// from perturbing recall.
    fn neighbours(
        &self,
        context: &UserContext,
        seeds: &[EntityKey],
        hops: usize,
        as_of: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<MemoryId>>;

    /// Closes the validity interval of every currently-live edge that
    /// `superseding` contradicts: same subject and predicate, but a
    /// different object. The contradicted edges get `invalid_at = at` and
    /// `invalidated_by = by`; an edge the superseding memory merely
    /// re-affirms (same object) is left untouched, and so is history the
    /// new memory says nothing about.
    ///
    /// Idempotent: a re-run skips edges already closed (`invalid_at` set)
    /// and never moves an existing interval. Driven by Task 7.3.3; defined
    /// here so the store's contract is whole.
    fn invalidate(
        &self,
        context: &UserContext,
        superseding: &[Relation],
        at: DateTime<Utc>,
        by: MemoryId,
    ) -> Result<()>;
}
