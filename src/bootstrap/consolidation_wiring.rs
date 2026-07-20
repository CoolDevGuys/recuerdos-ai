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
use crate::consolidation::application::session_distiller::SessionDistiller;
use crate::shared::error::Result;
use crate::understanding::application::candidate_extractor::CandidateExtractor;
use crate::understanding::application::memory_ingestor::MemoryIngestor;
use crate::understanding::application::memory_reconciler::MemoryReconciler;
use crate::understanding::application::verbatim_ingestor::VerbatimIngestor;
use crate::understanding::domain::ingest_pipeline::IngestPipeline;
use std::sync::Arc;

pub struct Consolidation {
    pub session_distiller: Arc<SessionDistiller>,
}

impl Consolidation {
    pub fn build(
        config: &AppConfig,
        memories: &Memories,
        understanding: &Understanding,
    ) -> Result<Self> {
        Ok(Self {
            session_distiller: Arc::new(SessionDistiller::new(
                session_pipeline(config, memories, understanding),
                understanding.enabled,
            )),
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
