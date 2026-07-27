//! HTTP plumbing shared by the three chat clients.
//!
//! The interesting part is [`classify`]: which HTTP failures are worth
//! retrying. Getting that wrong in either direction is expensive — retry
//! a 401 and every ingestion job burns its full attempt budget before
//! reporting a problem the operator could have fixed in seconds; *don't*
//! retry a 429 and a brief rate limit dead-letters a batch of memories.

use crate::understanding::domain::chat_model::ChatError;
use std::time::Duration;

/// Long enough for a slow structured generation, short enough that a
/// wedged connection does not hold an ingestion worker forever.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

pub fn http_client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().timeout(timeout).build()
}

/// Maps a transport-level failure.
///
/// Timeouts and connection errors are transient: the far side being slow
/// or a socket being reset says nothing about whether the request was
/// valid.
pub fn classify_transport(error: reqwest::Error) -> ChatError {
    if error.is_timeout() {
        return ChatError::Transient(format!("request timed out: {error}"));
    }
    if error.is_connect() || error.is_request() {
        return ChatError::Transient(format!("could not reach the provider: {error}"));
    }
    ChatError::Permanent(error.to_string())
}

/// Maps a non-2xx response.
///
/// 408/409/429 and every 5xx are transient — the two categories providers
/// use for "we are busy, come back". Everything else is a request the
/// provider will reject identically next time.
pub fn classify_status(status: reqwest::StatusCode, body: &str) -> ChatError {
    let detail = format!("HTTP {}: {}", status.as_u16(), summarise(body));

    if status.is_server_error()
        || matches!(
            status.as_u16(),
            408 /* timeout */ | 409 /* conflict */ | 429 /* rate limited */
        )
    {
        ChatError::Transient(detail)
    } else {
        ChatError::Permanent(detail)
    }
}

/// Provider error bodies can be long. The first line carries the message;
/// the rest is usually a request id and a stack of nested envelopes.
fn summarise(body: &str) -> String {
    const LIMIT: usize = 400;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "(empty body)".to_string();
    }
    if trimmed.chars().count() <= LIMIT {
        return trimmed.to_string();
    }
    trimmed.chars().take(LIMIT).collect::<String>() + "…"
}

/// Reads an API key from the configured environment variable.
///
/// Keys are never stored in config files — an operator who pastes one
/// into `recordagent.toml` will commit it eventually. Config names the
/// variable; the process environment holds the secret. `section` is the
/// config table to name in the error (`understanding`, `embeddings`), so
/// the message points at the setting the operator actually wrote.
pub fn key_from_env(variable: &str, section: &str) -> crate::shared::error::Result<String> {
    let value = std::env::var(variable).map_err(|_| {
        crate::shared::error::RaError::Validation(format!(
            "[{section}].api_key_env names {variable:?}, but that environment \
             variable is not set. Export it before starting the daemon."
        ))
    })?;

    if value.trim().is_empty() {
        return Err(crate::shared::error::RaError::Validation(format!(
            "the environment variable {variable:?} is set but empty"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn busy_signals_are_retryable() {
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
            StatusCode::REQUEST_TIMEOUT,
        ] {
            assert!(
                matches!(classify_status(status, "busy"), ChatError::Transient(_)),
                "{status} should be retryable"
            );
        }
    }

    #[test]
    fn caller_mistakes_are_not_retryable() {
        // Retrying these would turn a five-second fix into an ingestion
        // backlog that fails slowly and silently.
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::PAYLOAD_TOO_LARGE,
        ] {
            assert!(
                matches!(classify_status(status, "nope"), ChatError::Permanent(_)),
                "{status} should not be retried"
            );
        }
    }

    #[test]
    fn the_status_code_survives_into_the_message() {
        let error = classify_status(StatusCode::UNAUTHORIZED, "invalid x-api-key");
        let message = error.to_string();
        assert!(message.contains("401"), "{message}");
        assert!(message.contains("invalid x-api-key"), "{message}");
    }

    #[test]
    fn a_long_body_is_summarised_rather_than_logged_whole() {
        let body = "x".repeat(10_000);
        let message = classify_status(StatusCode::BAD_REQUEST, &body).to_string();
        assert!(message.chars().count() < 500, "got {} chars", message.len());
        assert!(message.ends_with('…'));
    }

    #[test]
    fn an_empty_body_still_says_something() {
        let message = classify_status(StatusCode::BAD_GATEWAY, "   ").to_string();
        assert!(message.contains("(empty body)"), "{message}");
    }
}
