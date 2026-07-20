//! Anthropic Messages API.
//!
//! # Why tool use rather than the structured-outputs parameter
//!
//! Anthropic offers `output_config.format` for schema-constrained JSON,
//! and on a current model it is the nicer API. Forced tool use is used
//! here instead because it works on *every* model back to Claude 3, and
//! `[understanding].model` is an operator's free-text config field. A
//! deployment pinning an older model for cost should get working
//! extraction, not a 400 from a parameter their model never learned.
//!
//! The mechanism: declare one tool whose `input_schema` is the schema we
//! want, then force it with `tool_choice`. The model's `tool_use.input`
//! is then the answer, already parsed by the API's own validator.

use super::structured_text::{RepairHint, TextCompletion, structured_from_text};
use super::transport::{DEFAULT_TIMEOUT, classify_status, classify_transport, http_client};
use crate::shared::error::{RaError, Result};
use crate::understanding::domain::chat_model::{
    ChatError, ChatModel, ChatResult, StructuredRequest,
};
use serde_json::{Value, json};
#[cfg(test)]
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// The API version header. Pinned rather than tracking latest: Anthropic
/// uses it precisely so that a server-side change cannot alter the shape
/// we parse.
const API_VERSION: &str = "2023-06-01";

/// The single tool the model is forced to call. Its name reaches the
/// model, so it is descriptive rather than `f`.
const TOOL_NAME: &str = "record_result";

pub struct AnthropicChatModel {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl AnthropicChatModel {
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

    /// Overrides the request timeout, so the contract tests can exercise
    /// the timeout path without a minute of waiting.
    #[cfg(test)]
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        self.client = http_client(timeout)
            .map_err(|e| RaError::Internal(format!("could not build an HTTP client: {e}")))?;
        Ok(self)
    }
}

#[async_trait::async_trait]
impl TextCompletion for AnthropicChatModel {
    async fn complete_text(
        &self,
        request: &StructuredRequest,
        repair: Option<&RepairHint>,
    ) -> ChatResult<String> {
        let mut messages = vec![json!({"role": "user", "content": request.user})];
        if let Some(hint) = repair {
            // Anthropic requires alternating roles, so the failed answer
            // goes back as the assistant turn it actually was.
            messages.push(json!({"role": "assistant", "content": hint.previous_output}));
            messages.push(json!({"role": "user", "content": hint.as_message()}));
        }

        let body = json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "system": request.system,
            "messages": messages,
            "tools": [{
                "name": TOOL_NAME,
                "description": format!("Record the {} result.", request.schema_name),
                "input_schema": request.schema,
            }],
            "tool_choice": {"type": "tool", "name": TOOL_NAME},
        });

        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(classify_transport)?;

        let status = response.status();
        let text = response.text().await.map_err(classify_transport)?;
        if !status.is_success() {
            return Err(classify_status(status, &text));
        }

        extract_tool_input(&text)
    }
}

/// Pulls the forced tool call's input out of the response.
///
/// Returned as a string so the shared repair path is the same for every
/// provider. In practice the API has already validated it against the
/// schema, so the reparse downstream always succeeds — the uniformity is
/// worth more than the microseconds.
fn extract_tool_input(body: &str) -> ChatResult<String> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|e| ChatError::Malformed(format!("response was not JSON: {e}")))?;

    // A refusal or a max_tokens cut-off arrives as HTTP 200 with no tool
    // call. Reporting it as "malformed" would be a lie the operator has
    // to debug, so the stop reason is named.
    let stop_reason = parsed
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    parsed
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        })
        .and_then(|block| block.get("input"))
        .map(|input| input.to_string())
        .ok_or_else(|| {
            let error =
                format!("the response contained no tool_use block (stop_reason: {stop_reason})");
            // `max_tokens` means the schema was too large for the budget —
            // a configuration problem, not a flaky model, so don't retry.
            if stop_reason == "max_tokens" {
                ChatError::Permanent(format!(
                    "{error}. Raise max_tokens or reduce the amount of text sent per job."
                ))
            } else {
                ChatError::Malformed(error)
            }
        })
}

#[async_trait::async_trait]
impl ChatModel for AnthropicChatModel {
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
    fn reads_the_forced_tool_call() {
        let body = json!({
            "stop_reason": "tool_use",
            "content": [
                {"type": "text", "text": "Let me record that."},
                {"type": "tool_use", "name": TOOL_NAME, "input": {"candidates": []}},
            ],
        })
        .to_string();

        let extracted = extract_tool_input(&body).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&extracted).unwrap(),
            json!({"candidates": []})
        );
    }

    #[test]
    fn a_truncated_response_names_the_cause_and_is_not_retried() {
        // Retrying an over-budget request just produces the same
        // truncation, so this must not look transient.
        let body = json!({"stop_reason": "max_tokens", "content": []}).to_string();
        let error = extract_tool_input(&body).unwrap_err();

        assert!(matches!(error, ChatError::Permanent(_)), "got {error:?}");
        assert!(error.to_string().contains("max_tokens"), "{error}");
    }

    #[test]
    fn a_refusal_is_reported_with_its_stop_reason() {
        let body = json!({"stop_reason": "refusal", "content": []}).to_string();
        let error = extract_tool_input(&body).unwrap_err();
        assert!(error.to_string().contains("refusal"), "{error}");
    }
}
