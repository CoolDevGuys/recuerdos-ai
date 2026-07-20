//! Stores a memory verbatim, exactly as given.
//!
//! "Direct" distinguishes it from the understanding pipeline arriving in
//! Phase 4, which extracts and reconciles before storing. The public
//! surfaces switch over then; this use case stays as the path for a
//! caller that has already decided what to remember.
//!
//! # Consistency across three stores
//!
//! A save touches SQLite (system of record), the vector index and the
//! text index. They cannot share one transaction — tantivy isn't SQL —
//! so the ordering is chosen to make partial failure harmless:
//!
//! 1. **Embed first.** The most likely failure (a model or network
//!    problem) happens before anything is written.
//! 2. **Insert the row, then the vector.** If the vector fails, the row
//!    is deleted again: a memory findable by keyword but not by meaning
//!    would be an invisible, permanent quality bug.
//! 3. **Index the text last.** If *that* fails the memory still exists
//!    and is semantically recallable, and the text index is derived
//!    state that can be rebuilt.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::embedder::Embedder;
use crate::memories::domain::memory::{Memory, NewMemory};
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::clock::Clock;
use crate::shared::error::Result;
use std::sync::Arc;

pub struct DirectMemorySaver {
    memories: Arc<dyn MemoryRepository>,
    vectors: Arc<dyn VectorIndex>,
    text: Arc<dyn TextIndex>,
    embedder: Arc<dyn Embedder>,
    clock: Arc<dyn Clock>,
}

impl DirectMemorySaver {
    pub fn new(
        memories: Arc<dyn MemoryRepository>,
        vectors: Arc<dyn VectorIndex>,
        text: Arc<dyn TextIndex>,
        embedder: Arc<dyn Embedder>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            memories,
            vectors,
            text,
            embedder,
            clock,
        }
    }

    pub fn execute(&self, context: &UserContext, new: NewMemory, actor: &str) -> Result<Memory> {
        let memory = Memory::create(context.user_id(), new, self.clock.now())?;

        let embedding = self.embedder.embed_one(memory.content())?;

        self.memories.insert(context, &memory, actor)?;

        if let Err(error) = self.vectors.upsert(context, memory.id(), &embedding) {
            // Compensate: without its vector the memory is only half
            // findable, and nothing would ever tell the user.
            if let Err(cleanup) = self.memories.delete(
                context,
                memory.id(),
                actor,
                "rolled back: the vector index write failed",
            ) {
                tracing::error!(
                    memory_id = %memory.id(),
                    %cleanup,
                    "failed to roll back a memory whose vector could not be written"
                );
            }
            return Err(error);
        }

        if let Err(error) = self.text.upsert(context, &memory) {
            // Not fatal: the memory is stored and semantically
            // recallable, and the text index can be rebuilt.
            tracing::warn!(
                memory_id = %memory.id(),
                %error,
                "memory saved but not added to the keyword index"
            );
        }

        Ok(memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, new_memory};
    use crate::shared::error::RaError;

    #[test]
    fn stores_a_memory_and_indexes_it_in_both_legs() {
        let fixture = Fixture::new();

        let memory = fixture
            .saver()
            .execute(&fixture.alex, new_memory("User prefers pnpm"), "test")
            .unwrap();

        assert_eq!(
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .unwrap()
                .content(),
            "User prefers pnpm"
        );
        assert!(fixture.vectors.contains(memory.id()));
        assert!(fixture.text.contains(memory.id()));
    }

    #[test]
    fn a_failed_vector_write_leaves_no_orphaned_memory() {
        let fixture = Fixture::new();
        fixture.vectors.fail_next_upsert();

        let error = fixture
            .saver()
            .execute(&fixture.alex, new_memory("User prefers pnpm"), "test")
            .unwrap_err();

        assert!(matches!(error, RaError::Internal(_)), "got {error:?}");
        assert!(
            fixture
                .memories
                .list(&fixture.alex, true)
                .unwrap()
                .is_empty(),
            "the memory row should have been rolled back"
        );
    }

    #[test]
    fn a_failed_embedding_writes_nothing_at_all() {
        let fixture = Fixture::new();
        fixture.embedder.fail_next();

        assert!(
            fixture
                .saver()
                .execute(&fixture.alex, new_memory("User prefers pnpm"), "test")
                .is_err()
        );

        assert!(
            fixture
                .memories
                .list(&fixture.alex, true)
                .unwrap()
                .is_empty()
        );
        assert!(fixture.vectors.is_empty());
    }

    #[test]
    fn a_failed_text_index_write_still_stores_the_memory() {
        // The text index is derived state; losing a write there must not
        // lose the memory itself.
        let fixture = Fixture::new();
        fixture.text.fail_next_upsert();

        let memory = fixture
            .saver()
            .execute(&fixture.alex, new_memory("User prefers pnpm"), "test")
            .unwrap();

        assert!(
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .is_some()
        );
        assert!(fixture.vectors.contains(memory.id()));
        assert!(!fixture.text.contains(memory.id()));
    }

    #[test]
    fn rejects_invalid_content_before_touching_any_store() {
        let fixture = Fixture::new();

        assert!(
            fixture
                .saver()
                .execute(&fixture.alex, new_memory("   "), "test")
                .is_err()
        );

        assert!(
            fixture
                .memories
                .list(&fixture.alex, true)
                .unwrap()
                .is_empty()
        );
        assert!(fixture.vectors.is_empty());
    }

    #[test]
    fn the_memory_belongs_to_the_authenticated_user() {
        let fixture = Fixture::new();

        let memory = fixture
            .saver()
            .execute(&fixture.alex, new_memory("alex's memory"), "test")
            .unwrap();

        assert_eq!(memory.user_id(), fixture.alex.user_id());
        assert!(
            fixture
                .memories
                .find(&fixture.sam, memory.id())
                .unwrap()
                .is_none(),
            "another user could read it"
        );
    }
}
