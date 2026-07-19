//! Forgets a memory: soft-deletes the row and drops it from both indexes.
//!
//! The row survives so the audit trail stays truthful — "what happened to
//! that memory?" must remain answerable. Actually erasing bytes is a
//! governance operation (project-plan.md §15), deliberately not something
//! an agent can trigger in passing.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use std::sync::Arc;

pub struct MemoryForgetter {
    memories: Arc<dyn MemoryRepository>,
    vectors: Arc<dyn VectorIndex>,
    text: Arc<dyn TextIndex>,
}

impl MemoryForgetter {
    pub fn new(
        memories: Arc<dyn MemoryRepository>,
        vectors: Arc<dyn VectorIndex>,
        text: Arc<dyn TextIndex>,
    ) -> Self {
        Self {
            memories,
            vectors,
            text,
        }
    }

    pub fn execute(&self, context: &UserContext, id: MemoryId, actor: &str) -> Result<()> {
        // The row first: it is the system of record, and if it fails
        // nothing should have been removed from the indexes.
        self.memories.delete(context, id, actor)?;

        // Index removals are best-effort. A leftover entry points at a
        // deleted row, which recall drops when it fetches candidates —
        // stale, but not visible to the user.
        if let Err(error) = self.vectors.remove(context, id) {
            tracing::warn!(memory_id = %id, %error, "failed to remove a vector");
        }
        if let Err(error) = self.text.remove(context, id) {
            tracing::warn!(memory_id = %id, %error, "failed to remove a text index entry");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::memory_repository::AuditOperation;
    use crate::memories::domain::recall_query::RecallQuery;
    use crate::shared::error::RaError;

    #[test]
    fn forgetting_removes_a_memory_from_recall_and_both_indexes() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "a note about pnpm");

        fixture
            .forgetter()
            .execute(&fixture.alex, memory.id(), "test")
            .unwrap();

        assert!(
            fixture
                .recaller()
                .execute(&fixture.alex, &RecallQuery::new("pnpm", 5).unwrap())
                .unwrap()
                .is_empty()
        );
        assert!(!fixture.vectors.contains(memory.id()));
        assert!(!fixture.text.contains(memory.id()));
    }

    #[test]
    fn the_deletion_is_recorded_in_the_audit_trail() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "a note");

        fixture
            .forgetter()
            .execute(&fixture.alex, memory.id(), "mcp")
            .unwrap();

        let audit = fixture.memories.audit_trail(&fixture.alex, 10).unwrap();
        assert!(
            audit.iter().any(|entry| entry.memory_id == memory.id()
                && entry.operation == AuditOperation::Delete
                && entry.actor == "mcp"),
            "the deletion is missing from the trail: {audit:?}"
        );
    }

    #[test]
    fn cannot_forget_another_users_memory() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "alex's note about pnpm");

        let result = fixture
            .forgetter()
            .execute(&fixture.sam, memory.id(), "sam");

        assert!(matches!(result, Err(RaError::NotFound(_))));
        assert!(
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .is_some(),
            "the memory should be untouched"
        );
        assert!(
            fixture.vectors.contains(memory.id()),
            "a rejected delete must not strip the index"
        );
    }

    #[test]
    fn forgetting_a_missing_memory_is_not_found() {
        let fixture = Fixture::new();
        assert!(matches!(
            fixture
                .forgetter()
                .execute(&fixture.alex, MemoryId::new(), "test"),
            Err(RaError::NotFound(_))
        ));
    }
}
