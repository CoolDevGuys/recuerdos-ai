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

#[async_trait::async_trait]
pub trait IngestPipeline: Send + Sync {
    /// Turns raw submitted content into stored memories, returning what
    /// it produced.
    ///
    /// An empty result is success, not failure: "nothing here is worth
    /// remembering" is the correct outcome for small talk, and treating
    /// it as an error would dead-letter every greeting a user sends.
    ///
    /// Errors are classified by the caller through [`is_retryable`]: a
    /// rate-limited provider should come back, a rejected payload should
    /// not.
    async fn execute(
        &self,
        context: &UserContext,
        payload: &IngestPayload,
    ) -> Result<Vec<MemoryId>>;
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
