//! Running synchronous work off the async runtime's worker threads.
//!
//! Everything below the use cases blocks — SQLite, tantivy, ONNX — and
//! the HTTP server, the MCP server and the ingest workers all share one
//! tokio runtime. Calling a blocking function directly from an async
//! context parks a runtime thread; enough of those at once and the server
//! stops answering, which looks like a hang rather than an error.
//!
//! Lives in `shared` rather than in one context's HTTP module because
//! three contexts need it and the boundary rules — rightly — forbid
//! reaching into another context's infrastructure to borrow a utility.

use crate::shared::error::{RaError, Result};

pub async fn blocking<T, F>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        // A panic in the closure arrives here. Reporting it rather than
        // resuming the unwind keeps one bad request from taking down the
        // runtime thread it happened to land on.
        .map_err(|e| RaError::Internal(format!("a blocking task failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_the_closures_value() {
        assert_eq!(blocking(|| Ok(41 + 1)).await.unwrap(), 42);
    }

    #[tokio::test]
    async fn propagates_the_closures_error() {
        let error = blocking(|| Err::<(), _>(RaError::NotFound("nope".into())))
            .await
            .unwrap_err();
        assert!(matches!(error, RaError::NotFound(_)), "got {error:?}");
    }

    #[tokio::test]
    async fn a_panic_becomes_an_error_rather_than_killing_the_runtime() {
        let error = blocking(|| -> Result<()> { panic!("boom") })
            .await
            .unwrap_err();
        assert!(matches!(error, RaError::Internal(_)), "got {error:?}");
    }
}
