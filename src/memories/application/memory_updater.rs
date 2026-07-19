//! Edits an existing memory, keeping both indexes in step.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::embedder::Embedder;
use crate::memories::domain::memory::{Memory, MemoryEdit};
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::clock::Clock;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use std::sync::Arc;

pub struct MemoryUpdater {
    memories: Arc<dyn MemoryRepository>,
    vectors: Arc<dyn VectorIndex>,
    text: Arc<dyn TextIndex>,
    embedder: Arc<dyn Embedder>,
    clock: Arc<dyn Clock>,
}

impl MemoryUpdater {
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

    pub fn execute(
        &self,
        context: &UserContext,
        id: MemoryId,
        edit: MemoryEdit,
        actor: &str,
    ) -> Result<Memory> {
        let existing = self
            .memories
            .find(context, id)?
            .ok_or_else(|| RaError::NotFound(format!("memory {id} not found")))?;

        let content_changed = edit
            .content
            .as_ref()
            .is_some_and(|content| content.trim() != existing.content());

        let updated = existing.edit(edit, self.clock.now())?;
        self.memories.update(context, &updated, actor)?;

        // Re-embedding is the expensive part, so it only happens when the
        // text actually changed. A tag-only edit leaves the vector valid.
        if content_changed {
            let embedding = self.embedder.embed_one(updated.content())?;
            self.vectors.upsert(context, updated.id(), &embedding)?;
        }

        // Tags and category are indexed too, so the text index is
        // refreshed on any edit.
        if let Err(error) = self.text.upsert(context, &updated) {
            tracing::warn!(
                memory_id = %updated.id(),
                %error,
                "memory updated but its keyword index entry is stale"
            );
        }

        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, now};
    use crate::memories::domain::category::Category;
    use crate::memories::domain::recall_query::RecallQuery;

    fn content_edit(content: &str) -> MemoryEdit {
        MemoryEdit {
            content: Some(content.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn updates_the_content_and_bumps_updated_at() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "original");

        let updated = fixture
            .updater()
            .execute(&fixture.alex, memory.id(), content_edit("revised"), "test")
            .unwrap();

        assert_eq!(updated.content(), "revised");
        assert_eq!(updated.created_at(), memory.created_at());
        assert_eq!(updated.updated_at(), now());
    }

    #[test]
    fn an_edited_memory_is_recalled_by_its_new_wording() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "deploys on flyio");

        fixture
            .updater()
            .execute(
                &fixture.alex,
                memory.id(),
                content_edit("deploys on hetzner"),
                "test",
            )
            .unwrap();

        let results = fixture
            .recaller()
            .execute(&fixture.alex, &RecallQuery::new("hetzner", 5).unwrap())
            .unwrap();

        assert_eq!(results.len(), 1, "the new wording should be findable");
        assert_eq!(results[0].memory.content(), "deploys on hetzner");
    }

    #[test]
    fn a_tag_only_edit_does_not_re_embed() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "unchanged content");

        // If this re-embedded, the injected failure would surface.
        fixture.embedder.fail_next();

        let updated = fixture
            .updater()
            .execute(
                &fixture.alex,
                memory.id(),
                MemoryEdit {
                    tags: Some(vec!["added".to_string()]),
                    ..Default::default()
                },
                "test",
            )
            .unwrap();

        assert_eq!(updated.tags(), &["added".to_string()]);
    }

    #[test]
    fn can_change_the_category() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "we chose sqlite");

        let updated = fixture
            .updater()
            .execute(
                &fixture.alex,
                memory.id(),
                MemoryEdit {
                    category: Some(Category::Decision),
                    ..Default::default()
                },
                "test",
            )
            .unwrap();

        assert_eq!(updated.category(), &Category::Decision);
    }

    #[test]
    fn cannot_update_another_users_memory() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "alex's memory");

        let result =
            fixture
                .updater()
                .execute(&fixture.sam, memory.id(), content_edit("hijacked"), "sam");

        assert!(matches!(result, Err(RaError::NotFound(_))));
        assert_eq!(
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .unwrap()
                .content(),
            "alex's memory"
        );
    }

    #[test]
    fn rejects_an_invalid_edit() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "original");

        assert!(
            fixture
                .updater()
                .execute(&fixture.alex, memory.id(), content_edit("   "), "test")
                .is_err()
        );
        assert_eq!(
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .unwrap()
                .content(),
            "original"
        );
    }

    #[test]
    fn updating_a_missing_memory_is_not_found() {
        let fixture = Fixture::new();
        assert!(matches!(
            fixture
                .updater()
                .execute(&fixture.alex, MemoryId::new(), content_edit("x"), "test"),
            Err(RaError::NotFound(_))
        ));
    }
}
