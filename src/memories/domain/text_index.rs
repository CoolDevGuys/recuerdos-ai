//! The keyword (BM25) leg of hybrid search.
//!
//! Pure vector search is weak on exact tokens — `useQuery`, `pnpm`,
//! `RA-1234` — because an embedding places them near their semantic
//! neighbours rather than at an exact match. For the coding persona those
//! literals are often the whole query, which is why hybrid beats vector
//! alone here (project-plan.md §7.7).

use super::memory::Memory;
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;

pub trait TextIndex: Send + Sync {
    fn upsert(&self, context: &UserContext, memory: &Memory) -> Result<()>;

    /// Index every memory in `memories`, committing in bulk.
    ///
    /// Rebuild paths (the text-index rebuilder use case) call it in
    /// chunks so a 10 000-memory rebuild is a handful of commits rather
    /// than 10 000. The default loops over `upsert`, which is correct for
    /// any implementation; implementations with a real batch path — one
    /// commit per chunk — override it. Same per-memory semantics: an
    /// existing document for the same id is replaced, not duplicated.
    fn upsert_many(&self, context: &UserContext, memories: &[Memory]) -> Result<()> {
        for memory in memories {
            self.upsert(context, memory)?;
        }
        Ok(())
    }

    /// Whether this user already has a *persisted* keyword index.
    ///
    /// The startup auto-rebuild uses it to tell "index predates the
    /// current analyzer version" from "user saved nothing yet", without
    /// opening the index. The default says *yes* — an implementation
    /// without persistent per-user state (a test double) reports itself
    /// as never needing a rebuild, which keeps the upgrade path tied to
    /// the real adapter that has one.
    fn is_populated(&self, _context: &UserContext) -> bool {
        true
    }

    fn remove(&self, context: &UserContext, id: MemoryId) -> Result<()>;

    /// Best matches for `query`, best first, scoped to this user.
    ///
    /// Returns `(id, score)` pairs where `score` is the raw BM25 weight —
    /// unbounded, and on a scale that moves with the corpus. It is
    /// therefore *not* comparable to the vector leg's similarity, and must
    /// be saturated before it can be read as relevance; a higher number
    /// only ever means "this leg matched it better".
    fn search(
        &self,
        context: &UserContext,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>>;
}
