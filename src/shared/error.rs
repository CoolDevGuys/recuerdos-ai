use std::time::Duration;
use thiserror::Error;

/// The crate-wide domain error type. Contexts may wrap this or return it
/// directly from use cases; infrastructure adapters translate it to their
/// transport's error shape (HTTP status + error envelope, MCP error, ...).
// Consumed starting with the identity/memories use cases in Phase 1+; not
// yet constructed by any Phase 0 code.
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum RaError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("validation failed: {0}")]
    Validation(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("conflict: {0}")]
    Conflict(String),

    /// Every rerank session was busy for as long as the caller was willing
    /// to wait (`Reranker::score_before`), so the stage was given up on
    /// *before* it started rather than part-way through its inference.
    ///
    /// A distinct variant, rather than an `Internal` whose text happens to
    /// say "busy", because the application layer has to tell the two apart
    /// to report them differently — and matching on an error message is the
    /// thing this type exists to avoid. Like `Internal` it never reaches a
    /// client: a recall does not fail because the second stage was busy
    /// (invariant I-2), so the request is served on the pre-rerank order and
    /// this travels as `rerank: {state: "skipped", reason: "deadline"}`.
    #[error("rerank busy: no session free within {} ms", .0.as_millis())]
    RerankBusy(Duration),

    #[error("internal error: {0}")]
    Internal(String),
}

#[allow(dead_code)]
pub type Result<T> = std::result::Result<T, RaError>;
