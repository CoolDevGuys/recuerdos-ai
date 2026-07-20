//! A finished session in, the few durable things it produced out.
//!
//! # Why this is not just `POST /v1/memories` with a longer body
//!
//! It is the same pipeline — extraction then reconciliation — pointed at
//! different material with a different question. A submission is
//! something a caller chose to record and the job is to label it. A
//! session is thousands of words nobody chose to record, nearly all of
//! them about the task at hand, and the job is to throw almost all of it
//! away. See `understanding::domain::extraction_prompt::Lens`.
//!
//! Reusing the pipeline rather than writing a second one is what makes
//! distillation inherit reconciliation for free: a convention established
//! in this session that contradicts one from three months ago supersedes
//! it, instead of landing beside it.
//!
//! # Why it refuses to run without a model
//!
//! Every other surface degrades to storing content verbatim when
//! `[understanding].provider = "none"`. Here that would mean storing an
//! entire transcript as a single memory — an enormous, unrecallable blob
//! that then gets spent from a context window on every future recall it
//! matches. Refusing is the honest answer: distillation *is* the model
//! call, and there is nothing left of it to degrade to.

use crate::consolidation::domain::distillation::{Distillation, SessionTranscript};
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::{RaError, Result};
use crate::understanding::domain::ingest_job::IngestPayload;
use crate::understanding::domain::ingest_pipeline::IngestPipeline;
use std::sync::Arc;

/// Recorded as the client on distilled memories when the caller did not
/// name one, so the audit trail can tell a distillation from a save.
pub const DEFAULT_CLIENT: &str = "session-distill";

pub struct SessionDistiller {
    /// The pipeline built with the session lens — not the one serving
    /// `POST /v1/memories`.
    pipeline: Arc<dyn IngestPipeline>,
    /// Whether a language model is configured at all.
    understanding: bool,
}

impl SessionDistiller {
    pub fn new(pipeline: Arc<dyn IngestPipeline>, understanding: bool) -> Self {
        Self {
            pipeline,
            understanding,
        }
    }

    pub async fn execute(
        &self,
        context: &UserContext,
        transcript: &SessionTranscript,
    ) -> Result<Distillation> {
        if !self.understanding {
            return Err(RaError::Validation(
                "session distillation needs a language model: set \
                 [understanding].provider to something other than \"none\". \
                 Without one there is nothing to distil a transcript down to, and \
                 storing it whole would be worse than storing nothing."
                    .to_string(),
            ));
        }

        let payload = IngestPayload {
            content: transcript.content().to_string(),
            // No category hint, deliberately. A session yields memories
            // of several kinds — a root cause, a convention, a project
            // fact — and suggesting one would pull them all under it.
            category: None,
            tags: transcript.tags.clone(),
            client: Some(
                transcript
                    .client
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CLIENT.to_string()),
            ),
            session_id: transcript.session_id.clone(),
        };

        let memory_ids = self.pipeline.execute(context, &payload).await?;

        tracing::info!(
            distilled = memory_ids.len(),
            session_id = transcript.session_id.as_deref().unwrap_or("-"),
            "session distilled"
        );

        Ok(Distillation { memory_ids })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory_repository::MemoryRepository;
    use crate::memories::domain::recall_query::RecallQuery;
    use crate::understanding::application::candidate_extractor::CandidateExtractor;
    use crate::understanding::application::memory_ingestor::MemoryIngestor;
    use crate::understanding::application::memory_reconciler::MemoryReconciler;
    use crate::understanding::application::scripted_chat_model::ScriptedChatModel;
    use crate::understanding::application::verbatim_ingestor::VerbatimIngestor;
    use crate::understanding::domain::chat_model::ChatModel;
    use crate::understanding::domain::taxonomy::Taxonomy;
    use serde_json::json;

    /// The real pipeline, built with the session lens, over in-memory
    /// stores and a scripted model.
    fn distiller(
        fixture: &Fixture,
        model: ScriptedChatModel,
    ) -> (SessionDistiller, Arc<ScriptedChatModel>) {
        let model = Arc::new(model);
        let shared = Arc::clone(&model) as Arc<dyn ChatModel>;

        let pipeline = MemoryIngestor::new(
            Arc::new(CandidateExtractor::for_sessions(
                Arc::clone(&shared),
                Arc::new(Taxonomy::new(vec![])),
            )),
            Arc::new(MemoryReconciler::new(
                Arc::new(fixture.recaller()),
                Arc::new(fixture.saver()),
                Arc::new(fixture.forgetter()),
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                shared,
                true,
            )),
        );

        (SessionDistiller::new(Arc::new(pipeline), true), model)
    }

    fn transcript(content: &str) -> SessionTranscript {
        SessionTranscript::new(content).unwrap()
    }

    #[tokio::test]
    async fn a_long_session_yields_the_few_things_that_outlive_it() {
        // The plan's worked example: a bug fix, a new convention and a
        // project fact survive; the back-and-forth around them does not.
        let fixture = Fixture::new();
        let (distiller, _) = distiller(
            &fixture,
            ScriptedChatModel::new()
                .queue(json!({"candidates": [
                    {"content": "Session tokens were expiring early because the refresh \
                                 timer used local time instead of UTC",
                     "category": "experience", "tags": ["auth", "bug"]},
                    {"content": "User requires table-driven tests for new Go packages",
                     "category": "preference.coding", "tags": ["go", "testing"]},
                    {"content": "The billing service exposes a /v2/invoices endpoint",
                     "category": "fact.project", "tags": ["billing"]}
                ]}))
                // The first candidate has nothing to compare against and is
                // stored outright; the next two each find it as a neighbour,
                // so reconciliation runs and says they are about other things.
                .queue(json!({"decisions": [
                    {"action": "ADD", "reason": "a convention, not a bug report"}
                ]}))
                .queue(json!({"decisions": [
                    {"action": "ADD", "reason": "a project fact, unrelated to either"}
                ]})),
        );

        let distillation = distiller
            .execute(
                &fixture.alex,
                &transcript(
                    "200 messages of debugging a token expiry bug, agreeing on a \
                     testing convention, and shipping the invoices endpoint",
                ),
            )
            .await
            .unwrap();

        assert_eq!(distillation.memory_ids.len(), 3);

        let stored: Vec<Category> = distillation
            .memory_ids
            .iter()
            .map(|id| {
                fixture
                    .memories
                    .find(&fixture.alex, *id)
                    .unwrap()
                    .unwrap()
                    .category()
                    .clone()
            })
            .collect();
        assert!(stored.contains(&Category::Experience), "{stored:?}");
        assert!(stored.contains(&Category::PreferenceCoding), "{stored:?}");
        assert!(stored.contains(&Category::FactProject), "{stored:?}");
    }

    #[tokio::test]
    async fn chit_chat_produces_nothing() {
        // The DoD case, and the common one. A session that yields nothing
        // is a success, not an error — treating it otherwise would make
        // every PreCompact hook look broken.
        let fixture = Fixture::new();
        let (distiller, _) = distiller(
            &fixture,
            ScriptedChatModel::new().queue(json!({"candidates": []})),
        );

        let distillation = distiller
            .execute(
                &fixture.alex,
                &transcript("thanks! that worked. ok, running the tests now. all green"),
            )
            .await
            .unwrap();

        assert!(distillation.memory_ids.is_empty());
        assert!(
            fixture
                .memories
                .list(&fixture.alex, true)
                .unwrap()
                .is_empty(),
            "a session with nothing durable in it stored something anyway"
        );
    }

    #[tokio::test]
    async fn the_session_is_asked_the_session_question() {
        // The whole reason distillation is its own use case. If it sent
        // the submission prompt, a transcript's task chatter would come
        // back labelled as durable memories.
        let fixture = Fixture::new();
        let (distiller, model) = distiller(
            &fixture,
            ScriptedChatModel::new().queue(json!({"candidates": []})),
        );

        distiller
            .execute(&fixture.alex, &transcript("a session"))
            .await
            .unwrap();

        let prompt = model.prompt(0);
        assert!(
            prompt.contains("still true after this session ends"),
            "distillation used the wrong lens: {prompt}"
        );
    }

    #[tokio::test]
    async fn a_convention_from_this_session_supersedes_an_older_one() {
        // Distillation inherits reconciliation by reusing the pipeline.
        // Without it, every session would pile a fresh copy of the
        // current convention on top of the last one.
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "The project uses npm");

        let (distiller, _) = distiller(
            &fixture,
            ScriptedChatModel::new()
                .queue(json!({"candidates": [
                    {"content": "The project uses pnpm", "category": "preference.coding"}
                ]}))
                .queue(json!({"decisions": [
                    {"action": "UPDATE", "memory_id": old.id().to_string(),
                     "reason": "the session switched package managers"}
                ]})),
        );

        distiller
            .execute(&fixture.alex, &transcript("we switched to pnpm today"))
            .await
            .unwrap();

        let recalled: Vec<String> = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &RecallQuery::new("package manager", 10).unwrap(),
            )
            .unwrap()
            .into_iter()
            .map(|scored| scored.memory.content().to_string())
            .collect();

        assert_eq!(
            recalled,
            ["The project uses pnpm"],
            "the superseded convention is still being recalled"
        );
    }

    #[tokio::test]
    async fn the_session_is_recorded_as_the_source() {
        let fixture = Fixture::new();
        let (distiller, _) = distiller(
            &fixture,
            ScriptedChatModel::new().queue(json!({"candidates": [
                {"content": "User prefers pnpm", "category": "preference.coding"}
            ]})),
        );

        let distillation = distiller
            .execute(
                &fixture.alex,
                &transcript("a session").from(Some("claude-code".to_string()), None),
            )
            .await
            .unwrap();

        let memory = fixture
            .memories
            .find(&fixture.alex, distillation.memory_ids[0])
            .unwrap()
            .unwrap();
        assert_eq!(memory.source().client.as_deref(), Some("claude-code"));
    }

    #[tokio::test]
    async fn without_a_model_it_refuses_instead_of_storing_the_transcript() {
        // Degrading to verbatim here would store the whole session as one
        // memory, which is worse than storing nothing: it is unrecallable
        // and it is spent from a context window every time it matches.
        let fixture = Fixture::new();
        let distiller = SessionDistiller::new(
            Arc::new(VerbatimIngestor::new(Arc::new(fixture.saver()), vec![])),
            false,
        );

        let error = distiller
            .execute(&fixture.alex, &transcript("a long transcript"))
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("[understanding].provider"),
            "the error must name the setting to change: {error}"
        );
        assert!(
            fixture
                .memories
                .list(&fixture.alex, true)
                .unwrap()
                .is_empty(),
            "the transcript was stored anyway"
        );
    }
}
