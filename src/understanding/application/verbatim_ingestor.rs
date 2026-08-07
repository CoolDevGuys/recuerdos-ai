//! Ingestion with no language model: store what was sent.
//!
//! `[understanding].provider = "none"` is the default, so this is what
//! most installations actually run. It exists so that every surface keeps
//! working without a provider — same endpoints, same job records, same
//! responses — and turning understanding on later is a config edit rather
//! than a client change.
//!
//! # What it does not do
//!
//! It does not split content into atomic memories, and it does not
//! reconcile: a contradiction is stored alongside what it contradicts,
//! and both come back on recall. That is the honest cost of running
//! without a model, and the reason the pipeline is worth turning on.
//!
//! The one inference it makes is the category, from phrasing that is
//! unambiguous in English — "always", "never", "I prefer". A rule you can
//! read in ten lines is a different thing from a guess: it is either
//! right or obviously wrong, and it beats filing every memory under one
//! catch-all label where category filters stop meaning anything.

use crate::identity::domain::user_context::UserContext;
use crate::memories::application::direct_memory_saver::DirectMemorySaver;
use crate::memories::domain::category::Category;
use crate::memories::domain::memory::{MemorySource, NewMemory};
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use crate::understanding::application::memory_ingestor::DEFAULT_ACTOR;
use crate::understanding::domain::ingest_job::IngestPayload;
use crate::understanding::domain::ingest_pipeline::IngestPipeline;
use std::sync::Arc;

pub struct VerbatimIngestor {
    saver: Arc<DirectMemorySaver>,
    extra_categories: Vec<String>,
}

impl VerbatimIngestor {
    pub fn new(saver: Arc<DirectMemorySaver>, extra_categories: Vec<String>) -> Self {
        Self {
            saver,
            extra_categories,
        }
    }
}

#[async_trait::async_trait]
impl IngestPipeline for VerbatimIngestor {
    async fn execute(
        &self,
        context: &UserContext,
        payload: &IngestPayload,
    ) -> Result<Vec<MemoryId>> {
        // The caller's category wins when they gave one — they know more
        // about the content than a keyword rule does. An unparseable one
        // is an error, not something to quietly ignore: a client sending
        // `preference.codeing` should hear about it.
        let category = match payload.category.as_deref() {
            Some(raw) => Category::parse_with_extras(raw, &self.extra_categories)?,
            None => infer_category(&payload.content),
        };

        let new = NewMemory {
            content: payload.content.clone(),
            category,
            subcategory: None,
            tags: payload.tags.clone(),
            entities: Vec::new(),
            // Verbatim: the user said it, we did not infer it.
            confidence: 1.0,
            source: MemorySource {
                client: payload.client.clone(),
                session_id: payload.session_id.clone(),
            },
            expires_at: None,
        };

        let saver = Arc::clone(&self.saver);
        let context = context.clone();
        let actor = payload.client.clone().unwrap_or(DEFAULT_ACTOR.to_string());

        let memory = tokio::task::spawn_blocking(move || saver.execute(&context, new, &actor))
            .await
            .map_err(|error| {
                crate::shared::error::RaError::Internal(format!("a save task panicked: {error}"))
            })??;

        Ok(vec![memory.id()])
    }
}

/// Phrases that mark a standing instruction rather than a fact.
///
/// Kept short and unambiguous on purpose. Every entry here is a phrase
/// whose presence genuinely means "this is a rule I want followed"; a
/// longer list would start guessing, and a wrong category is worse than
/// the neutral default because it makes a category filter lie.
const PREFERENCE_MARKERS: &[&str] = &[
    "i prefer",
    "i always",
    "i never",
    "always use",
    "never use",
    "always write",
    "never write",
    "don't use",
    "do not use",
    "make sure to",
    "from now on",
];

/// Marks a decision and its rationale.
const DECISION_MARKERS: &[&str] = &["we decided", "we chose", "we're going with", "we went with"];

fn infer_category(content: &str) -> Category {
    let lowered = content.to_ascii_lowercase();

    if DECISION_MARKERS.iter().any(|m| lowered.contains(m)) {
        return Category::Decision;
    }
    if PREFERENCE_MARKERS.iter().any(|m| lowered.contains(m)) {
        return Category::PreferenceCoding;
    }
    // The neutral default: asserts the content is true of the user's work
    // without claiming to know it is a standing rule.
    Category::FactProject
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::memory_repository::MemoryRepository;
    use crate::memories::domain::recall_query::RecallQuery;
    use crate::shared::error::RaError;

    fn ingestor(fixture: &Fixture) -> VerbatimIngestor {
        VerbatimIngestor::new(Arc::new(fixture.saver()), vec!["fact.homelab".to_string()])
    }

    fn payload(content: &str) -> IngestPayload {
        IngestPayload {
            content: content.to_string(),
            category: None,
            tags: vec![],
            client: Some("rest".to_string()),
            session_id: None,
        }
    }

    #[tokio::test]
    async fn content_is_stored_exactly_as_submitted() {
        let fixture = Fixture::new();

        let stored = ingestor(&fixture)
            .execute(&fixture.alex, &payload("The backend runs on Hetzner"))
            .await
            .unwrap();

        let memory = fixture
            .memories
            .find(&fixture.alex, stored[0])
            .unwrap()
            .unwrap();
        assert_eq!(memory.content(), "The backend runs on Hetzner");
        assert_eq!(memory.confidence(), 1.0, "nothing was inferred");
    }

    #[tokio::test]
    async fn a_standing_instruction_is_filed_as_a_preference() {
        // Otherwise every memory lands in one category and filtering by
        // category stops telling a caller anything.
        let fixture = Fixture::new();

        for content in [
            "I prefer pnpm over npm",
            "Always use table-driven tests",
            "Don't use default exports",
        ] {
            let stored = ingestor(&fixture)
                .execute(&fixture.alex, &payload(content))
                .await
                .unwrap();
            let memory = fixture
                .memories
                .find(&fixture.alex, stored[0])
                .unwrap()
                .unwrap();

            assert_eq!(
                memory.category(),
                &Category::PreferenceCoding,
                "{content:?} should read as a preference"
            );
        }
    }

    #[tokio::test]
    async fn a_decision_is_recognised_by_its_phrasing() {
        let fixture = Fixture::new();

        let stored = ingestor(&fixture)
            .execute(
                &fixture.alex,
                &payload("We decided to use SQLite because the installer has to stay small"),
            )
            .await
            .unwrap();

        let memory = fixture
            .memories
            .find(&fixture.alex, stored[0])
            .unwrap()
            .unwrap();
        assert_eq!(memory.category(), &Category::Decision);
    }

    #[tokio::test]
    async fn anything_else_gets_the_neutral_default() {
        let fixture = Fixture::new();

        let stored = ingestor(&fixture)
            .execute(&fixture.alex, &payload("The API is written in Rust"))
            .await
            .unwrap();

        let memory = fixture
            .memories
            .find(&fixture.alex, stored[0])
            .unwrap()
            .unwrap();
        assert_eq!(memory.category(), &Category::FactProject);
    }

    #[tokio::test]
    async fn a_caller_supplied_category_beats_the_heuristic() {
        // The client saw the content in context; a keyword rule did not.
        let fixture = Fixture::new();

        let stored = ingestor(&fixture)
            .execute(
                &fixture.alex,
                &IngestPayload {
                    category: Some("experience".to_string()),
                    ..payload("I prefer pnpm")
                },
            )
            .await
            .unwrap();

        let memory = fixture
            .memories
            .find(&fixture.alex, stored[0])
            .unwrap()
            .unwrap();
        assert_eq!(memory.category(), &Category::Experience);
    }

    #[tokio::test]
    async fn a_configured_extra_category_is_accepted() {
        let fixture = Fixture::new();

        let stored = ingestor(&fixture)
            .execute(
                &fixture.alex,
                &IngestPayload {
                    category: Some("fact.homelab".to_string()),
                    ..payload("The NAS runs TrueNAS")
                },
            )
            .await
            .unwrap();

        let memory = fixture
            .memories
            .find(&fixture.alex, stored[0])
            .unwrap()
            .unwrap();
        assert_eq!(memory.category().as_str(), "fact.homelab");
    }

    #[tokio::test]
    async fn an_unknown_category_is_an_error_rather_than_silently_ignored() {
        // A client sending `preference.codeing` needs to hear about it,
        // not have it quietly become something else.
        let fixture = Fixture::new();

        let error = ingestor(&fixture)
            .execute(
                &fixture.alex,
                &IngestPayload {
                    category: Some("preference.codeing".to_string()),
                    ..payload("x")
                },
            )
            .await
            .unwrap_err();

        assert!(matches!(error, RaError::Validation(_)), "got {error:?}");
    }

    #[tokio::test]
    async fn contradictions_accumulate_because_nothing_reconciles_them() {
        // The honest cost of running without a model, asserted so it is a
        // documented limitation rather than a surprise.
        let fixture = Fixture::new();
        let ingestor = ingestor(&fixture);

        ingestor
            .execute(&fixture.alex, &payload("Backend deploys on Fly.io"))
            .await
            .unwrap();
        ingestor
            .execute(&fixture.alex, &payload("Backend deploys on Hetzner"))
            .await
            .unwrap();

        let recalled = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &RecallQuery::new("Backend deploys", 10).unwrap(),
            )
            .unwrap();

        assert_eq!(
            recalled.len(),
            2,
            "without a provider both survive — this is what turning understanding on fixes"
        );
    }
}
