//! The LLM contract this context consumes.
//!
//! Owned by `understanding` (the consumer), implemented in `providers` —
//! the same arrangement as `memories::domain::Embedder`, for the same
//! reason: the use cases decide what shape they need, and a provider
//! either fits it or does not get used.
//!
//! # Why only structured completion
//!
//! There is deliberately no `complete_text`. Every call this pipeline
//! makes — extract candidates, decide ADD/UPDATE/DELETE/NOOP — wants a
//! machine-readable answer, and a trait that also offered free prose
//! would invite someone to parse it with a regex. Asking for a schema is
//! the only thing on offer, so the fragile path does not exist.

use crate::shared::error::RaError;
use serde_json::Value;

/// What went wrong, classified by whether trying again could help.
///
/// This distinction is the whole reason the trait has its own error type
/// rather than returning [`RaError`]: `RetryingChatModel` needs to tell a
/// rate limit (wait and retry) from a bad API key (retrying just burns
/// the ingestion budget and delays the failure the operator needs to see).
#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    /// 429, 5xx, timeout, connection reset — retry with backoff.
    #[error("{0}")]
    Transient(String),

    /// 4xx, bad credentials, unknown model — retrying changes nothing.
    #[error("{0}")]
    Permanent(String),

    /// The call succeeded but the output was not usable JSON, and the one
    /// repair attempt also failed. Distinct from `Permanent` because it
    /// says something about the *model*, not the configuration.
    #[error("model returned unusable output: {0}")]
    Malformed(String),
}

impl From<ChatError> for RaError {
    fn from(error: ChatError) -> Self {
        match error {
            // All three are internal to the operator: an ingestion job
            // failing because Anthropic is rate-limiting us is not the
            // API caller's fault and must not surface as their 4xx.
            ChatError::Transient(message) => {
                RaError::Internal(format!("language model unavailable: {message}"))
            }
            ChatError::Permanent(message) => {
                RaError::Internal(format!("language model rejected the request: {message}"))
            }
            ChatError::Malformed(message) => RaError::Internal(message),
        }
    }
}

pub type ChatResult<T> = std::result::Result<T, ChatError>;

/// One structured-output request.
///
/// A struct rather than positional arguments so that adding, say, a
/// temperature knob later does not touch three implementations' call
/// signatures.
#[derive(Debug, Clone)]
pub struct StructuredRequest {
    /// Instructions that describe the job — the taxonomy, the rules.
    pub system: String,
    /// The material to work on.
    pub user: String,
    /// JSON Schema the answer must satisfy. Providers enforce this
    /// natively where they can (Anthropic tool input schema, OpenAI
    /// `json_schema` response format, Ollama `format`); where they
    /// cannot, it still reaches the model as part of the prompt.
    pub schema: Value,
    /// Names the schema for providers that require one.
    pub schema_name: String,
    pub max_tokens: u32,
}

impl StructuredRequest {
    pub fn new(
        system: impl Into<String>,
        user: impl Into<String>,
        schema_name: impl Into<String>,
        schema: Value,
    ) -> Self {
        Self {
            system: system.into(),
            user: user.into(),
            schema,
            schema_name: schema_name.into(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

/// Enough for a few dozen candidates or decisions. Extraction output is
/// bounded by the input it summarises, so this is a guard against a
/// runaway generation rather than a budget anyone should tune.
pub const DEFAULT_MAX_TOKENS: u32 = 4_096;

#[async_trait::async_trait]
pub trait ChatModel: Send + Sync {
    /// Identifies the model, for audit entries and job records. Two
    /// deployments' memories are not comparable if they were labelled by
    /// different models, so the answer is worth recording.
    fn model_id(&self) -> &str;

    /// Sends the request and returns JSON conforming to `request.schema`.
    ///
    /// Implementations are responsible for producing *parseable* JSON;
    /// validating it against the domain's expectations is the caller's
    /// job, because only the caller knows what a sensible candidate looks
    /// like.
    async fn complete_structured(&self, request: &StructuredRequest) -> ChatResult<Value>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chat_error_becomes_an_internal_error() {
        // An LLM problem is never the API caller's fault. If any of these
        // mapped to Validation or Unauthorized, an ingestion request would
        // report a 4xx and the caller would "fix" a request that was fine.
        for error in [
            ChatError::Transient("429".to_string()),
            ChatError::Permanent("bad key".to_string()),
            ChatError::Malformed("not json".to_string()),
        ] {
            assert!(
                matches!(RaError::from(error), RaError::Internal(_)),
                "chat failures must surface as internal errors"
            );
        }
    }
}
