//! Edits many memories at once: re-file them, re-label them, tag or untag
//! them (`POST /v1/memories:bulk`, ai-specs/web-ui-plan.md §4.3), and the
//! member moves behind renaming, merging and deleting a category.
//!
//! # All or nothing, and embed first
//!
//! Every id must be one of the caller's active memories, or nothing
//! happens and the answer names the ones that were not. The new vectors are
//! computed *before* anything is written, so a failing embedder leaves the
//! store exactly as it was rather than holding rows whose vectors describe
//! their old labels. Then the rows go in one transaction, then the vectors
//! and the keyword index.
//!
//! Only memories whose embedded text changed are re-embedded: under
//! `content` document text a re-file never re-embeds; under `enriched` the
//! category, sub-label and tags are part of the vector's text, so it does.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::category::Category;
use crate::memories::domain::document_text::DocumentText;
use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::memories::domain::memory::{Memory, MemoryEdit};
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::clock::Clock;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use std::collections::HashSet;
use std::sync::Arc;

/// The most memories one bulk request may touch.
pub const MAX_BULK_IDS: usize = 500;
/// Texts per embedder call while re-embedding.
const EMBED_CHUNK: usize = 64;

/// What to change on every selected memory. Absent fields are left alone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BulkEdit {
    pub category: Option<Category>,
    /// `Some(None)` clears the sub-label.
    pub subcategory: Option<Option<String>>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
}

impl BulkEdit {
    fn is_empty(&self) -> bool {
        self.category.is_none()
            && self.subcategory.is_none()
            && self.add_tags.is_empty()
            && self.remove_tags.is_empty()
    }

    fn as_memory_edit(&self, memory: &Memory) -> MemoryEdit {
        let tags = (!self.add_tags.is_empty() || !self.remove_tags.is_empty()).then(|| {
            let removed: HashSet<String> = self
                .remove_tags
                .iter()
                .map(|tag| tag.trim().to_ascii_lowercase())
                .collect();
            memory
                .tags()
                .iter()
                .filter(|tag| !removed.contains(tag.as_str()))
                .cloned()
                .chain(self.add_tags.iter().cloned())
                .collect()
        });
        MemoryEdit {
            category: self.category.clone(),
            subcategory: self.subcategory.clone(),
            tags,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct BulkOutcome {
    /// Every selected memory as it is now, changed or not.
    pub memories: Vec<Memory>,
    /// How many actually changed.
    pub changed: usize,
    /// How many were re-embedded because their embedded text changed.
    pub reembedded: usize,
}

pub struct MemoryBulkEditor {
    memories: Arc<dyn MemoryRepository>,
    vectors: Arc<dyn VectorIndex>,
    text: Arc<dyn TextIndex>,
    embedder: Arc<dyn Embedder>,
    clock: Arc<dyn Clock>,
    document_text: DocumentText,
}

impl MemoryBulkEditor {
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
            document_text: DocumentText::default(),
        }
    }

    pub fn with_document_text(mut self, document_text: DocumentText) -> Self {
        self.document_text = document_text;
        self
    }

    /// Applies `edit` to every memory in `ids`.
    pub fn execute(
        &self,
        context: &UserContext,
        ids: &[MemoryId],
        edit: &BulkEdit,
        actor: &str,
    ) -> Result<BulkOutcome> {
        let mut unique = Vec::with_capacity(ids.len());
        let mut seen = HashSet::new();
        for id in ids {
            if seen.insert(*id) {
                unique.push(*id);
            }
        }
        if unique.is_empty() {
            return Err(RaError::Validation("no memories were selected".to_string()));
        }
        if unique.len() > MAX_BULK_IDS {
            return Err(RaError::Validation(format!(
                "at most {MAX_BULK_IDS} memories per bulk edit, got {}",
                unique.len()
            )));
        }
        if edit.is_empty() {
            return Err(RaError::Validation(
                "a bulk edit must change something".to_string(),
            ));
        }

        let found = self.memories.find_many(context, &unique)?;
        let present: HashSet<MemoryId> = found.iter().map(Memory::id).collect();
        let missing: Vec<String> = unique
            .iter()
            .filter(|id| !present.contains(id))
            .map(ToString::to_string)
            .collect();
        if !missing.is_empty() {
            return Err(RaError::NotFound(format!(
                "these memories were not found: {}",
                missing.join(", ")
            )));
        }

        let detail = describe(edit, unique.len());
        self.apply(
            context,
            found,
            |memory| Some(edit.as_memory_edit(memory)),
            actor,
            &detail,
        )
    }

    /// Edits `memories` with whatever `edit_of` says for each (`None`
    /// leaves one alone), all or nothing — the engine under
    /// [`execute`](Self::execute) and the category moves.
    pub(crate) fn apply(
        &self,
        context: &UserContext,
        memories: Vec<Memory>,
        edit_of: impl Fn(&Memory) -> Option<MemoryEdit>,
        actor: &str,
        detail: &str,
    ) -> Result<BulkOutcome> {
        let now = self.clock.now();
        let mut result = Vec::with_capacity(memories.len());
        let mut changed: Vec<Memory> = Vec::new();
        let mut to_embed: Vec<(MemoryId, String)> = Vec::new();

        for before in memories {
            let Some(edit) = edit_of(&before) else {
                result.push(before);
                continue;
            };
            let after = before.clone().edit(edit, now)?;
            let unchanged = after.category() == before.category()
                && after.subcategory() == before.subcategory()
                && after.tags() == before.tags()
                && after.content() == before.content();
            if unchanged {
                result.push(before);
                continue;
            }
            let text_after = self.document_text.of(&after);
            if text_after != self.document_text.of(&before) {
                to_embed.push((after.id(), text_after));
            }
            changed.push(after.clone());
            result.push(after);
        }

        if changed.is_empty() {
            return Ok(BulkOutcome {
                memories: result,
                changed: 0,
                reembedded: 0,
            });
        }

        // Embed before writing: if this fails, nothing has changed.
        let mut embeddings = Vec::with_capacity(to_embed.len());
        for chunk in to_embed.chunks(EMBED_CHUNK) {
            let texts: Vec<String> = chunk.iter().map(|(_, text)| text.clone()).collect();
            let vectors = self.embedder.embed(&texts, EmbeddingTask::Document)?;
            if vectors.len() != chunk.len() {
                return Err(RaError::Internal(format!(
                    "embedder returned {} vectors for {} texts",
                    vectors.len(),
                    chunk.len()
                )));
            }
            embeddings.extend(chunk.iter().map(|(id, _)| *id).zip(vectors));
        }

        self.memories
            .update_many(context, &changed, actor, detail)?;

        for (id, embedding) in &embeddings {
            self.vectors.upsert(context, *id, embedding)?;
        }
        if let Err(error) = self.text.upsert_many(context, &changed) {
            tracing::warn!(%error, count = changed.len(), "bulk edit applied but the keyword index is stale");
        }

        Ok(BulkOutcome {
            memories: result,
            changed: changed.len(),
            reembedded: embeddings.len(),
        })
    }
}

/// The audit detail every memory in one bulk edit shares.
fn describe(edit: &BulkEdit, count: usize) -> String {
    let mut parts = Vec::new();
    if let Some(category) = &edit.category {
        parts.push(format!("category → {category}"));
    }
    match &edit.subcategory {
        Some(Some(sub)) => parts.push(format!("sub-label → {sub}")),
        Some(None) => parts.push("sub-label cleared".to_string()),
        None => {}
    }
    if !edit.add_tags.is_empty() {
        parts.push(format!("+tags {}", edit.add_tags.join(", ")));
    }
    if !edit.remove_tags.is_empty() {
        parts.push(format!("-tags {}", edit.remove_tags.join(", ")));
    }
    format!(
        "bulk edit of {count} {}: {}",
        if count == 1 { "memory" } else { "memories" },
        parts.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::memory_repository::AuditOperation;

    fn editor(fixture: &Fixture, document_text: DocumentText) -> MemoryBulkEditor {
        fixture.bulk_editor().with_document_text(document_text)
    }

    fn recategorise(category: Category) -> BulkEdit {
        BulkEdit {
            category: Some(category),
            ..Default::default()
        }
    }

    #[test]
    fn every_selected_memory_is_refiled_and_audited() {
        let fixture = Fixture::new();
        let a = fixture.save(&fixture.alex, "uses pnpm");
        let b = fixture.save(&fixture.alex, "uses vitest");

        let outcome = editor(&fixture, DocumentText::Content)
            .execute(
                &fixture.alex,
                &[a.id(), b.id()],
                &recategorise(Category::Skill),
                "web-ui",
            )
            .unwrap();

        assert_eq!(outcome.changed, 2);
        for id in [a.id(), b.id()] {
            let stored = fixture.memories.find(&fixture.alex, id).unwrap().unwrap();
            assert_eq!(stored.category(), &Category::Skill);
        }
        let trail = fixture.memories.audit_trail(&fixture.alex, 10).unwrap();
        let bulk: Vec<_> = trail
            .iter()
            .filter(|e| e.operation == AuditOperation::Update && e.actor == "web-ui")
            .collect();
        assert_eq!(bulk.len(), 2);
        assert!(
            bulk[0].detail.contains("category → skill"),
            "{}",
            bulk[0].detail
        );
    }

    #[test]
    fn tags_are_added_and_removed_without_touching_the_others() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "uses pnpm");
        let edit = BulkEdit {
            add_tags: vec!["Tooling".to_string()],
            remove_tags: vec!["nothing-here".to_string()],
            ..Default::default()
        };

        let outcome = editor(&fixture, DocumentText::Content)
            .execute(&fixture.alex, &[memory.id()], &edit, "test")
            .unwrap();

        let tags = outcome.memories[0].tags().to_vec();
        assert!(tags.contains(&"tooling".to_string()), "{tags:?}");
        assert!(memory.tags().iter().all(|t| tags.contains(t)), "{tags:?}");
    }

    #[test]
    fn one_unknown_id_refuses_the_whole_request() {
        let fixture = Fixture::new();
        let mine = fixture.save(&fixture.alex, "mine");
        let theirs = fixture.save(&fixture.sam, "theirs");

        let err = editor(&fixture, DocumentText::Content)
            .execute(
                &fixture.alex,
                &[mine.id(), theirs.id()],
                &recategorise(Category::Skill),
                "test",
            )
            .unwrap_err();

        assert!(matches!(&err, RaError::NotFound(m) if m.contains(&theirs.id().to_string())));
        let untouched = fixture
            .memories
            .find(&fixture.alex, mine.id())
            .unwrap()
            .unwrap();
        assert_eq!(
            untouched.category(),
            mine.category(),
            "nothing may be written"
        );
    }

    #[test]
    fn under_enriched_text_a_refile_re_embeds_and_under_content_it_does_not() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "uses pnpm");

        let content = editor(&fixture, DocumentText::Content)
            .execute(
                &fixture.alex,
                &[memory.id()],
                &recategorise(Category::Skill),
                "t",
            )
            .unwrap();
        let enriched = editor(&fixture, DocumentText::Enriched)
            .execute(
                &fixture.alex,
                &[memory.id()],
                &recategorise(Category::Decision),
                "t",
            )
            .unwrap();

        assert_eq!(content.reembedded, 0);
        assert_eq!(enriched.reembedded, 1);
    }

    #[test]
    fn a_failing_embedder_leaves_every_memory_as_it_was() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "uses pnpm");
        fixture.embedder.fail_next();

        let result = editor(&fixture, DocumentText::Enriched).execute(
            &fixture.alex,
            &[memory.id()],
            &recategorise(Category::Skill),
            "t",
        );

        assert!(result.is_err());
        let stored = fixture
            .memories
            .find(&fixture.alex, memory.id())
            .unwrap()
            .unwrap();
        assert_eq!(stored.category(), memory.category());
    }

    #[test]
    fn limits_are_enforced() {
        let fixture = Fixture::new();
        let editor = editor(&fixture, DocumentText::Content);
        let too_many: Vec<MemoryId> = (0..=MAX_BULK_IDS).map(|_| MemoryId::new()).collect();

        for (ids, edit) in [
            (vec![], recategorise(Category::Skill)),
            (too_many, recategorise(Category::Skill)),
            (vec![MemoryId::new()], BulkEdit::default()),
        ] {
            assert!(matches!(
                editor.execute(&fixture.alex, &ids, &edit, "t"),
                Err(RaError::Validation(_))
            ));
        }
    }
}
