//! Composition root for the consolidation context.
//!
//! Everything here is built on top of the other two contexts rather than
//! beside them: distillation reuses the understanding pipeline, and the
//! merge path reuses the memories use cases. That is deliberate — a
//! second way to write a memory would be a second place for the audit
//! trail, the indexes and the user scoping to drift out of agreement.

use crate::bootstrap::config::AppConfig;
use crate::bootstrap::memories_wiring::Memories;
use crate::bootstrap::understanding_wiring::Understanding;
use crate::bootstrap::wiring::Identity;
use crate::consolidation::application::consolidation_runner::ConsolidationRunner;
use crate::consolidation::application::memory_maintainer::MemoryMaintainer;
use crate::consolidation::application::memory_merger::MemoryMerger;
use crate::consolidation::application::profile_digest_writer::ProfileDigestWriter;
use crate::consolidation::application::session_distiller::SessionDistiller;
use crate::consolidation::domain::consolidation_state::ConsolidationStateStore;
use crate::consolidation::domain::profile_digest::ProfileDigestStore;
use crate::consolidation::infrastructure::sqlite_consolidation_state_store::SqliteConsolidationStateStore;
use crate::consolidation::infrastructure::sqlite_profile_digest_store::SqliteProfileDigestStore;
use crate::shared::error::Result;
use crate::understanding::application::candidate_extractor::CandidateExtractor;
use crate::understanding::application::memory_ingestor::MemoryIngestor;
use crate::understanding::application::memory_reconciler::MemoryReconciler;
use crate::understanding::application::verbatim_ingestor::VerbatimIngestor;
use crate::understanding::domain::ingest_pipeline::IngestPipeline;
use std::sync::Arc;

/// Convert a config value to `Some` if non-zero, `None` otherwise. A zero
/// in the config means "unlimited", which the runner expresses as `None`.
fn some_if_nonzero<T: Default + Copy + PartialEq>(v: T) -> Option<T> {
    if v == T::default() { None } else { Some(v) }
}

pub struct Consolidation {
    pub session_distiller: Arc<SessionDistiller>,
    /// Always built. Expiry and decay need no model, so the nightly job
    /// has work to do in every installation; only merging is conditional
    /// on a provider — see `ConsolidationRunner`.
    pub runner: Arc<ConsolidationRunner>,
    /// Serves `GET /v1/profile` and the `memory://profile` resource.
    /// Falls back to the memories context's assembler without a model.
    pub profile_digest_writer: Arc<ProfileDigestWriter>,
    pub enabled: bool,
    pub schedule: String,
}

impl Consolidation {
    pub fn build(
        config: &AppConfig,
        identity: &Identity,
        memories: &Memories,
        understanding: &Understanding,
        database: Arc<crate::shared::sqlite::SqliteDatabase>,
    ) -> Result<Self> {
        let merger = understanding.model.as_ref().map(|model| {
            Arc::new(MemoryMerger::new(
                Arc::clone(&memories.saver),
                Arc::clone(&memories.repository),
                Arc::clone(model),
                Arc::clone(&understanding.taxonomy),
            ))
        });

        if merger.is_none() && config.consolidation.enabled {
            tracing::info!(
                "[consolidation].enabled = true but no [understanding].provider is set: \
                 expired memories will still be retired and decay still recomputed, but \
                 duplicates will not be merged — that is a judgement about meaning and \
                 has no offline fallback."
            );
        }

        let runner = Arc::new(ConsolidationRunner::new(
            Arc::clone(&identity.users),
            Arc::clone(&memories.repository),
            Arc::clone(&memories.embedder),
            Arc::new(MemoryMaintainer::new(
                Arc::clone(&memories.repository),
                Arc::clone(&memories.forgetter),
            )),
            merger,
            Arc::clone(&identity.clock),
            config.consolidation.similarity_threshold as f32,
            some_if_nonzero(config.consolidation.budget.max_llm_calls),
            some_if_nonzero(config.consolidation.budget.max_duration_secs),
            some_if_nonzero(config.consolidation.budget.max_memories),
            Some(
                Arc::new(SqliteConsolidationStateStore::new(Arc::clone(&database)))
                    as Arc<dyn ConsolidationStateStore>,
            ),
        ));

        Ok(Self {
            session_distiller: Arc::new(SessionDistiller::new(
                session_pipeline(config, memories, understanding),
                understanding.enabled,
            )),
            runner,
            profile_digest_writer: Arc::new(ProfileDigestWriter::new(
                Arc::clone(&memories.repository),
                Arc::new(SqliteProfileDigestStore::new(database)) as Arc<dyn ProfileDigestStore>,
                Arc::clone(&memories.profile_assembler),
                understanding.model.clone(),
                Arc::clone(&identity.clock),
            )),
            enabled: config.consolidation.enabled,
            schedule: config.consolidation.schedule.clone(),
        })
    }
}

/// The ingestion pipeline pointed at sessions rather than submissions.
///
/// Structurally identical to the one behind `POST /v1/memories` — same
/// extraction, same reconciliation, same stores — and differing in one
/// place: the extractor is built with the session lens, which asks what
/// survives the session instead of what the text says.
///
/// Without a model this is the verbatim ingestor, purely so the type
/// checks out. `SessionDistiller` refuses to run in that mode before it
/// ever reaches the pipeline, because storing a whole transcript as one
/// memory is worse than storing nothing.
fn session_pipeline(
    config: &AppConfig,
    memories: &Memories,
    understanding: &Understanding,
) -> Arc<dyn IngestPipeline> {
    match &understanding.model {
        Some(model) => Arc::new(MemoryIngestor::new(
            Arc::new(CandidateExtractor::for_sessions(
                Arc::clone(model),
                Arc::clone(&understanding.taxonomy),
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
    }
}
