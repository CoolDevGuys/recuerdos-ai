//! Composition root for the memories context.

use crate::bootstrap::config::AppConfig;
use crate::memories::application::direct_memory_saver::DirectMemorySaver;
use crate::memories::application::memory_exporter::MemoryExporter;
use crate::memories::application::memory_finder::MemoryFinder;
use crate::memories::application::memory_forgetter::MemoryForgetter;
use crate::memories::application::memory_recaller::MemoryRecaller;
use crate::memories::application::memory_updater::MemoryUpdater;
use crate::memories::application::profile_assembler::ProfileAssembler;
use crate::memories::domain::embedder::Embedder;
use crate::memories::domain::entity_graph::EntityGraph;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::recall_ranker::{
    DEFAULT_SIMILARITY_BASELINE, RecallRanker, RelevanceCalibration,
    default_similarity_baseline_for,
};
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::memories::infrastructure::sqlite_entity_graph::SqliteEntityGraph;
use crate::memories::infrastructure::sqlite_memory_repository::SqliteMemoryRepository;
use crate::memories::infrastructure::sqlite_vector_index::SqliteVectorIndex;
use crate::memories::infrastructure::tantivy_text_index::TantivyTextIndex;
use crate::providers::infrastructure::chat::transport;
use crate::providers::infrastructure::embeddings::fastembed_embedder::FastembedEmbedder;
use crate::providers::infrastructure::embeddings::gemini_embedder::{self, GeminiEmbedder};
use crate::providers::infrastructure::embeddings::remote_embedder::{
    EmbeddingApi, OLLAMA_DEFAULT_BASE_URL, OPENAI_DEFAULT_BASE_URL, RemoteEmbedder,
};
use crate::shared::clock::{Clock, SystemClock};
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::SqliteDatabase;
use std::sync::Arc;

/// Where the per-user tantivy indexes live, under the data directory.
const TEXT_INDEX_DIR: &str = "text-index";

pub struct Memories {
    pub saver: Arc<DirectMemorySaver>,
    pub recaller: Arc<MemoryRecaller>,
    pub finder: Arc<MemoryFinder>,
    pub updater: Arc<MemoryUpdater>,
    pub forgetter: Arc<MemoryForgetter>,
    pub exporter: Arc<MemoryExporter>,
    pub profile_assembler: Arc<ProfileAssembler>,
    /// Exposed for the audit endpoint, which reads the trail directly
    /// rather than through a use case of its own.
    pub repository: Arc<dyn MemoryRepository>,
    /// Exposed for consolidation, which compares memories against each
    /// other rather than against a query and so needs the vectors
    /// directly — see `consolidation::application::consolidation_runner`.
    pub embedder: Arc<dyn Embedder>,
    /// Exposed for the startup text-index rebuild and `reindex --text`
    /// (reranking-plan.md Phase 1.4): the rebuilder writes the keyword
    /// index wholesale, which only the wiring layer — not the recall or
    /// save paths — should be able to ask for.
    pub text_index: Arc<dyn TextIndex>,
    /// The entity/relation graph, present only when `[graph].enabled`
    /// (implementation-plan.md Task 7.3). `None` is the default and the
    /// pre-graph behaviour: nothing writes edges, recall does not hop, and
    /// results are identical to a build without this field. The write path
    /// consumes it in Task 7.3.2 (ingest) and recall's third leg in 7.3.4.
    pub graph: Option<Arc<dyn EntityGraph>>,
    /// Config echoes the handlers need when parsing requests.
    pub extra_categories: Vec<String>,
    pub default_limit: usize,
    /// The relevance floor every agent-facing read applies unless the
    /// request overrides it. See `[retrieval].min_relevance`.
    pub min_relevance: f32,
}

impl Memories {
    pub fn build(config: &AppConfig, database: Arc<SqliteDatabase>) -> Result<Self> {
        let embedder = build_embedder(config)?;
        Self::assemble(config, database, embedder)
    }

    /// Wires the context over a caller-supplied embedder.
    ///
    /// Tests use it with the deterministic fake: loading the real ONNX
    /// model takes seconds and would make an HTTP-layer unit test depend
    /// on a 130 MB download.
    #[cfg(test)]
    pub fn for_test(
        database: Arc<SqliteDatabase>,
        embedder: Arc<dyn Embedder>,
        text_index_dir: std::path::PathBuf,
    ) -> Result<Self> {
        let config = AppConfig {
            storage: crate::bootstrap::config::StorageConfig {
                path: text_index_dir.to_string_lossy().to_string(),
                ..Default::default()
            },
            ..AppConfig::default()
        };
        Self::assemble(&config, database, embedder)
    }

    /// Wires the context over an already-built embedder, so commands
    /// that construct one themselves (`reindex`) do not load the model
    /// twice.
    pub(crate) fn assemble(
        config: &AppConfig,
        database: Arc<SqliteDatabase>,
        embedder: Arc<dyn Embedder>,
    ) -> Result<Self> {
        let concrete_repository = SqliteMemoryRepository::new(
            Arc::clone(&database),
            embedder.model_id(),
            embedder.dimensions(),
        );
        // Fail fast, before the vector table is opened at the new width:
        // a store built by a different model must send the operator to
        // `reindex` with a clear message, not surface later as a raw
        // dimension-mismatch on the first recall.
        concrete_repository.verify_pin()?;
        let repository: Arc<dyn MemoryRepository> = Arc::new(concrete_repository);
        let vectors: Arc<dyn VectorIndex> = Arc::new(SqliteVectorIndex::open(
            Arc::clone(&database),
            embedder.dimensions(),
        )?);
        let text: Arc<dyn TextIndex> = Arc::new(TantivyTextIndex::open(
            config.data_dir().join(TEXT_INDEX_DIR),
        )?);
        // Built only when enabled. Off (the default) leaves it `None`, so
        // no edge is ever written and recall is byte-identical to a build
        // without the graph — the "inert until asked for" guarantee.
        let graph: Option<Arc<dyn EntityGraph>> = if config.graph.enabled {
            Some(Arc::new(SqliteEntityGraph::new(Arc::clone(&database))))
        } else {
            None
        };
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);

        Ok(Self {
            saver: Arc::new(DirectMemorySaver::new(
                Arc::clone(&repository),
                Arc::clone(&vectors),
                Arc::clone(&text),
                Arc::clone(&embedder),
                Arc::clone(&clock),
            )),
            recaller: Arc::new(MemoryRecaller::new(
                Arc::clone(&repository),
                Arc::clone(&vectors),
                Arc::clone(&text),
                Arc::clone(&embedder),
                RecallRanker::new(config.retrieval.recency_half_life_days)
                    .with_graph_ranking(config.graph.rank_weight, config.graph.unanchored_floor)
                    .with_relevance_calibration(RelevanceCalibration::new(
                        resolve_similarity_baseline(config, embedder.model_id()),
                    )),
                Arc::clone(&clock),
                graph.clone(),
                config.graph.max_hops,
                config.graph.hop_limit,
            )),
            finder: Arc::new(MemoryFinder::new(Arc::clone(&repository))),
            updater: Arc::new(MemoryUpdater::new(
                Arc::clone(&repository),
                Arc::clone(&vectors),
                Arc::clone(&text),
                Arc::clone(&embedder),
                Arc::clone(&clock),
            )),
            forgetter: Arc::new(MemoryForgetter::new(
                Arc::clone(&repository),
                Arc::clone(&vectors),
                Arc::clone(&text),
            )),
            exporter: Arc::new(MemoryExporter::new(Arc::clone(&repository))),
            profile_assembler: Arc::new(ProfileAssembler::new(
                Arc::clone(&repository),
                Arc::clone(&clock),
            )),
            repository,
            embedder,
            text_index: text,
            graph,
            extra_categories: config.understanding.taxonomy.extra_categories.clone(),
            default_limit: config.retrieval.default_limit as usize,
            min_relevance: config.retrieval.min_relevance,
        })
    }
}

/// Resolve the vector-leg relevance baseline for one instance (task 2.2).
///
/// Precedence: an explicit `[retrieval].similarity_baseline` always wins;
/// otherwise the per-model measurement from [`default_similarity_baseline_for`]
/// applies; otherwise the fallback [`DEFAULT_SIMILARITY_BASELINE`] with a
/// `warn!`, because every relevance threshold in the system reads this
/// number and an uncalibrated model silently miscalibrates all of them.
///
/// Split out of [`Memories::assemble`] so the CLI calibration report can
/// print the baseline it used with the exact precedence recall will use —
/// two implementations would drift, and the whole point of task 2.2 is
/// that the number a threshold sits inside is the number actually in force.
pub(crate) fn resolve_similarity_baseline(config: &AppConfig, model_id: &str) -> f32 {
    if let Some(baseline) = config.retrieval.similarity_baseline {
        return baseline;
    }
    default_similarity_baseline_for(model_id).unwrap_or_else(|| {
        tracing::warn!(
            model = model_id,
            "similarity_baseline not calibrated for model {model_id}; \
             recall relevance may be miscalibrated. Measure it with \
             `recuerdos-ai calibrate` and set [retrieval].similarity_baseline."
        );
        DEFAULT_SIMILARITY_BASELINE
    })
}

pub(crate) fn build_embedder(config: &AppConfig) -> Result<Arc<dyn Embedder>> {
    let embeddings = &config.embeddings;
    let configured_base = embeddings.base_url.trim();
    let base_url = |default: &str| {
        if configured_base.is_empty() {
            default.to_string()
        } else {
            configured_base.to_string()
        }
    };

    // The key is optional for a remote provider: a hosted endpoint needs
    // one, a local server (Ollama, a self-hosted vLLM) does not. An empty
    // `api_key_env` means "send no auth"; a named-but-unset variable is a
    // misconfiguration and stops startup.
    let api_key = |section: &str| -> Result<Option<String>> {
        if embeddings.api_key_env.trim().is_empty() {
            Ok(None)
        } else {
            transport::key_from_env(&embeddings.api_key_env, section).map(Some)
        }
    };

    match embeddings.provider.as_str() {
        "local" => Ok(Arc::new(FastembedEmbedder::load(
            &embeddings.model,
            config.model_cache_dir(),
        )?)),

        // Native Gemini: its own client, so it can pass the taskType that
        // the OpenAI-compat endpoint hides — RETRIEVAL_DOCUMENT on store,
        // RETRIEVAL_QUERY on search — for better retrieval.
        "gemini" => {
            let key = api_key("embeddings")?.ok_or_else(|| {
                RaError::Validation(
                    "[embeddings].provider = \"gemini\" needs an API key; set \
                     [embeddings].api_key_env to the env var holding a Google AI \
                     Studio key"
                        .to_string(),
                )
            })?;
            Ok(Arc::new(GeminiEmbedder::load(
                &embeddings.model,
                &base_url(gemini_embedder::DEFAULT_BASE_URL),
                key,
            )?))
        }

        "openai-compat" => Ok(Arc::new(RemoteEmbedder::load(
            EmbeddingApi::OpenAiCompat,
            &embeddings.model,
            &base_url(OPENAI_DEFAULT_BASE_URL),
            api_key("embeddings")?,
            embeddings.max_batch_size,
        )?)),

        // Ollama is unauthenticated by design; any configured key is
        // ignored rather than sent.
        "ollama" => Ok(Arc::new(RemoteEmbedder::load(
            EmbeddingApi::Ollama,
            &embeddings.model,
            &base_url(OLLAMA_DEFAULT_BASE_URL),
            None,
            embeddings.max_batch_size,
        )?)),

        // Unreachable via `AppConfig::load`, which validates the provider
        // name against the same list; kept as an error rather than a
        // panic so a provider added to config but not here fails to start
        // instead of at runtime.
        other => Err(RaError::Validation(format!(
            "[embeddings].provider {other:?} has no implementation"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::config::RetrievalConfig;

    fn config_with(baseline: Option<f32>) -> AppConfig {
        AppConfig {
            retrieval: RetrievalConfig {
                similarity_baseline: baseline,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Precedence, spelled out across all four cells: the operator's
    /// explicit number wins over the map, and the map wins over the
    /// fallback, for every model combination. (The `warn!` on the
    /// fallback path fires through `tracing`; its *value* is what the
    /// relevance thresholds depend on, so that is what is pinned.)
    #[test]
    fn an_explicit_baseline_wins_and_unset_defers_to_the_model_map() {
        assert_eq!(
            resolve_similarity_baseline(&config_with(Some(0.7)), "bge-small-en-v1.5"),
            0.7
        );
        assert_eq!(
            resolve_similarity_baseline(&config_with(Some(0.7)), "gemini-embedding-2"),
            0.7
        );
        assert_eq!(
            resolve_similarity_baseline(&config_with(None), "bge-small-en-v1.5"),
            0.5
        );
        assert_eq!(
            resolve_similarity_baseline(&config_with(None), "gemini-embedding-2"),
            0.46
        );
    }

    /// The unknown-model row of the same table: the fallback keeps every
    /// pre-map deployment byte-identical, which is what "no change, no
    /// regression" required of Phase 2.
    #[test]
    fn an_uncalibrated_model_falls_back_to_the_historical_constant() {
        assert_eq!(
            resolve_similarity_baseline(&config_with(None), "text-embedding-004"),
            DEFAULT_SIMILARITY_BASELINE
        );
    }
}
