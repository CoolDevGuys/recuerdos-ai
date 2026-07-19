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

    fn remove(&self, context: &UserContext, id: MemoryId) -> Result<()>;

    /// Best matches for `query`, best first, scoped to this user.
    fn search(&self, context: &UserContext, query: &str, limit: usize) -> Result<Vec<MemoryId>>;
}
