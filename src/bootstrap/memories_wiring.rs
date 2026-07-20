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
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::recall_ranker::RecallRanker;
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::memories::infrastructure::sqlite_memory_repository::SqliteMemoryRepository;
use crate::memories::infrastructure::sqlite_vector_index::SqliteVectorIndex;
use crate::memories::infrastructure::tantivy_text_index::TantivyTextIndex;
use crate::providers::infrastructure::embeddings::fastembed_embedder::FastembedEmbedder;
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
        let repository: Arc<dyn MemoryRepository> = Arc::new(SqliteMemoryRepository::new(
            Arc::clone(&database),
            embedder.model_id(),
            embedder.dimensions(),
        ));
        let vectors: Arc<dyn VectorIndex> = Arc::new(SqliteVectorIndex::open(
            Arc::clone(&database),
            embedder.dimensions(),
        )?);
        let text: Arc<dyn TextIndex> = Arc::new(TantivyTextIndex::open(
            config.data_dir().join(TEXT_INDEX_DIR),
        )?);
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
            extra_categories: config.understanding.taxonomy.extra_categories.clone(),
            default_limit: config.retrieval.default_limit as usize,
        })
    }
}

fn build_embedder(config: &AppConfig) -> Result<Arc<dyn Embedder>> {
    match config.embeddings.provider.as_str() {
        "local" => Ok(Arc::new(FastembedEmbedder::load(
            &config.embeddings.model,
            config.model_cache_dir(),
        )?)),
        // Remote embedding providers are Phase 4 work; refusing loudly
        // beats starting up and failing on the first save.
        other => Err(RaError::Validation(format!(
            "[embeddings].provider {other:?} is not implemented yet — only \"local\" \
             works today (remote providers arrive in Phase 4)"
        ))),
    }
}
