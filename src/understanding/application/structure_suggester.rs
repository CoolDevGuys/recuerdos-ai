//! **Suggest** in the Needs review queue (ai-specs/memory-structure-plan.md
//! §8): one model call proposes kind, topic and who each selected memory is
//! true for. It writes nothing — the user accepts or changes each proposal,
//! and the bulk edit applies what they chose, under the save rules.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::applies_to::Target;
use crate::memories::domain::kind::Kind;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::project_registry::ProjectRegistry;
use crate::memories::domain::topic_catalog::TopicCatalog;
use crate::shared::blocking::blocking;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use crate::understanding::domain::chat_model::ChatModel;
use crate::understanding::domain::structure_prompt::{
    SuggestionContext, parse_suggestions, structure_request,
};
use std::sync::Arc;

/// The most memories one call asks about: a prompt the model reads whole.
pub const MAX_SUGGEST_IDS: usize = 25;

#[derive(Debug, Clone, PartialEq)]
pub struct StructureSuggestion {
    pub id: MemoryId,
    pub kind: Kind,
    pub topic: String,
    pub target: Target,
    pub project: Option<String>,
    pub reason: String,
}

pub struct StructureSuggester {
    model: Arc<dyn ChatModel>,
    memories: Arc<dyn MemoryRepository>,
    projects: Arc<dyn ProjectRegistry>,
    topics: Arc<dyn TopicCatalog>,
}

impl StructureSuggester {
    pub fn new(
        model: Arc<dyn ChatModel>,
        memories: Arc<dyn MemoryRepository>,
        projects: Arc<dyn ProjectRegistry>,
        topics: Arc<dyn TopicCatalog>,
    ) -> Self {
        Self {
            model,
            memories,
            projects,
            topics,
        }
    }

    /// Proposals for the memories in `ids` that the model could place; a
    /// memory it could not is simply missing from the answer.
    pub async fn execute(
        &self,
        context: &UserContext,
        ids: &[MemoryId],
    ) -> Result<Vec<StructureSuggestion>> {
        if ids.is_empty() {
            return Err(RaError::Validation("no memories were selected".to_string()));
        }
        if ids.len() > MAX_SUGGEST_IDS {
            return Err(RaError::Validation(format!(
                "at most {MAX_SUGGEST_IDS} memories per suggestion, got {}",
                ids.len()
            )));
        }

        let (memories, projects, topics) = {
            let (repository, registry, catalog) = (
                Arc::clone(&self.memories),
                Arc::clone(&self.projects),
                Arc::clone(&self.topics),
            );
            let (context, ids) = (context.clone(), ids.to_vec());
            blocking(move || {
                let found = repository.find_many(&context, &ids)?;
                if found.len() != ids.len() {
                    return Err(RaError::NotFound(
                        "some of these memories were not found".to_string(),
                    ));
                }
                let projects = registry.names(&context)?;
                let topics: Vec<String> = catalog
                    .all(&context)?
                    .iter()
                    .map(|topic| topic.as_str().to_string())
                    .collect();
                Ok((found, projects, topics))
            })
            .await?
        };

        let suggestion_context = SuggestionContext {
            topics: &topics,
            projects: &projects,
        };
        let answer = self
            .model
            .complete_structured(&structure_request(&memories, &suggestion_context))
            .await?;

        Ok(
            parse_suggestions(&answer, memories.len(), &suggestion_context)
                .into_iter()
                .map(|suggestion| StructureSuggestion {
                    id: memories[suggestion.index].id(),
                    kind: suggestion.kind,
                    topic: suggestion.topic,
                    target: suggestion.target,
                    project: suggestion.project,
                    reason: suggestion.reason,
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::understanding::application::scripted_chat_model::ScriptedChatModel;
    use serde_json::json;

    #[tokio::test]
    async fn proposals_come_back_by_id_and_nothing_is_written() {
        let fixture = Fixture::new();
        let registry = fixture.project_registry();
        registry.create(&fixture.alex, "quill", &[]).unwrap();
        let api = fixture.save_fact(&fixture.alex, "The API is written in Rust");
        let nas = fixture.save_fact(&fixture.alex, "The NAS backups are in /mnt/tank");
        let model = Arc::new(ScriptedChatModel::new().queue(json!({"suggestions": [
            {"number": 1, "kind": "fact", "topic": "coding", "target": "project",
             "project": "quill", "reason": "about the quill codebase"},
            {"number": 2, "kind": "reference", "topic": "tech", "target": "general",
             "reason": "a path on the NAS"},
        ]})));
        let suggester = StructureSuggester::new(
            Arc::clone(&model) as Arc<dyn ChatModel>,
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            registry,
            fixture.topic_catalog(),
        );

        let suggestions = suggester
            .execute(&fixture.alex, &[api.id(), nas.id()])
            .await
            .unwrap();

        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0].id, api.id());
        assert_eq!(suggestions[0].project.as_deref(), Some("quill"));
        assert_eq!(suggestions[1].kind, Kind::Reference);
        assert!(model.prompt(0).contains("Projects: quill"));
        let stored = fixture
            .memories
            .find(&fixture.alex, api.id())
            .unwrap()
            .unwrap();
        assert!(
            stored.structure().needs_review(),
            "a suggestion writes nothing"
        );
    }

    #[tokio::test]
    async fn too_many_or_unknown_memories_are_refused_before_the_model() {
        let fixture = Fixture::new();
        let model = Arc::new(ScriptedChatModel::new());
        let suggester = StructureSuggester::new(
            Arc::clone(&model) as Arc<dyn ChatModel>,
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            fixture.project_registry(),
            fixture.topic_catalog(),
        );
        let ids: Vec<MemoryId> = (0..=MAX_SUGGEST_IDS).map(|_| MemoryId::new()).collect();

        assert!(suggester.execute(&fixture.alex, &ids).await.is_err());
        assert!(suggester.execute(&fixture.alex, &ids[..1]).await.is_err());
        assert_eq!(model.call_count(), 0);
    }
}
