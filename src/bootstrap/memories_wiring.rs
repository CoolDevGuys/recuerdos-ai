//! Composition root for the memories context.

use crate::bootstrap::config::AppConfig;
use crate::identity::domain::user_context::UserContext;
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
    DEFAULT_SIMILARITY_BASELINE, RecallRanker, RelevanceCalibration, RerankStrategy,
    default_similarity_baseline_for,
};
use crate::memories::domain::reranker::{RerankMode, Reranker};
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
use crate::providers::infrastructure::rerank::fastembed_reranker::FastembedReranker;
use crate::shared::clock::{Clock, SystemClock};
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::SqliteDatabase;
use std::sync::Arc;
use std::time::Duration;

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
        // The wall clock: what the daemon, `reindex` and every other
        // production entry point get.
        Self::build_with_clock(config, database, Arc::new(SystemClock))
    }

    /// [`build`](Self::build) with the clock handed in, so a harness can own
    /// "now" instead of racing it.
    ///
    /// This is the only seam between the context and time: the saver stamps
    /// `created_at` from this clock, the ranker ages results against it, and
    /// the graph's `valid_from` is the memory's `created_at`, so one clock
    /// decides all three. `eval` uses it to seed a corpus with *ages*
    /// (recall-quality-plan.md Task 0.1) — without it every eval memory is
    /// milliseconds old, the recency band is identical for all of them, and
    /// what looks like a relevance decision is a UUID tie-break. Production
    /// callers keep [`build`](Self::build) and [`SystemClock`].
    pub(crate) fn build_with_clock(
        config: &AppConfig,
        database: Arc<SqliteDatabase>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let embedder = build_embedder(config)?;
        Self::assemble_with_clock(config, database, embedder, build_reranker(config)?, clock)
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
    /// twice. The second-stage reranker (reranking-plan.md Phase 5.3) is
    /// built here — and *only* here, and only when `[rerank].mode !=
    /// "off"` — so a load or download failure with the stage enabled
    /// aborts startup exactly like an embedding provider failure.
    pub(crate) fn assemble(
        config: &AppConfig,
        database: Arc<SqliteDatabase>,
        embedder: Arc<dyn Embedder>,
    ) -> Result<Self> {
        Self::assemble_with_reranker(config, database, embedder, build_reranker(config)?)
    }

    /// [`assemble`](Self::assemble) with the rerank stage handed in,
    /// bypassing [`build_reranker`]. Tests use it to prove the wiring
    /// *routes* a reranker (mode, depth, timeout and strategy reach the
    /// recaller) without loading a real model — the same reason
    /// [`for_test`](Self::for_test) takes a fake embedder.
    pub(crate) fn assemble_with_reranker(
        config: &AppConfig,
        database: Arc<SqliteDatabase>,
        embedder: Arc<dyn Embedder>,
        reranker: Option<WiredReranker>,
    ) -> Result<Self> {
        Self::assemble_with_clock(config, database, embedder, reranker, Arc::new(SystemClock))
    }

    /// [`assemble_with_reranker`](Self::assemble_with_reranker) with the
    /// clock handed in — see [`build_with_clock`](Self::build_with_clock).
    /// Everything else is the same wiring, so what the eval measures is what
    /// the daemon serves down to the component that tells time.
    fn assemble_with_clock(
        config: &AppConfig,
        database: Arc<SqliteDatabase>,
        embedder: Arc<dyn Embedder>,
        reranker: Option<WiredReranker>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        // Indexes, repository, graph, clock and the tuned ranker come from
        // one place, shared with the audit's read-only pair
        // (`assemble_audit_recall`): the audit compares two pipelines and
        // that comparison is worthless if only one of them is the one the
        // daemon runs.
        let RecallCore {
            repository,
            vectors,
            text,
            graph,
            // The same `Arc` that went in: `RecallCore` holds the clock so the
            // audit can read it, and every writer below stamps from it.
            clock,
            ranker,
        } = RecallCore::open(config, &database, &embedder, false, clock)?;

        // The rerank knobs travel the whole way here — the reranker itself
        // plus mode/depth/timeout onto the recaller, the strategy and weight
        // already carried by `ranker` — because a placeholder value lost
        // between config and a struct default is exactly the drift the
        // Phase 2 baseline map exists to prevent. `with_rerank_strategy`
        // is inert while the recaller has no reranker to score with
        // (`apply_rerank` over an empty list is a hard no-op), so setting
        // it unconditionally keeps the `reranker: None` identity path
        // byte-identical while making the wiring one shape.
        let mut recaller = MemoryRecaller::new(
            Arc::clone(&repository),
            Arc::clone(&vectors),
            Arc::clone(&text),
            Arc::clone(&embedder),
            ranker,
            Arc::clone(&clock),
            graph.clone(),
            config.graph.max_hops,
            config.graph.hop_limit,
        );
        // "no reranker, no change", spelled as code: `None` skips
        // `with_reranker` entirely, so the recaller keeps its pre-Phase-4
        // fields at their defaults.
        if let Some(reranker) = reranker {
            recaller = recaller.with_reranker(
                reranker.reranker,
                reranker.mode,
                config.rerank.depth,
                Duration::from_millis(config.rerank.timeout_ms),
            );
        }

        Ok(Self {
            saver: Arc::new(DirectMemorySaver::new(
                Arc::clone(&repository),
                Arc::clone(&vectors),
                Arc::clone(&text),
                Arc::clone(&embedder),
                Arc::clone(&clock),
            )),
            recaller: Arc::new(recaller),
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

/// The read side of a memories instance: the indexes, the system of
/// record, the graph and the tuned ranker, with nothing that writes.
///
/// Extracted from [`Memories::assemble_with_reranker`] so that
/// [`assemble_audit_recall`] builds its pipelines from the *same* parts in
/// the *same* order. An audit whose "before" pipeline had drifted from the
/// served one would report a difference that was only the drift.
struct RecallCore {
    repository: Arc<dyn MemoryRepository>,
    vectors: Arc<dyn VectorIndex>,
    text: Arc<dyn TextIndex>,
    graph: Option<Arc<dyn EntityGraph>>,
    clock: Arc<dyn Clock>,
    /// The ranker exactly as `[retrieval]`, `[graph]` and `[rerank]`
    /// configure it, including the rerank strategy and weight.
    ranker: RecallRanker,
}

impl RecallCore {
    /// Opens the retrieval stack. `read_only_text` decides how the keyword
    /// index is opened: `false` for the daemon (it writes), `true` for
    /// [`assemble_audit_recall`], which must neither write nor take the
    /// writer lock a running daemon is holding. `clock` is the instant every
    /// `created_at` stamp and every recency multiplier is measured against —
    /// [`SystemClock`] in production, a settable one in the eval.
    fn open(
        config: &AppConfig,
        database: &Arc<SqliteDatabase>,
        embedder: &Arc<dyn Embedder>,
        read_only_text: bool,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let concrete_repository = SqliteMemoryRepository::new(
            Arc::clone(database),
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
            Arc::clone(database),
            embedder.dimensions(),
        )?);
        let text: Arc<dyn TextIndex> = if read_only_text {
            Arc::new(TantivyTextIndex::open_read_only(
                config.data_dir().join(TEXT_INDEX_DIR),
            )?)
        } else {
            Arc::new(TantivyTextIndex::open(
                config.data_dir().join(TEXT_INDEX_DIR),
            )?)
        };
        // Built only when enabled. Off (the default) leaves it `None`, so
        // no edge is ever written and recall is byte-identical to a build
        // without the graph — the "inert until asked for" guarantee.
        let graph: Option<Arc<dyn EntityGraph>> = if config.graph.enabled {
            Some(Arc::new(SqliteEntityGraph::new(Arc::clone(database))))
        } else {
            None
        };

        Ok(Self {
            repository,
            vectors,
            text,
            graph,
            clock,
            ranker: RecallRanker::new(config.retrieval.recency_half_life_days)
                .with_graph_ranking(config.graph.rank_weight, config.graph.unanchored_floor)
                .with_relevance_calibration(RelevanceCalibration::new(resolve_similarity_baseline(
                    config,
                    embedder.model_id(),
                )))
                .with_rerank_strategy(rerank_strategy(&config.rerank)),
        })
    }
}

/// The indexes, the ranker and the system of record, wired into a recall
/// pipeline with **no** second stage — the part every audit side shares.
///
/// Every column of `rerank audit` / `recall audit` is built from here, so the
/// wiring rules (read-only keyword index, one similarity baseline, one clock,
/// the keyword leg must be populated) cannot drift between two sides of a
/// comparison — an audit whose "before" pipeline had drifted from the served
/// one would report a difference that was only the drift.
///
/// Deliberately not a `Memories`: that context carries the saver, updater
/// and forgetter, and a command whose entire promise is that it cannot change
/// a store should not be handed the objects that would.
fn assemble_recall_base(
    config: &AppConfig,
    database: &Arc<SqliteDatabase>,
    embedder: &Arc<dyn Embedder>,
    context: &UserContext,
) -> Result<MemoryRecaller> {
    let core = RecallCore::open(config, database, embedder, true, Arc::new(SystemClock))?;
    // Refuse a store whose keyword leg is dead rather than audit it. The
    // recaller treats a failing keyword search as degradation — right for a
    // request, wrong for a measurement: the "before" column would be a
    // vector-only ordering while reading like the hybrid one the daemon
    // serves, and every rank delta in the report would be an artefact.
    if !core.text.is_populated(context) {
        return Err(RaError::Validation(format!(
            "user {:?} has no keyword index to read. Stop the daemon and run \
             `recuerdos-ai reindex --text`, then audit again.",
            context.handle()
        )));
    }
    Ok(MemoryRecaller::new(
        Arc::clone(&core.repository),
        Arc::clone(&core.vectors),
        Arc::clone(&core.text),
        Arc::clone(embedder),
        core.ranker,
        Arc::clone(&core.clock),
        core.graph,
        config.graph.max_hops,
        config.graph.hop_limit,
    ))
}

/// One side of a comparison, built from that side's own config over a store
/// opened for reading (recall-quality-plan.md Task 0.5).
///
/// The `rerank audit` alias calls this twice with the *same* config, forcing
/// the mode `Off` then `On`; `recall audit --compare-config` calls it once per
/// config, so the whole config is what differs — graph weights, recency,
/// hybrid on/off, the ranker's strategy. The guarantees are unchanged from the
/// rerank audit: the keyword index is read-only, nothing on this path writes,
/// and the daemon may stay running.
///
/// `reranker` is the model to attach and `mode` this side's effective mode:
/// `[rerank].mode` as configured for a `--compare-config` side, or the forced
/// `On` / `Off` of the `rerank audit` alias. Asking for a stage with no model
/// is a wiring bug and says so, rather than quietly serving one side a
/// different pipeline than it claimed to run.
pub(crate) fn assemble_recall_side(
    config: &AppConfig,
    database: &Arc<SqliteDatabase>,
    embedder: Arc<dyn Embedder>,
    reranker: Option<Arc<dyn Reranker>>,
    mode: RerankMode,
    context: &UserContext,
    depth: usize,
) -> Result<MemoryRecaller> {
    if mode != RerankMode::Off && reranker.is_none() {
        return Err(RaError::Internal(format!(
            "an audit side asked for rerank mode {mode:?} with no model attached"
        )));
    }
    let base = assemble_recall_base(config, database, &embedder, context)?;
    // No deadline, exactly as the rerank audit: an offline tool measuring what
    // a stage costs must not be cut off in the middle of it.
    Ok(base.with_second_stage(reranker, mode, depth, Duration::ZERO))
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

/// A loaded reranker plus what the config says to do with it — the unit
/// the composition root hands the recaller, so mode can never disagree
/// with the presence of the reranker it belongs to.
pub(crate) struct WiredReranker {
    pub reranker: Arc<dyn Reranker>,
    pub mode: RerankMode,
}

/// `[rerank].mode` as the domain type. `AppConfig::validate` refuses the
/// other spellings; the error arm exists so a hand-built config (a test,
/// a tool) fails to wire rather than silently defaulting a mode nobody
/// asked for.
pub(crate) fn parse_rerank_mode(mode: &str) -> Result<RerankMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "off" => Ok(RerankMode::Off),
        "shadow" => Ok(RerankMode::Shadow),
        "on" => Ok(RerankMode::On),
        other => Err(RaError::Validation(format!(
            "[rerank].mode {other:?} is not one of off, shadow, on"
        ))),
    }
}

/// `[rerank].strategy` (and its weight) as the ranker's strategy. Same
/// validation argument as [`parse_rerank_mode`].
pub(crate) fn rerank_strategy(rerank: &crate::bootstrap::config::RerankConfig) -> RerankStrategy {
    match rerank.strategy.trim().to_ascii_lowercase().as_str() {
        "replace" => RerankStrategy::Replace,
        // "rrf" and anything past validation: the leg form, which is
        // also `RerankStrategy::default()`'s shape.
        _ => RerankStrategy::Rrf {
            weight: rerank.weight,
        },
    }
}

/// Build the second stage for one instance (reranking-plan.md Phase 5.3).
///
/// `mode = off` returns `None` *before* resolving or loading anything —
/// that ordering is the guarantee that an off deployment never touches
/// the network, loads a model, or pays startup cost, and a test pins
/// that it stays None even when the model name is nonsense and the cache
/// directory does not exist. Any other mode loads for real, and a load
/// failure is a startup failure: "you enabled this and it is broken" is
/// decided at boot, never as a silent permanent fallback (invariant I-2's
/// split between runtime degradation and startup honesty).
pub(crate) fn build_reranker(config: &AppConfig) -> Result<Option<WiredReranker>> {
    let mode = parse_rerank_mode(&config.rerank.mode)?;
    if mode == RerankMode::Off {
        return Ok(None);
    }
    let reranker = load_reranker(config, config.rerank.max_concurrency)?;
    tracing::info!(
        model = reranker.model_id(),
        ?mode,
        depth = config.rerank.depth,
        max_concurrency = config.rerank.max_concurrency,
        "rerank stage loaded"
    );
    Ok(Some(WiredReranker {
        reranker: Arc::new(reranker),
        mode,
    }))
}

/// Loads `[rerank].model` for a tool that is not the daemon
/// (`rerank audit`, reranking-plan.md Phase 6.2).
///
/// Unlike [`build_reranker`] this ignores `[rerank].mode` entirely: the
/// audit exists to *decide* whether the stage is worth enabling, so it has
/// to work on a deployment where the honest config answer is still `off`.
/// What it keeps is everything that decides the ordering — model,
/// strategy, weight, depth — so the "after" column is what enabling the
/// stage would actually serve.
///
/// One inference at a time: nothing else is waiting on this process, and
/// every unit of concurrency is another copy of the model in RAM on the
/// small box this command is meant for.
pub(crate) fn load_reranker_for_audit(config: &AppConfig) -> Result<FastembedReranker> {
    load_reranker(config, 1)
}

/// The one place the rerank model is resolved and loaded, so the daemon and
/// the audit cannot end up judging a different model from the one the
/// deployment would run. A load failure (missing file, no network to
/// download it) is an error either way, and names the model.
fn load_reranker(config: &AppConfig, max_concurrency: usize) -> Result<FastembedReranker> {
    // Logged *before* the load, not only after it. Loading is where a first
    // `mode = "shadow"`/`"on"` boot can spend minutes — the weights are over a
    // gigabyte and come over the network — and an operator watching a log that
    // stops at the previous line cannot tell a slow download from a hung
    // process. Naming the model and the directory says where to look
    // (`du -sh <cache_dir>` grows as it downloads) without reaching for a
    // debugger. A model already in the cache, or a bring-your-own path, pays
    // nothing and this line costs nothing either.
    tracing::info!(
        model = %config.rerank.model,
        cache_dir = %config.model_cache_dir().display(),
        "loading the rerank model; if its weights are not in this cache yet, \
         the first load downloads them (~1.1 GB for the default model) and can \
         take minutes"
    );
    FastembedReranker::load(
        &config.rerank.model,
        config.model_cache_dir(),
        max_concurrency,
        config.rerank.batch_size,
    )
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

    // --- the rerank wiring (reranking-plan.md Phase 5) ------------------

    use crate::bootstrap::config::RerankConfig;
    use crate::identity::domain::user_context::UserContext;
    use crate::memories::application::test_doubles::FakeReranker;
    use crate::memories::domain::recall_query::RecallQuery;

    fn config_with_rerank(rerank: RerankConfig) -> AppConfig {
        AppConfig {
            rerank,
            ..AppConfig::default()
        }
    }

    #[test]
    fn an_off_mode_builds_no_reranker_and_never_reaches_the_model() {
        // The DoD's "assert with a spy/counter", spelled as the strongest
        // version of itself: with `mode = off`, `build_reranker` returns
        // `None` even when the model name is nonsense and the cache
        // directory does not exist — proving it returns *before*
        // resolving or loading anything. No network, no load, no startup
        // cost, structurally.
        let config = config_with_rerank(RerankConfig {
            mode: "off".to_string(),
            model: "definitely-not-a-model".to_string(),
            ..RerankConfig::default()
        });
        // (`WiredReranker` holds a `dyn`, so the `Option` inside is not
        // `Debug` — `is_ok_and` asserts without needing it.)
        assert!(build_reranker(&config).is_ok_and(|built| built.is_none()));

        // Env-override spelling of the same default is what the plan's
        // `RECUERDOS_AI_*` convention offers; the gate is on the parsed
        // mode, so both spellings are inert.
        let config = config_with_rerank(RerankConfig {
            ..RerankConfig::default()
        });
        assert!(build_reranker(&config).is_ok_and(|built| built.is_none()));
    }

    #[test]
    fn an_enabled_mode_reaches_the_load_and_a_broken_load_is_an_error() {
        // Both non-off modes must attempt construction: an unknown model
        // name fails with the adapter's validation error (not the off
        // path's silent `None`), which is what aborts startup through
        // `assemble`. A user-defined `.onnx` path with no file beside it
        // proves the same from the load side without needing the network.
        // (`WiredReranker` cannot implement `Debug` — the reranker is a
        // `dyn` — so errors are extracted by hand rather than `expect_err`.)
        for mode in ["on", "shadow"] {
            let error = match build_reranker(&config_with_rerank(RerankConfig {
                mode: mode.to_string(),
                model: "definitely-not-a-model".to_string(),
                ..RerankConfig::default()
            })) {
                Err(error) => error,
                Ok(_) => panic!("an enabled rerank with an unknown model must not wire"),
            };
            assert!(matches!(error, RaError::Validation(_)), "got {error:?}");
            assert!(
                error.to_string().contains("definitely-not-a-model"),
                "{error}"
            );
        }

        let missing = std::env::temp_dir().join("ra-wiring-test-absent/model.onnx");
        let error = match build_reranker(&config_with_rerank(RerankConfig {
            mode: "on".to_string(),
            model: missing.display().to_string(),
            ..RerankConfig::default()
        })) {
            Err(error) => error,
            Ok(_) => panic!("a user-defined model whose files are absent must not wire"),
        };
        assert!(error.to_string().contains("tokenizer"), "{error}");
    }

    #[test]
    fn the_strategy_and_weight_map_onto_the_ranker() {
        let rrf = rerank_strategy(&RerankConfig {
            strategy: "rrf".to_string(),
            weight: 2.5,
            ..RerankConfig::default()
        });
        assert!(
            matches!(rrf, RerankStrategy::Rrf { weight } if (weight - 2.5).abs() < f32::EPSILON)
        );
        assert!(matches!(
            rerank_strategy(&RerankConfig {
                strategy: "replace".to_string(),
                ..RerankConfig::default()
            }),
            RerankStrategy::Replace
        ));
    }

    /// The phase's wiring-through test: a reranker handed to
    /// `assemble_with_reranker` must reach the *recaller the config
    /// describes* — mode, depth and strategy all honoured. A fake
    /// reversal under `Replace` makes "the rerank order was served" an
    /// exact assertion (Phase 4's reasoning), and the depth-2 variant
    /// proves the configured head size travels too.
    #[test]
    fn the_rerank_config_reaches_recall_through_the_real_wiring() {
        // Graded repetition of the query term gives each memory a
        // *distinct* fused rank. Fixture texts that tie in both indexes
        // would let random memory ids break the tie, and the fused order
        // would then differ between the instances below.
        let contents = ["pnpm pnpm pnpm", "pnpm pnpm", "pnpm"];
        let query = RecallQuery::new("pnpm", 10).unwrap();
        // One database *and* storage dir per instance: each recall's
        // `touch_accessed` feeds the importance bookkeeping, so sharing
        // one store would let an earlier run reorder a later one's
        // fusion for reasons this test is not about.
        let instance = |depth: usize, reranker: Option<WiredReranker>| {
            let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
            let dir = tempfile::tempdir().unwrap();
            let config = AppConfig {
                storage: crate::bootstrap::config::StorageConfig {
                    path: dir.path().to_string_lossy().to_string(),
                    ..Default::default()
                },
                rerank: RerankConfig {
                    mode: "on".to_string(),
                    strategy: "replace".to_string(),
                    depth,
                    ..RerankConfig::default()
                },
                ..AppConfig::default()
            };
            let memories = Memories::assemble_with_reranker(
                &config,
                Arc::clone(&database),
                Arc::new(crate::memories::application::fake_embedder::FakeEmbedder::default()),
                reranker,
            )
            .unwrap();
            let context = authenticate_for_test(&database);
            for content in contents {
                memories
                    .saver
                    .execute(
                        &context,
                        crate::memories::application::test_doubles::new_memory(content),
                        "test",
                    )
                    .unwrap();
            }
            (
                memories
                    .recaller
                    .execute(&context, &query)
                    .unwrap()
                    .iter()
                    .map(|s| s.memory.content().to_string())
                    .collect::<Vec<_>>(),
                dir,
            )
        };
        let fake = || Arc::new(FakeReranker::reversing()) as Arc<dyn Reranker>;

        let (before, _dir) = instance(RerankConfig::default().depth, None);
        assert_eq!(before.len(), contents.len(), "fixture sanity");

        // `mode = on` + `replace` + the reversing fake ⇒ exactly the
        // reversed fused head; the default depth covers all three.
        let (served, _dir) = instance(
            RerankConfig::default().depth,
            Some(WiredReranker {
                reranker: fake(),
                mode: RerankMode::On,
            }),
        );
        let mut expected = before.clone();
        expected.reverse();
        assert_eq!(served, expected, "the wired reranker did not reach recall");

        // `depth = 2`: only the top two are reranked, so exactly they
        // swap and the third keeps its fused place behind the head.
        let (shallow, _dir) = instance(
            2,
            Some(WiredReranker {
                reranker: fake(),
                mode: RerankMode::On,
            }),
        );
        assert_eq!(
            shallow,
            vec![before[1].clone(), before[0].clone(), before[2].clone()],
            "[rerank].depth did not bound the reranked head"
        );
    }

    /// Task 0.1's seam, asserted at the wiring rather than in the eval: the
    /// clock handed to the assembly is the clock the *saver* stamps from and
    /// the *recaller* ages against. If either quietly kept `SystemClock`, the
    /// eval's aged corpus would score as if every memory were new, and the
    /// numbers would still look plausible.
    #[test]
    fn an_injected_clock_reaches_the_saver_and_the_recaller() {
        use crate::shared::clock::FixedClock;

        let instant = chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .expect("a fixed instant")
            .with_timezone(&chrono::Utc);
        let clock = FixedClock::at(instant);

        let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let config = AppConfig {
            storage: crate::bootstrap::config::StorageConfig {
                path: dir.path().to_string_lossy().to_string(),
                ..Default::default()
            },
            ..AppConfig::default()
        };
        let memories = Memories::assemble_with_clock(
            &config,
            Arc::clone(&database),
            Arc::new(crate::memories::application::fake_embedder::FakeEmbedder::default()),
            None,
            Arc::new(clock.clone()),
        )
        .unwrap();
        let context = authenticate_for_test(&database);

        let stored = memories
            .saver
            .execute(
                &context,
                crate::memories::application::test_doubles::new_memory("User prefers pnpm"),
                "test",
            )
            .unwrap();
        assert_eq!(
            stored.created_at(),
            instant,
            "the saver used the wall clock, not the one it was wired with"
        );

        // The recaller reads the same handle. Two half-lives of travel takes
        // the recency band to its floor, and only a recall that asked *this*
        // clock what time it is can notice.
        let query = RecallQuery::new("pnpm", 5).unwrap();
        let fresh = memories.recaller.execute(&context, &query).unwrap()[0].score;
        clock.set(instant + chrono::Duration::days(400));
        let aged = memories.recaller.execute(&context, &query).unwrap()[0].score;
        assert!(
            aged < fresh,
            "the recaller scored {aged} after 400 days and {fresh} before: the injected clock \
             did not reach it"
        );
    }

    /// A `UserContext` for the wiring test, minted the only legal way —
    /// through the identity context's own authenticator (the same route
    /// `test_doubles` takes).
    fn authenticate_for_test(database: &Arc<SqliteDatabase>) -> UserContext {
        let identity = crate::bootstrap::wiring::Identity::from_database(Arc::clone(database))
            .expect("identity tables");
        identity
            .user_creator
            .execute("alex", None)
            .expect("create user");
        let issued = identity
            .api_key_issuer
            .execute(
                "alex",
                vec![crate::identity::domain::scope::Scope::Write],
                "test",
            )
            .expect("issue key");
        identity
            .key_authenticator
            .execute(&issued.token.render())
            .expect("authenticate")
    }
}
