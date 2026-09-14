//! Stores what the caller meant when the caller said what they meant.
//!
//! # The problem this routes around
//!
//! With a language model configured, every submission goes through
//! extraction: the content is read for durable facts, which are paraphrased,
//! split, categorised and reconciled. That is the right default for "here is
//! a conversation, remember what matters in it".
//!
//! It is the wrong answer to "store this sentence, under this category". A
//! caller who named the category has already done the classification the
//! extractor exists to perform, and handing their exact wording to a model
//! that is instructed to distil it produces the three failures the caller was
//! not expecting: their one memory became three, their category was replaced
//! by the model's, and their phrasing was rewritten — silently, and reported
//! back as success.
//!
//! # The rule
//!
//! A named category is taken as "verbatim, filed here", and this routes the
//! payload to [`VerbatimIngestor`](super::verbatim_ingestor::VerbatimIngestor)
//! instead. No category is taken as "you decide", which is what extraction is
//! for.
//!
//! # The cost, stated plainly
//!
//! The verbatim path does not reconcile. A category-bearing save stores a
//! contradiction alongside what it contradicts rather than superseding it, and
//! stores a duplicate alongside the original. That is the same trade every
//! `[understanding].provider = "none"` installation already makes, and it is
//! the right trade *here* only because the caller asked for their own words
//! to be kept: the alternative is to keep silently rewriting them, which is
//! worse, and impossible to work around from the client side.
//!
//! Callers who want both — their category, and reconciliation — have the
//! extraction path with a category *hint*, which the extractor is prompted to
//! respect but may override.

use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use crate::understanding::domain::ingest_job::IngestPayload;
use crate::understanding::domain::ingest_pipeline::{IngestOutcome, IngestPipeline};
use std::sync::Arc;

pub struct IntentFirstIngestor {
    /// Extraction and reconciliation: the default, for content the caller
    /// did not classify.
    understanding: Arc<dyn IngestPipeline>,
    /// Store-as-sent: what a caller who named a category is asking for.
    verbatim: Arc<dyn IngestPipeline>,
}

impl IntentFirstIngestor {
    pub fn new(understanding: Arc<dyn IngestPipeline>, verbatim: Arc<dyn IngestPipeline>) -> Self {
        Self {
            understanding,
            verbatim,
        }
    }
}

#[async_trait::async_trait]
impl IngestPipeline for IntentFirstIngestor {
    async fn execute(
        &self,
        context: &UserContext,
        payload: &IngestPayload,
    ) -> Result<IngestOutcome> {
        if payload.category.is_some() {
            self.verbatim.execute(context, payload).await
        } else {
            self.understanding.execute(context, payload).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory_repository::MemoryRepository;
    use crate::understanding::application::candidate_extractor::CandidateExtractor;
    use crate::understanding::application::memory_ingestor::MemoryIngestor;
    use crate::understanding::application::memory_reconciler::MemoryReconciler;
    use crate::understanding::application::scripted_chat_model::ScriptedChatModel;
    use crate::understanding::application::verbatim_ingestor::VerbatimIngestor;
    use crate::understanding::domain::chat_model::ChatModel;
    use crate::understanding::domain::ingest_pipeline::IngestStatus;
    use crate::understanding::domain::taxonomy::Taxonomy;
    use serde_json::json;

    fn payload(content: &str, category: Option<&str>) -> IngestPayload {
        IngestPayload {
            content: content.to_string(),
            category: category.map(str::to_string),
            tags: vec![],
            client: Some("claude-code".to_string()),
            session_id: None,
        }
    }

    /// The real verbatim pipeline, over in-memory stores.
    fn verbatim(fixture: &Fixture) -> Arc<dyn IngestPipeline> {
        Arc::new(VerbatimIngestor::new(Arc::new(fixture.saver()), vec![]))
    }

    /// The real extraction pipeline, scripted to rewrite whatever it is
    /// given into a different memory under a different category — which is
    /// exactly what a caller asking for verbatim storage must not receive.
    fn understanding(
        fixture: &Fixture,
        replies: Vec<serde_json::Value>,
    ) -> (Arc<dyn IngestPipeline>, Arc<ScriptedChatModel>) {
        let mut scripted = ScriptedChatModel::new();
        for reply in replies {
            scripted = scripted.queue(reply);
        }
        let model = Arc::new(scripted);

        let pipeline = MemoryIngestor::new(
            Arc::new(CandidateExtractor::new(
                Arc::clone(&model) as Arc<dyn ChatModel>,
                Arc::new(Taxonomy::new(vec![])),
                false,
            )),
            Arc::new(MemoryReconciler::new(
                Arc::new(fixture.recaller()),
                Arc::new(fixture.saver()),
                Arc::new(fixture.forgetter()),
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                Arc::clone(&model) as Arc<dyn ChatModel>,
                None,
                true,
            )),
        );
        (Arc::new(pipeline), model)
    }

    #[tokio::test]
    async fn a_named_category_is_stored_as_sent_instead_of_extracted() {
        // The reported bug. The caller said what it was and how it read;
        // extraction turned it into something else and called it a success.
        let fixture = Fixture::new();
        let (understanding, model) = understanding(
            &fixture,
            vec![json!({"candidates": [
                {"content": "The user expressed a packaging preference",
                 "category": "fact.project"},
                {"content": "npm was mentioned as an alternative",
                 "category": "fact.project"}
            ]})],
        );
        let ingestor = IntentFirstIngestor::new(understanding, verbatim(&fixture));

        ingestor
            .execute(
                &fixture.alex,
                &payload(
                    "Use pnpm, not npm, in this repository",
                    Some("preference.coding"),
                ),
            )
            .await
            .unwrap();

        assert_eq!(
            model.call_count(),
            0,
            "a caller who named the category was still sent through extraction"
        );

        let stored = fixture
            .memories
            .list(&fixture.alex, true)
            .unwrap()
            .into_iter()
            .map(|memory| (memory.content().to_string(), memory.category().clone()))
            .collect::<Vec<_>>();

        assert_eq!(
            stored,
            vec![(
                "Use pnpm, not npm, in this repository".to_string(),
                Category::PreferenceCoding
            )],
            "the caller's wording or category did not survive the pipeline"
        );
    }

    #[tokio::test]
    async fn unclassified_content_still_goes_through_extraction() {
        // Routing must not cost anything to the ordinary case: content with
        // no category is exactly what extraction is for.
        let fixture = Fixture::new();
        let (understanding, model) = understanding(
            &fixture,
            vec![json!({"candidates": [
                {"content": "The project deploys on Hetzner", "category": "fact.project"}
            ]})],
        );
        let ingestor = IntentFirstIngestor::new(understanding, verbatim(&fixture));

        let outcome = ingestor
            .execute(
                &fixture.alex,
                &payload("we moved off fly.io to hetzner", None),
            )
            .await
            .unwrap();

        assert_eq!(model.call_count(), 1, "extraction should have run");
        assert_eq!(outcome.status, IngestStatus::Stored);
        assert_eq!(outcome.memory_ids.len(), 1);
    }

    #[tokio::test]
    async fn the_outcome_of_whichever_path_ran_is_passed_through() {
        // The router must not flatten the answer it is handed: "nothing
        // durable" and "already known" are different replies to a user.
        let fixture = Fixture::new();
        let (understanding, _) = understanding(&fixture, vec![json!({"candidates": []})]);
        let ingestor = IntentFirstIngestor::new(understanding, verbatim(&fixture));

        let outcome = ingestor
            .execute(&fixture.alex, &payload("thanks!", None))
            .await
            .unwrap();

        assert_eq!(outcome.status, IngestStatus::NothingDurable);
        assert!(outcome.memory_ids.is_empty());
    }
}
