//! Ollama `/api/chat`.
//!
//! The zero-egress option: understanding runs on the operator's own
//! hardware and no memory content leaves the host. That makes it the
//! provider to reach for when the corpus is personal, which is most of
//! them.
//!
//! Ollama takes a JSON Schema directly in `format` and constrains
//! generation to it, so structured output is well supported. The schema
//! still goes into the system prompt as well: small local models follow
//! the grammar but wander on *semantics* — right shape, invented fields —
//! and seeing the schema described in words measurably helps.
//!
//! No API key: Ollama is unauthenticated and expected to be bound to
//! localhost. If someone exposes it publicly, that is a deployment
//! decision this client cannot second-guess.

use super::openai_compat_chat_model::system_with_schema;
use super::structured_text::{RepairHint, TextCompletion, structured_from_text};
use super::transport::{classify_status, classify_transport, http_client};
use crate::shared::error::{RaError, Result};
use crate::understanding::domain::chat_model::{
    ChatError, ChatModel, ChatResult, StructuredRequest,
};
use serde_json::{Value, json};
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";

/// Generous compared to the hosted providers: a local model on CPU can
/// take minutes for a few hundred tokens, and timing that out would make
/// the zero-egress path look broken rather than slow.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(300);

pub struct OllamaChatModel {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaChatModel {
    pub fn new(model: impl Into<String>, base_url: &str) -> Result<Self> {
        Ok(Self {
            client: http_client(LOCAL_TIMEOUT)
                .map_err(|e| RaError::Internal(format!("could not build an HTTP client: {e}")))?,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.into(),
        })
    }

    /// Overrides the request timeout — see [`AnthropicChatModel::with_timeout`].
    ///
    /// [`AnthropicChatModel::with_timeout`]: super::anthropic_chat_model::AnthropicChatModel::with_timeout
    #[cfg(test)]
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        self.client = http_client(timeout)
            .map_err(|e| RaError::Internal(format!("could not build an HTTP client: {e}")))?;
        Ok(self)
    }
}

#[async_trait::async_trait]
impl TextCompletion for OllamaChatModel {
    async fn complete_text(
        &self,
        request: &StructuredRequest,
        repair: Option<&RepairHint>,
    ) -> ChatResult<String> {
        let mut messages = vec![
            json!({"role": "system", "content": system_with_schema(request)}),
            json!({"role": "user", "content": request.user}),
        ];
        if let Some(hint) = repair {
            messages.push(json!({"role": "assistant", "content": hint.previous_output}));
            messages.push(json!({"role": "user", "content": hint.as_message()}));
        }

        let body = json!({
            "model": self.model,
            "messages": messages,
            // A streamed reply would have to be reassembled before it
            // could be parsed, and nothing here renders tokens as they
            // arrive — an ingestion job has no reader to stream to.
            "stream": false,
            "format": request.schema,
            "options": {"num_predict": request.max_tokens},
        });

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(classify_transport)?;

        let status = response.status();
        let text = response.text().await.map_err(classify_transport)?;
        if !status.is_success() {
            return Err(classify_status(status, &text));
        }

        extract_message_content(&text)
    }
}

fn extract_message_content(body: &str) -> ChatResult<String> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|e| ChatError::Malformed(format!("response was not JSON: {e}")))?;

    // Ollama reports a missing model as HTTP 200 with an `error` field
    // rather than a 4xx. Left unhandled it would surface as "no message
    // content", sending the operator hunting for a parsing bug instead of
    // running `ollama pull`.
    if let Some(error) = parsed.get("error").and_then(Value::as_str) {
        return Err(ChatError::Permanent(format!("ollama: {error}")));
    }

    parsed
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ChatError::Malformed("the response contained no message".to_string()))
}

#[async_trait::async_trait]
impl ChatModel for OllamaChatModel {
    fn model_id(&self) -> &str {
        &self.model
    }

    async fn complete_structured(&self, request: &StructuredRequest) -> ChatResult<Value> {
        structured_from_text(self, request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_message_content() {
        let body = json!({"message": {"role": "assistant", "content": "{\"a\": 1}"}}).to_string();
        assert_eq!(extract_message_content(&body).unwrap(), "{\"a\": 1}");
    }

    #[test]
    fn a_two_hundred_with_an_error_field_is_reported_as_the_error_it_is() {
        let body = json!({"error": "model \"llama9\" not found, try pulling it first"}).to_string();
        let error = extract_message_content(&body).unwrap_err();

        assert!(matches!(error, ChatError::Permanent(_)), "got {error:?}");
        assert!(error.to_string().contains("not found"), "{error}");
    }
}
