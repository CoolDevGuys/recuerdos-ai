//! Retry and backoff, as a decorator.
//!
//! Kept out of the three clients on purpose. Retry policy is one decision
//! — how long to wait, how many times, on which failures — and three
//! copies of it would drift until "why did that job give up so fast?"
//! depended on which provider was configured. The clients stay dumb
//! transports; this wraps whichever one is in use.
//!
//! It composes with anything implementing [`ChatModel`], including the
//! test double, which is how the retry behaviour itself is tested without
//! a network.

use crate::understanding::domain::chat_model::{
    ChatError, ChatModel, ChatResult, StructuredRequest,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Three total attempts: one retry covers a transient blip, a second
/// covers a short rate-limit window. Beyond that the provider is having a
/// real outage, and the job queue's own retry — which waits minutes, not
/// milliseconds, and survives a restart — is the right layer to handle it.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Doubles per attempt: 500ms, then 1s.
pub const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(500);

pub struct RetryingChatModel {
    inner: Arc<dyn ChatModel>,
    max_attempts: u32,
    initial_backoff: Duration,
}

impl RetryingChatModel {
    pub fn new(inner: Arc<dyn ChatModel>) -> Self {
        Self {
            inner,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
        }
    }

    /// Tests use this to keep the suite fast — the policy under test is
    /// *how many* attempts and *which* errors, not the wall-clock wait.
    #[cfg(test)]
    pub fn with_policy(mut self, max_attempts: u32, initial_backoff: Duration) -> Self {
        self.max_attempts = max_attempts;
        self.initial_backoff = initial_backoff;
        self
    }
}

#[async_trait::async_trait]
impl ChatModel for RetryingChatModel {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    async fn complete_structured(&self, request: &StructuredRequest) -> ChatResult<Value> {
        let mut backoff = self.initial_backoff;

        for attempt in 1..=self.max_attempts {
            let error = match self.inner.complete_structured(request).await {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };

            // Permanent and Malformed both mean the next identical call
            // produces the same answer. Malformed has already had its own
            // repair attempt inside the client; retrying the whole request
            // would just buy a second one at four times the cost.
            if !matches!(error, ChatError::Transient(_)) || attempt == self.max_attempts {
                return Err(error);
            }

            tracing::warn!(
                attempt,
                max_attempts = self.max_attempts,
                backoff_ms = backoff.as_millis() as u64,
                %error,
                "language model call failed; retrying"
            );
            tokio::time::sleep(backoff).await;
            backoff *= 2;
        }

        unreachable!("the loop returns on the final attempt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// Fails a set number of times, then succeeds. Counts every call.
    struct Flaky {
        remaining_failures: Mutex<u32>,
        error: fn(String) -> ChatError,
        calls: Mutex<u32>,
    }

    impl Flaky {
        fn new(failures: u32, error: fn(String) -> ChatError) -> Arc<Self> {
            Arc::new(Self {
                remaining_failures: Mutex::new(failures),
                error,
                calls: Mutex::new(0),
            })
        }

        fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl ChatModel for Flaky {
        fn model_id(&self) -> &str {
            "flaky"
        }

        async fn complete_structured(&self, _request: &StructuredRequest) -> ChatResult<Value> {
            *self.calls.lock().unwrap() += 1;
            let mut remaining = self.remaining_failures.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                return Err((self.error)("boom".to_string()));
            }
            Ok(json!({"ok": true}))
        }
    }

    fn request() -> StructuredRequest {
        StructuredRequest::new("s", "u", "answer", json!({"type": "object"}))
    }

    fn retrying(inner: Arc<dyn ChatModel>) -> RetryingChatModel {
        RetryingChatModel::new(inner).with_policy(3, Duration::from_millis(1))
    }

    #[tokio::test]
    async fn a_transient_failure_is_retried_until_it_succeeds() {
        let flaky = Flaky::new(2, ChatError::Transient);
        let value = retrying(flaky.clone())
            .complete_structured(&request())
            .await
            .unwrap();

        assert_eq!(value, json!({"ok": true}));
        assert_eq!(flaky.calls(), 3);
    }

    #[tokio::test]
    async fn attempts_are_capped() {
        let flaky = Flaky::new(u32::MAX, ChatError::Transient);
        let error = retrying(flaky.clone())
            .complete_structured(&request())
            .await
            .unwrap_err();

        assert!(matches!(error, ChatError::Transient(_)), "got {error:?}");
        assert_eq!(
            flaky.calls(),
            3,
            "retrying forever would hold an ingestion worker through an outage"
        );
    }

    #[tokio::test]
    async fn a_permanent_failure_is_not_retried() {
        // A bad API key must fail in one call. Three attempts per job
        // across a backlog turns a config typo into a long, confusing
        // outage instead of an immediate error.
        let flaky = Flaky::new(u32::MAX, ChatError::Permanent);
        let error = retrying(flaky.clone())
            .complete_structured(&request())
            .await
            .unwrap_err();

        assert!(matches!(error, ChatError::Permanent(_)), "got {error:?}");
        assert_eq!(flaky.calls(), 1);
    }

    #[tokio::test]
    async fn malformed_output_is_not_retried_here() {
        // The client already made its one repair attempt. Retrying the
        // whole request would silently quadruple the cost of a model that
        // simply cannot follow the schema.
        let flaky = Flaky::new(u32::MAX, ChatError::Malformed);
        let error = retrying(flaky.clone())
            .complete_structured(&request())
            .await
            .unwrap_err();

        assert!(matches!(error, ChatError::Malformed(_)), "got {error:?}");
        assert_eq!(flaky.calls(), 1);
    }

    #[tokio::test]
    async fn the_wrapped_models_identity_shows_through() {
        // Audit entries record which model labelled a memory; the
        // decorator must not shadow that with its own name.
        let model = retrying(Flaky::new(0, ChatError::Transient));
        assert_eq!(model.model_id(), "flaky");
    }

    #[tokio::test]
    async fn backoff_grows_between_attempts() {
        let flaky = Flaky::new(2, ChatError::Transient);
        let model = RetryingChatModel::new(flaky.clone()).with_policy(3, Duration::from_millis(20));

        let started = std::time::Instant::now();
        model.complete_structured(&request()).await.unwrap();

        // 20ms + 40ms of waiting, minus scheduler slop.
        assert!(
            started.elapsed() >= Duration::from_millis(50),
            "expected doubling backoff, waited only {:?}",
            started.elapsed()
        );
    }
}
