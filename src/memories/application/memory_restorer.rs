//! Brings a forgotten memory back: the web UI's undo for "forget".
//!
//! Forgetting is a soft delete that also drops the memory from both
//! indexes (`memory_forgetter.rs`). Undoing it must therefore do more than
//! clear the marker — a row that is active again but absent from the vector
//! and keyword indexes would be a memory recall can never return. So the
//! restorer re-embeds and re-indexes, and if the embedding fails it forgets
//! the memory again rather than leaving it half-restored.
//!
//! Restoring, rather than re-saving the content as a new memory, keeps the
//! id, `created_at` and the audit history: the undo leaves no trace beyond
//! the two audit entries that say it happened.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::document_text::DocumentText;
use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use std::sync::Arc;

pub struct MemoryRestorer {
    memories: Arc<dyn MemoryRepository>,
    vectors: Arc<dyn VectorIndex>,
    text: Arc<dyn TextIndex>,
    embedder: Arc<dyn Embedder>,
    document_text: DocumentText,
}

impl MemoryRestorer {
    pub fn new(
        memories: Arc<dyn MemoryRepository>,
        vectors: Arc<dyn VectorIndex>,
        text: Arc<dyn TextIndex>,
        embedder: Arc<dyn Embedder>,
    ) -> Self {
        Self {
            memories,
            vectors,
            text,
            embedder,
            document_text: DocumentText::default(),
        }
    }

    /// What the vector is rebuilt from — see [`DocumentText`].
    pub fn with_document_text(mut self, document_text: DocumentText) -> Self {
        self.document_text = document_text;
        self
    }

    pub fn execute(&self, context: &UserContext, id: MemoryId, actor: &str) -> Result<Memory> {
        let memory = self.memories.restore(context, id, actor)?;

        let indexed = self
            .embedder
            .embed_one(&self.document_text.of(&memory), EmbeddingTask::Document)
            .and_then(|embedding| self.vectors.upsert(context, memory.id(), &embedding));
        if let Err(error) = indexed {
            // Back to forgotten: an active memory without a vector is
            // invisible to semantic recall, which is worse than an undo that
            // visibly failed and can be retried.
            if let Err(rollback) =
                self.memories
                    .delete(context, id, actor, "restore failed: could not re-embed")
            {
                tracing::error!(memory_id = %id, %rollback, "restore rollback failed");
            }
            return Err(error);
        }

        if let Err(error) = self.text.upsert(context, &memory) {
            tracing::warn!(
                memory_id = %id,
                %error,
                "memory restored but its keyword index entry is missing"
            );
        }

        Ok(memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::memory_repository::AuditOperation;
    use crate::memories::domain::recall_query::RecallQuery;
    use crate::shared::error::RaError;

    fn recall(fixture: &Fixture, query: &str) -> Vec<String> {
        fixture
            .recaller()
            .execute(&fixture.alex, &RecallQuery::new(query, 5).unwrap())
            .unwrap()
            .iter()
            .map(|hit| hit.memory.content().to_string())
            .collect()
    }

    #[test]
    fn a_restored_memory_is_recalled_again() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "deploys on hetzner");
        fixture
            .forgetter()
            .execute(&fixture.alex, memory.id(), "test", "")
            .unwrap();
        assert!(recall(&fixture, "hetzner").is_empty());

        fixture
            .restorer()
            .execute(&fixture.alex, memory.id(), "test")
            .unwrap();

        assert_eq!(recall(&fixture, "hetzner"), vec!["deploys on hetzner"]);
    }

    #[test]
    fn restoring_keeps_the_id_and_creation_time() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "original");
        fixture
            .forgetter()
            .execute(&fixture.alex, memory.id(), "test", "")
            .unwrap();

        let restored = fixture
            .restorer()
            .execute(&fixture.alex, memory.id(), "test")
            .unwrap();

        assert_eq!(restored.id(), memory.id());
        assert_eq!(restored.created_at(), memory.created_at());
    }

    #[test]
    fn restoring_is_audited() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "original");
        fixture
            .forgetter()
            .execute(&fixture.alex, memory.id(), "test", "")
            .unwrap();

        fixture
            .restorer()
            .execute(&fixture.alex, memory.id(), "web-ui")
            .unwrap();

        let trail = fixture.memories.audit_trail(&fixture.alex, 10).unwrap();
        assert!(
            trail
                .iter()
                .any(|e| e.operation == AuditOperation::Restore && e.actor == "web-ui")
        );
    }

    #[test]
    fn a_memory_that_was_never_forgotten_is_not_found() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "still here");

        let err = fixture
            .restorer()
            .execute(&fixture.alex, memory.id(), "test")
            .unwrap_err();

        assert!(matches!(err, RaError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn another_users_forgotten_memory_cannot_be_restored() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "alex's");
        fixture
            .forgetter()
            .execute(&fixture.alex, memory.id(), "test", "")
            .unwrap();

        let err = fixture
            .restorer()
            .execute(&fixture.sam, memory.id(), "test")
            .unwrap_err();

        assert!(matches!(err, RaError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn a_failed_re_embed_leaves_the_memory_forgotten() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "deploys on hetzner");
        fixture
            .forgetter()
            .execute(&fixture.alex, memory.id(), "test", "")
            .unwrap();
        fixture.embedder.fail_next();

        assert!(
            fixture
                .restorer()
                .execute(&fixture.alex, memory.id(), "test")
                .is_err()
        );

        assert!(
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .is_none(),
            "a half-restored memory would be active but unrecallable"
        );
        // And the undo can be retried.
        fixture
            .restorer()
            .execute(&fixture.alex, memory.id(), "test")
            .unwrap();
    }
}
