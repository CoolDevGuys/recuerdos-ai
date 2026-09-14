//! What a worker does with a claimed job.
//!
//! A one-method contract so the queue machinery — claiming, backoff,
//! dead-lettering, crash recovery — can be tested against a pipeline that
//! fails on command, without a language model anywhere near it. Those are
//! the behaviours most likely to be subtly wrong and least likely to be
//! noticed, so they deserve tests that are fast and deterministic.

use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use crate::understanding::domain::ingest_job::IngestPayload;
use serde::Serialize;

/// Why an ingest stored no memories.
///
/// Existed because "saved!" and "nothing happened" were otherwise
/// indistinguishable to a caller. An agent that cannot tell them apart
/// guesses, and it guesses the way it was hoping to: it tells the user the
/// memory was kept. The distinction is also not cosmetic at the far end — a
/// save that produced nothing because the content was small talk wants
/// different follow-up from one that produced nothing because the store
/// already knew it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestStatus {
    /// At least one memory was written.
    Stored,
    /// Nothing in the submission was durable — small talk, an acknowledgement,
    /// a step in the task rather than a fact about it.
    NothingDurable,
    /// Candidates were found, but every one of them matched something already
    /// stored, so reconciliation wrote nothing new.
    AlreadyKnown,
    /// The store changed without a new memory being written: a retraction
    /// deleted, or a candidate superseded something. Nothing to show the
    /// user, and emphatically not a no-op.
    ChangedWithoutStoring,
}

/// What a pipeline run did.
///
/// Not just the ids, for the reason above. `status` is the machine-readable
/// half of an answer the prose in `tool_text` has always had to guess at.
#[derive(Debug, Clone)]
pub struct IngestOutcome {
    pub memory_ids: Vec<MemoryId>,
    pub status: IngestStatus,
}

impl IngestOutcome {
    /// A run that wrote `memory_ids`.
    pub fn stored(memory_ids: Vec<MemoryId>) -> Self {
        Self {
            memory_ids,
            status: IngestStatus::Stored,
        }
    }

    /// A run that wrote nothing, for `reason`. A non-empty id list always
    /// reports [`IngestStatus::Stored`], whatever was passed: the ids are
    /// the evidence, and an outcome claiming otherwise is a bug upstream
    /// rather than something to propagate.
    pub fn empty(reason: IngestStatus) -> Self {
        Self {
            memory_ids: Vec::new(),
            status: reason,
        }
    }

    /// Wraps a reconciler's answer, which knows whether it changed anything
    /// even when it stored nothing.
    pub fn from_reconciled(memory_ids: Vec<MemoryId>, superseded_or_deleted: usize) -> Self {
        if !memory_ids.is_empty() {
            return Self::stored(memory_ids);
        }
        if superseded_or_deleted > 0 {
            return Self::empty(IngestStatus::ChangedWithoutStoring);
        }
        Self::empty(IngestStatus::AlreadyKnown)
    }
}

#[async_trait::async_trait]
pub trait IngestPipeline: Send + Sync {
    /// Turns raw submitted content into stored memories.
    ///
    /// Producing no memories is success, not failure: "nothing here is worth
    /// remembering" is the correct outcome for small talk, and treating it
    /// as an error would dead-letter every greeting a user sends. Say which
    /// kind of nothing it was via [`IngestOutcome::status`] — a caller
    /// reporting back to a human needs to know whether to say "saved".
    ///
    /// Errors are classified by the caller through [`is_retryable`]: a
    /// rate-limited provider should come back, a rejected payload should
    /// not.
    async fn execute(
        &self,
        context: &UserContext,
        payload: &IngestPayload,
    ) -> Result<IngestOutcome>;
}

/// Whether a failed job is worth another attempt.
///
/// `Internal` covers the whole "something outside us broke" family —
/// provider outages, rate limits, a locked database — and is the only
/// thing retrying can fix. A `Validation` failure means the content
/// itself is unacceptable, and re-running it produces the same answer
/// three times before dead-lettering anyway; failing immediately gets the
/// operator a useful error sooner.
pub fn is_retryable(error: &crate::shared::error::RaError) -> bool {
    use crate::shared::error::RaError;
    matches!(error, RaError::Internal(_) | RaError::Conflict(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::error::RaError;

    #[test]
    fn outages_are_retryable_and_bad_content_is_not() {
        assert!(is_retryable(&RaError::Internal("provider down".into())));
        assert!(is_retryable(&RaError::Conflict(
            "database is locked".into()
        )));

        assert!(!is_retryable(&RaError::Validation(
            "content is empty".into()
        )));
        assert!(!is_retryable(&RaError::NotFound("user is gone".into())));
        assert!(!is_retryable(&RaError::Forbidden("no write scope".into())));
    }
}
