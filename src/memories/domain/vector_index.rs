//! The semantic leg of hybrid search.

use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;

pub trait VectorIndex: Send + Sync {
    fn upsert(&self, context: &UserContext, id: MemoryId, embedding: &[f32]) -> Result<()>;

    fn remove(&self, context: &UserContext, id: MemoryId) -> Result<()>;

    /// Nearest neighbours to `embedding`, best first, scoped to this
    /// user. Returns at most `limit` ids.
    fn search(
        &self,
        context: &UserContext,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<MemoryId>>;
}
