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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub subject: String,
    pub predicate: String,
    pub object: String,
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

    /// The memories reachable from `seeds` within `hops` edges, live at
    /// `as_of` (or now, if `None`), best-first by hop distance and capped
    /// at `limit`.
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
