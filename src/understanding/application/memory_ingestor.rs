//! The pipeline, end to end: raw content in, stored memories out.
//!
//! Two steps and no logic of its own. It exists so the worker depends on
//! one thing rather than on the order the two halves have to run in, and
//! so `[understanding].provider = "none"` can substitute a different
//! implementation of the same contract without the worker knowing.

use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use crate::understanding::application::candidate_extractor::CandidateExtractor;
use crate::understanding::application::memory_reconciler::MemoryReconciler;
use crate::understanding::domain::extraction_prompt::SourceHints;
use crate::understanding::domain::ingest_job::IngestPayload;
use crate::understanding::domain::ingest_pipeline::IngestPipeline;
use std::sync::Arc;

/// Recorded as the actor on every memory the pipeline writes when the
/// submission did not name a client. Distinguishable in the audit trail
/// from a memory a client saved directly.
pub const DEFAULT_ACTOR: &str = "pipeline";

pub struct MemoryIngestor {
    extractor: Arc<CandidateExtractor>,
    reconciler: Arc<MemoryReconciler>,
}

impl MemoryIngestor {
    pub fn new(extractor: Arc<CandidateExtractor>, reconciler: Arc<MemoryReconciler>) -> Self {
        Self {
            extractor,
            reconciler,
        }
    }
}

#[async_trait::async_trait]
impl IngestPipeline for MemoryIngestor {
    async fn execute(
        &self,
        context: &UserContext,
        payload: &IngestPayload,
    ) -> Result<Vec<MemoryId>> {
        let hints = SourceHints {
            client: payload.client.clone(),
            category: payload.category.clone(),
            tags: payload.tags.clone(),
        };

        let candidates = self.extractor.execute(&payload.content, &hints).await?;
        if candidates.is_empty() {
            // Short-circuit rather than calling reconciliation with an
            // empty list: nothing to compare, and it makes the common
            // "small talk" case free.
            tracing::debug!("nothing durable found in the submitted content");
            return Ok(Vec::new());
        }

        let actor = payload.client.as_deref().unwrap_or(DEFAULT_ACTOR);
        let outcome = self.reconciler.execute(context, &candidates, actor).await?;

        tracing::info!(
            candidates = candidates.len(),
            stored = outcome.stored.len(),
            superseded = outcome.superseded.len(),
            deleted = outcome.deleted.len(),
            "ingestion complete"
        );

        Ok(outcome.stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::memory_repository::MemoryRepository;
    use crate::memories::domain::recall_query::RecallQuery;
    use crate::understanding::application::scripted_chat_model::ScriptedChatModel;
    use crate::understanding::domain::chat_model::ChatModel;
    use crate::understanding::domain::taxonomy::Taxonomy;
    use serde_json::json;

    fn payload(content: &str) -> IngestPayload {
        IngestPayload {
            content: content.to_string(),
            category: None,
            tags: vec![],
            client: Some("claude-code".to_string()),
            session_id: None,
        }
    }

    /// Wires the real pipeline over in-memory stores and one scripted
    /// model shared by both halves — so the extraction reply is consumed
    /// first and the reconciliation replies follow, exactly as the two
    /// use cases call it.
    fn ingestor(fixture: &Fixture, model: ScriptedChatModel) -> MemoryIngestor {
        let model = Arc::new(model) as Arc<dyn ChatModel>;

        MemoryIngestor::new(
            Arc::new(CandidateExtractor::new(
                Arc::clone(&model),
                Arc::new(Taxonomy::new(vec![])),
            )),
            Arc::new(MemoryReconciler::new(
                Arc::new(fixture.recaller()),
                Arc::new(fixture.saver()),
                Arc::new(fixture.forgetter()),
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                model,
                true,
            )),
        )
    }

    fn recall(fixture: &Fixture, query: &str) -> Vec<String> {
        fixture
            .recaller()
            .execute(&fixture.alex, &RecallQuery::new(query, 10).unwrap())
            .unwrap()
            .into_iter()
            .map(|scored| scored.memory.content().to_string())
            .collect()
    }

    #[tokio::test]
    async fn raw_text_becomes_stored_memories() {
        let fixture = Fixture::new();
        let ingestor = ingestor(
            &fixture,
            ScriptedChatModel::new()
                .queue(json!({"candidates": [
                    {"content": "The backend runs on Hetzner", "category": "fact.project"},
                    {"content": "User requires table-driven tests in Go",
                     "category": "preference.coding"}
                ]}))
                // The first candidate has nothing to compare against and
                // is stored outright. The second then finds it as a
                // neighbour, so reconciliation runs — and says the two
                // are about different things.
                .queue(json!({"decisions": [
                    {"action": "ADD", "reason": "unrelated to the deployment target"}
                ]})),
        );

        let stored = ingestor
            .execute(
                &fixture.alex,
                &payload("we moved to Hetzner; always table-driven tests"),
            )
            .await
            .unwrap();

        assert_eq!(stored.len(), 2);
        // Both are retrievable. Which one ranks first is a property of
        // the fake embedder, not of the pipeline, so it is not asserted.
        let recalled = recall(&fixture, "Hetzner");
        assert!(
            recalled.contains(&"The backend runs on Hetzner".to_string()),
            "{recalled:?}"
        );
        assert!(
            recalled.contains(&"User requires table-driven tests in Go".to_string()),
            "the second candidate never made it into the store: {recalled:?}"
        );
    }

    #[tokio::test]
    async fn the_submitting_client_is_recorded_as_the_source() {
        // So the audit trail can tell an editor's ingestion from a
        // script's, which is the whole reason `--client` exists.
        let fixture = Fixture::new();
        let ingestor = ingestor(
            &fixture,
            ScriptedChatModel::new().queue(json!({"candidates": [
                {"content": "User prefers pnpm", "category": "preference.coding"}
            ]})),
        );

        let stored = ingestor
            .execute(&fixture.alex, &payload("I prefer pnpm"))
            .await
            .unwrap();

        let memory = fixture
            .memories
            .find(&fixture.alex, stored[0])
            .unwrap()
            .unwrap();
        assert_eq!(memory.source().client.as_deref(), Some("claude-code"));
    }

    #[tokio::test]
    async fn small_talk_stores_nothing_and_costs_one_model_call() {
        // Reconciliation is skipped entirely: there is nothing to compare
        // against, and this is the most common submission there is.
        let fixture = Fixture::new();
        let model = Arc::new(ScriptedChatModel::new().queue(json!({"candidates": []})));
        let ingestor = MemoryIngestor::new(
            Arc::new(CandidateExtractor::new(
                Arc::clone(&model) as Arc<dyn ChatModel>,
                Arc::new(Taxonomy::new(vec![])),
            )),
            Arc::new(MemoryReconciler::new(
                Arc::new(fixture.recaller()),
                Arc::new(fixture.saver()),
                Arc::new(fixture.forgetter()),
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                Arc::clone(&model) as Arc<dyn ChatModel>,
                true,
            )),
        );

        let stored = ingestor
            .execute(&fixture.alex, &payload("thanks, that worked!"))
            .await
            .unwrap();

        assert!(stored.is_empty());
        assert_eq!(model.call_count(), 1, "reconciliation should not have run");
    }

    #[tokio::test]
    async fn a_contradiction_supersedes_end_to_end() {
        // The project-plan §12.3 scenario, through the real pipeline:
        // extraction finds the new fact, reconciliation retires the old.
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "Backend deploys on Fly.io");

        let ingestor = ingestor(
            &fixture,
            ScriptedChatModel::new()
                .queue(json!({"candidates": [
                    {"content": "Backend deploys on Hetzner", "category": "fact.project"}
                ]}))
                .queue(json!({"decisions": [
                    {"action": "UPDATE", "memory_id": old.id().to_string(),
                     "reason": "the deployment target changed"}
                ]})),
        );

        ingestor
            .execute(
                &fixture.alex,
                &payload("btw we're switching the backend to Hetzner, fly.io got too expensive"),
            )
            .await
            .unwrap();

        assert_eq!(
            recall(&fixture, "where does the backend deploy"),
            ["Backend deploys on Hetzner"],
            "recall still returns the retired answer"
        );
    }

    #[tokio::test]
    async fn a_provider_failure_propagates_so_the_job_can_retry() {
        let fixture = Fixture::new();
        let ingestor = ingestor(
            &fixture,
            ScriptedChatModel::new().queue_error(
                crate::understanding::domain::chat_model::ChatError::Transient("429".to_string()),
            ),
        );

        let error = ingestor
            .execute(&fixture.alex, &payload("I prefer pnpm"))
            .await
            .unwrap_err();

        assert!(
            crate::understanding::domain::ingest_pipeline::is_retryable(&error),
            "a rate limit must leave the job retryable, not dead-letter it: {error:?}"
        );
    }
}
