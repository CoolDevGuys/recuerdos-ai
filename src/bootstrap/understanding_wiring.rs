//! Composition root for the understanding context.
//!
//! Picks a `ChatModel` from `[understanding]` and wraps it in the retry
//! decorator. Everything downstream sees one trait object and cannot tell
//! which provider it got, which is the point: swapping Anthropic for a
//! local Ollama is a config edit, not a code change.

use crate::bootstrap::config::AppConfig;
use crate::bootstrap::memories_wiring::Memories;
use crate::providers::infrastructure::chat::anthropic_chat_model::AnthropicChatModel;
use crate::providers::infrastructure::chat::ollama_chat_model::OllamaChatModel;
use crate::providers::infrastructure::chat::openai_compat_chat_model::OpenAiCompatChatModel;
use crate::providers::infrastructure::chat::retrying_chat_model::RetryingChatModel;
use crate::providers::infrastructure::chat::{
    anthropic_chat_model, ollama_chat_model, openai_compat_chat_model, transport,
};
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::SqliteDatabase;
use crate::understanding::application::candidate_extractor::CandidateExtractor;
use crate::understanding::application::memory_ingestor::MemoryIngestor;
use crate::understanding::application::memory_reconciler::MemoryReconciler;
use crate::understanding::application::verbatim_ingestor::VerbatimIngestor;
use crate::understanding::domain::chat_model::ChatModel;
use crate::understanding::domain::ingest_job::JobQueue;
use crate::understanding::domain::ingest_pipeline::IngestPipeline;
use crate::understanding::domain::taxonomy::Taxonomy;
use crate::understanding::infrastructure::sqlite_job_queue::SqliteJobQueue;
use std::sync::Arc;
use tokio::sync::Notify;

/// Builds the configured chat model, or `None` when understanding is off.
///
/// `provider = "none"` is the default, and returning `None` rather than a
/// no-op model is deliberate: the ingestion path has to *know* it is in
/// degraded mode so it can fall back to storing content verbatim, and a
/// stub that returned empty extractions would look like "the model found
/// nothing worth remembering" and silently drop every memory.
///
/// Credentials are read at startup, not at first use. A missing API key
/// should stop the daemon with a clear message, not surface hours later
/// as a pile of dead-lettered ingestion jobs.
pub fn build_chat_model(config: &AppConfig) -> Result<Option<Arc<dyn ChatModel>>> {
    let understanding = &config.understanding;
    let configured_base = understanding.base_url.trim();
    let base_url = |default: &str| {
        if configured_base.is_empty() {
            default.to_string()
        } else {
            configured_base.to_string()
        }
    };

    let model: Arc<dyn ChatModel> = match understanding.provider.as_str() {
        "none" => return Ok(None),

        "anthropic" => Arc::new(AnthropicChatModel::new(
            &understanding.model,
            transport::key_from_env(&understanding.api_key_env)?,
            &base_url(anthropic_chat_model::DEFAULT_BASE_URL),
        )?),

        "openai-compat" => Arc::new(OpenAiCompatChatModel::new(
            &understanding.model,
            transport::key_from_env(&understanding.api_key_env)?,
            &base_url(openai_compat_chat_model::DEFAULT_BASE_URL),
        )?),

        // No key: Ollama is unauthenticated by design.
        "ollama" => Arc::new(OllamaChatModel::new(
            &understanding.model,
            &base_url(ollama_chat_model::DEFAULT_BASE_URL),
        )?),

        // Unreachable via `AppConfig::load`, which validates the provider
        // name. Kept as an error rather than a panic so that a future
        // provider added to the config list but not here fails to start
        // instead of failing at runtime.
        other => {
            return Err(RaError::Validation(format!(
                "[understanding].provider {other:?} has no implementation"
            )));
        }
    };

    tracing::info!(
        provider = %understanding.provider,
        model = model.model_id(),
        "understanding enabled"
    );

    Ok(Some(Arc::new(RetryingChatModel::new(model))))
}

/// The understanding context as the rest of the process sees it.
///
/// Both fields exist in every mode. That uniformity is the point of the
/// `IngestPipeline` trait: an installation without a provider still gets
/// jobs, job ids, and the same endpoints — it just gets a pipeline that
/// stores content verbatim. Turning understanding on is then a config
/// edit, and no client notices except that the memories get better.
pub struct Understanding {
    pub queue: Arc<dyn JobQueue>,
    pub pipeline: Arc<dyn IngestPipeline>,
    /// Whether a language model is behind the pipeline. Reported on the
    /// job so a caller can tell "stored verbatim" from "extracted".
    pub enabled: bool,
    /// Pinged after an enqueue so a worker picks the job up immediately
    /// rather than on its next poll.
    pub wake: Arc<Notify>,
    /// The model itself, for the consolidation context to build its own
    /// prompts on — distillation, merging, the profile digest.
    ///
    /// Handed over already decorated with retries, so every caller gets
    /// the same backoff policy rather than each remembering to wrap it.
    /// `None` means no provider is configured.
    pub model: Option<Arc<dyn ChatModel>>,
    pub taxonomy: Arc<Taxonomy>,
}

impl Understanding {
    pub fn build(
        config: &AppConfig,
        database: Arc<SqliteDatabase>,
        memories: &Memories,
    ) -> Result<Self> {
        let model = build_chat_model(config)?;
        let taxonomy = Arc::new(Taxonomy::new(
            config.understanding.taxonomy.extra_categories.clone(),
        ));

        let pipeline: Arc<dyn IngestPipeline> = match &model {
            Some(model) => Arc::new(MemoryIngestor::new(
                Arc::new(CandidateExtractor::new(
                    Arc::clone(model),
                    Arc::clone(&taxonomy),
                )),
                Arc::new(MemoryReconciler::new(
                    Arc::clone(&memories.recaller),
                    Arc::clone(&memories.saver),
                    Arc::clone(&memories.forgetter),
                    Arc::clone(&memories.repository),
                    Arc::clone(model),
                    config.understanding.reconcile,
                )),
            )),
            None => Arc::new(VerbatimIngestor::new(
                Arc::clone(&memories.saver),
                config.understanding.taxonomy.extra_categories.clone(),
            )),
        };

        let enabled = config.understanding.provider != "none";
        if !enabled {
            tracing::info!(
                "[understanding].provider = \"none\": submitted content is stored \
                 verbatim. Set a provider to enable extraction and reconciliation."
            );
        }

        Ok(Self {
            queue: Arc::new(SqliteJobQueue::new(database)),
            pipeline,
            enabled,
            wake: Arc::new(Notify::new()),
            model,
            taxonomy,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::config::UnderstandingConfig;

    fn config(provider: &str) -> AppConfig {
        AppConfig {
            understanding: UnderstandingConfig {
                provider: provider.to_string(),
                model: "some-model".to_string(),
                api_key_env: "RECORDAGENT_TEST_CHAT_KEY".to_string(),
                ..UnderstandingConfig::default()
            },
            ..AppConfig::default()
        }
    }

    #[test]
    fn the_default_is_no_model_at_all() {
        assert!(
            build_chat_model(&AppConfig::default()).unwrap().is_none(),
            "zero-egress by default: nothing should be built without opting in"
        );
    }

    #[test]
    fn ollama_needs_no_credentials() {
        let model = build_chat_model(&config("ollama"))
            .expect("ollama must build without an API key")
            .expect("a model");
        assert_eq!(model.model_id(), "some-model");
    }

    #[test]
    fn a_missing_api_key_stops_startup_and_names_the_variable() {
        // Deferring this to first use would turn a one-line fix into a
        // queue of dead-lettered jobs discovered hours later.
        //
        // SAFETY: single-threaded test that removes a variable it owns.
        unsafe { std::env::remove_var("RECORDAGENT_TEST_CHAT_KEY") };

        let error = match build_chat_model(&config("anthropic")) {
            Err(error) => error,
            Ok(_) => panic!("a missing API key must stop startup"),
        };
        assert!(matches!(error, RaError::Validation(_)), "got {error:?}");
        assert!(
            error.to_string().contains("RECORDAGENT_TEST_CHAT_KEY"),
            "the message must name the variable to set: {error}"
        );
    }
}
