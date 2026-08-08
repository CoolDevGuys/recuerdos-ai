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
use crate::memories::domain::recall_ranker::RecallRanker;
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
    /// The entity/relation graph, present only when `[graph].enabled`
    /// (implementation-plan.md Task 7.3). `None` is the default and the
    /// pre-graph behaviour: nothing writes edges, recall does not hop, and
    /// results are identical to a build without this field. Wired but not
    /// yet consumed — the write path joins in Task 7.3.2, recall in 7.3.4.
    #[allow(dead_code)]
    pub graph: Option<Arc<dyn EntityGraph>>,
    /// Config echoes the handlers need when parsing requests.
    pub extra_categories: Vec<String>,
    pub default_limit: usize,
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

    fn assemble(
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
                RecallRanker::new(config.retrieval.recency_half_life_days),
                Arc::clone(&clock),
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
            graph,
            extra_categories: config.understanding.taxonomy.extra_categories.clone(),
            default_limit: config.retrieval.default_limit as usize,
        })
    }
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
        )?)),

        // Ollama is unauthenticated by design; any configured key is
        // ignored rather than sent.
        "ollama" => Ok(Arc::new(RemoteEmbedder::load(
            EmbeddingApi::Ollama,
            &embeddings.model,
            &base_url(OLLAMA_DEFAULT_BASE_URL),
            None,
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
