//! The semantic leg of hybrid search.

use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;

pub trait VectorIndex: Send + Sync {
    fn upsert(&self, context: &UserContext, id: MemoryId, embedding: &[f32]) -> Result<()>;

    fn remove(&self, context: &UserContext, id: MemoryId) -> Result<()>;

    /// Nearest neighbours to `embedding`, best first, scoped to this user.
    /// Returns at most `limit` `(id, similarity)` pairs.
    ///
    /// `similarity` is cosine similarity rescaled to `0.0..=1.0`: `1.0` for
    /// a vector identical to the query, `0.0` for one pointing the other
    /// way. Ranking cannot use it — the keyword leg's numbers are not on
    /// the same scale, which is why fusion works on ranks — but it is what
    /// lets a caller say "nothing here is actually about my question"
    /// rather than always returning the top of the list. Ranks alone
    /// cannot express that: a rank-1 hit out of a corpus of noise still
    /// looks like a rank-1 hit.
    fn search(
        &self,
        context: &UserContext,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>>;
}
