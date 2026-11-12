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

    /// The caller edited on the strength of a version that is no longer the
    /// current one (`If-Match` named an older `updated_at`). Distinct from
    /// `Conflict` because the remedy differs: re-read and decide, rather than
    /// retry — and the ingest pipeline retries a `Conflict`.
    #[error("precondition failed: {0}")]
    PreconditionFailed(String),

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

    /// A save or edit refused by one of the memory rules
    /// (ai-specs/memory-structure-plan.md §4), with a stable machine code —
    /// `kind_target_mismatch`, `needs_confirmation`, `secret`, … — and a
    /// message that says why and how to fix it. Distinct from `Validation`
    /// because a caller (an agent, mostly) acts on the code: ask the user,
    /// pick another kind, name the project.
    ///
    /// `details` carries what a caller needs to act without parsing the
    /// message: for a `conflict`, the stored memories it contradicts.
    #[error("rejected ({code}): {message}")]
    Rejected {
        code: String,
        message: String,
        details: Option<serde_json::Value>,
    },

    #[error("internal error: {0}")]
    Internal(String),
}

impl RaError {
    /// A rule's refusal with no details beyond its code and message.
    pub fn rejected(code: &str, message: impl Into<String>) -> Self {
        RaError::Rejected {
            code: code.to_string(),
            message: message.into(),
            details: None,
        }
    }
}

#[allow(dead_code)]
pub type Result<T> = std::result::Result<T, RaError>;
