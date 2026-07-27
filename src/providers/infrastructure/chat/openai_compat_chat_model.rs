//! OpenAI-compatible `/chat/completions`.
//!
//! One client covers OpenAI, OpenRouter, Groq, Together, vLLM, LM Studio
//! and anything else that copied the endpoint — which is most of the
//! ecosystem, and the reason this is the widest-reach provider here.
//!
//! # Schema enforcement is requested, never assumed
//!
//! `response_format: {"type": "json_schema", …}` is honoured by OpenAI and
//! by some of the clones. Others ignore it silently; a few reject the
//! whole request for having an unknown field. So the schema is *also*
//! spelled out in the system prompt, and the answer goes through the
//! shared parse-and-repair path regardless. When the server does enforce
//! it, the redundancy costs a few dozen tokens. When it doesn't, that
//! redundancy is the only thing making the provider work at all.

use super::structured_text::{RepairHint, TextCompletion, structured_from_text};
use super::transport::{DEFAULT_TIMEOUT, classify_status, classify_transport, http_client};
use crate::shared::error::{RaError, Result};
use crate::understanding::domain::chat_model::{
    ChatError, ChatModel, ChatResult, StructuredRequest,
};
use serde_json::{Value, json};
#[cfg(test)]
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Google's OpenAI-compatibility endpoint. `[understanding].provider =
/// "gemini"` is a preset that points this same client here: Gemini speaks
/// the chat-completions protocol, so — unlike embeddings, where a native
/// client is needed for `taskType` — no separate reasoning client is
/// warranted. It exposes only the standard features this client uses
/// (`messages`, `max_tokens`, `response_format`), and the schema is
/// repeated in the prompt with a repair pass besides, so structured
/// extraction holds up regardless of how faithfully it honours
/// `response_format`.
pub const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";

pub struct OpenAiCompatChatModel {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiCompatChatModel {
    pub fn new(
        model: impl Into<String>,
        api_key: impl Into<String>,
        base_url: &str,
    ) -> Result<Self> {
        Ok(Self {
            client: http_client(DEFAULT_TIMEOUT)
                .map_err(|e| RaError::Internal(format!("could not build an HTTP client: {e}")))?,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.into(),
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

/// Appends the schema to the system prompt, for servers that ignore
/// `response_format`.
pub(super) fn system_with_schema(request: &StructuredRequest) -> String {
    format!(
        "{}\n\nReply with a single JSON value and nothing else — no prose, no markdown \
         fences. It must conform to this JSON Schema:\n\n{}",
        request.system,
        serde_json::to_string_pretty(&request.schema)
            .unwrap_or_else(|_| request.schema.to_string())
    )
}

#[async_trait::async_trait]
impl TextCompletion for OpenAiCompatChatModel {
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
            "max_tokens": request.max_tokens,
            "messages": messages,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": request.schema_name,
                    "schema": request.schema,
                    "strict": false,
                },
            },
        });

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
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

    let choice = parsed
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| ChatError::Malformed("the response contained no choices".to_string()))?;

    // A length cut-off yields truncated JSON that repair cannot fix,
    // because the model was not wrong — it ran out of room.
    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        return Err(ChatError::Permanent(
            "the reply was truncated by the token limit. Raise max_tokens or reduce \
             the amount of text sent per job."
                .to_string(),
        ));
    }

    choice
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ChatError::Malformed("the choice contained no message content".to_string()))
}

#[async_trait::async_trait]
impl ChatModel for OpenAiCompatChatModel {
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
    fn reads_the_first_choices_content() {
        let body = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "{\"a\": 1}"}}]
        })
        .to_string();
        assert_eq!(extract_message_content(&body).unwrap(), "{\"a\": 1}");
    }

    #[test]
    fn a_truncated_reply_is_permanent_rather_than_sent_to_repair() {
        // Repair would re-ask with an even longer prompt and truncate
        // again — a loop that costs two calls to fail the same way.
        let body = json!({
            "choices": [{"finish_reason": "length", "message": {"content": "{\"a\":"}}]
        })
        .to_string();
        let error = extract_message_content(&body).unwrap_err();
        assert!(matches!(error, ChatError::Permanent(_)), "got {error:?}");
    }

    #[test]
    fn the_schema_reaches_the_prompt_for_servers_that_ignore_response_format() {
        let request = StructuredRequest::new(
            "Extract memories.",
            "some text",
            "candidates",
            json!({"type": "array", "items": {"type": "object"}}),
        );
        let system = system_with_schema(&request);
        assert!(system.contains("Extract memories."));
        assert!(system.contains("\"array\""), "{system}");
    }
}
