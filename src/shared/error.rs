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

    #[error("internal error: {0}")]
    Internal(String),
}

#[allow(dead_code)]
pub type Result<T> = std::result::Result<T, RaError>;
