//! Turning a model's reply into JSON, with one repair attempt.
//!
//! Every provider here ultimately hands back a string. Parsing it, and
//! deciding what to do when it is not valid JSON, is identical across all
//! three — so it lives once, and the clients stay dumb transports.
//!
//! # Why exactly one repair attempt
//!
//! Models that get JSON wrong usually get it wrong in a way they can fix
//! when shown the parse error: a trailing comma, a stray sentence before
//! the object. Models that get it wrong *twice* are misunderstanding the
//! schema, and a third call spends money to learn nothing. One retry
//! captures nearly all of the recoverable cases and bounds the cost of
//! the unrecoverable ones.

use crate::understanding::domain::chat_model::{ChatError, ChatResult, StructuredRequest};
use serde_json::Value;

/// A provider's raw transport, before JSON handling.
///
/// `repair` carries the previous bad output and its parse error when this
/// is the second attempt; a client passes it back to the model as an
/// extra turn.
#[async_trait::async_trait]
pub trait TextCompletion: Send + Sync {
    async fn complete_text(
        &self,
        request: &StructuredRequest,
        repair: Option<&RepairHint>,
    ) -> ChatResult<String>;
}

/// What the model got wrong last time.
#[derive(Debug, Clone)]
pub struct RepairHint {
    pub previous_output: String,
    pub parse_error: String,
}

impl RepairHint {
    /// The corrective turn, phrased as the user coming back with the
    /// problem rather than as a system rule — it is feedback about this
    /// one answer, not a standing instruction.
    pub fn as_message(&self) -> String {
        format!(
            "Your previous reply could not be parsed as JSON ({}). Here it is:\n\n{}\n\n\
             Reply again with only the JSON value, conforming to the schema. \
             No prose, no markdown fences.",
            self.parse_error, self.previous_output
        )
    }
}

/// Runs the completion and parses it, repairing once if needed.
pub async fn structured_from_text(
    client: &dyn TextCompletion,
    request: &StructuredRequest,
) -> ChatResult<Value> {
    let first = client.complete_text(request, None).await?;
    let error = match parse_json(&first) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };

    tracing::debug!(%error, "model returned unparseable JSON; attempting one repair");

    let hint = RepairHint {
        previous_output: truncate_for_prompt(&first),
        parse_error: error.clone(),
    };
    let second = client.complete_text(request, Some(&hint)).await?;

    parse_json(&second).map_err(|second_error| {
        ChatError::Malformed(format!(
            "unparseable JSON after one repair attempt (first: {error}; second: {second_error})"
        ))
    })
}

/// Parses a reply, tolerating the two things models do to JSON even when
/// told not to: wrapping it in a markdown fence, and prefacing it with a
/// sentence. Anything beyond that is a genuine failure and goes to repair.
fn parse_json(raw: &str) -> Result<Value, String> {
    let trimmed = strip_code_fence(raw.trim());

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }

    // Fall back to the first balanced JSON value in the text.
    if let Some(slice) = first_json_value(trimmed)
        && let Ok(value) = serde_json::from_str::<Value>(slice)
    {
        return Ok(value);
    }

    Err(serde_json::from_str::<Value>(trimmed)
        .expect_err("we only reach here when parsing failed")
        .to_string())
}

fn strip_code_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    // ```json\n{...}\n``` — drop the language tag line and the closing fence.
    let body = rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest);
    body.trim_end().strip_suffix("```").unwrap_or(body).trim()
}

/// The first `{...}` or `[...]` region with balanced delimiters, ignoring
/// braces inside strings. Enough to rescue "Here you go: {...}".
fn first_json_value(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|b| *b == b'{' || *b == b'[')?;
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, byte) in bytes[start..].iter().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b if b == open => depth += 1,
            b if b == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + offset + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Bad output can be as long as `max_tokens`; echoing all of it back
/// doubles the cost of the repair call for no extra signal.
fn truncate_for_prompt(text: &str) -> String {
    const LIMIT: usize = 2_000;
    if text.chars().count() <= LIMIT {
        return text.to_string();
    }
    let head: String = text.chars().take(LIMIT).collect();
    format!("{head}… (truncated)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// Returns queued replies and records what it was asked.
    struct Replies {
        queued: Mutex<Vec<String>>,
        repairs: Mutex<Vec<Option<RepairHint>>>,
    }

    impl Replies {
        fn new(replies: &[&str]) -> Self {
            Self {
                queued: Mutex::new(replies.iter().rev().map(|s| s.to_string()).collect()),
                repairs: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl TextCompletion for Replies {
        async fn complete_text(
            &self,
            _request: &StructuredRequest,
            repair: Option<&RepairHint>,
        ) -> ChatResult<String> {
            self.repairs.lock().unwrap().push(repair.cloned());
            self.queued
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| ChatError::Permanent("no reply queued".to_string()))
        }
    }

    fn request() -> StructuredRequest {
        StructuredRequest::new("system", "user", "answer", json!({"type": "object"}))
    }

    #[tokio::test]
    async fn clean_json_needs_no_repair() {
        let client = Replies::new(&[r#"{"ok": true}"#]);
        let value = structured_from_text(&client, &request()).await.unwrap();
        assert_eq!(value, json!({"ok": true}));
        assert_eq!(client.repairs.lock().unwrap().len(), 1, "one call only");
    }

    #[tokio::test]
    async fn a_markdown_fence_is_stripped_rather_than_costing_a_repair_call() {
        let client = Replies::new(&["```json\n{\"ok\": true}\n```"]);
        let value = structured_from_text(&client, &request()).await.unwrap();
        assert_eq!(value, json!({"ok": true}));
        assert_eq!(
            client.repairs.lock().unwrap().len(),
            1,
            "fencing is common enough that paying for a second call would be waste"
        );
    }

    #[tokio::test]
    async fn a_preamble_sentence_is_stripped() {
        let client = Replies::new(&["Sure, here you go: [{\"a\": 1}] — hope that helps!"]);
        let value = structured_from_text(&client, &request()).await.unwrap();
        assert_eq!(value, json!([{"a": 1}]));
    }

    #[tokio::test]
    async fn braces_inside_strings_do_not_end_the_value_early() {
        let client = Replies::new(&[r#"note: {"content": "use a } brace", "n": 1}"#]);
        let value = structured_from_text(&client, &request()).await.unwrap();
        assert_eq!(value, json!({"content": "use a } brace", "n": 1}));
    }

    #[tokio::test]
    async fn malformed_output_gets_exactly_one_repair_attempt() {
        let client = Replies::new(&["{not json at all", r#"{"ok": true}"#]);
        let value = structured_from_text(&client, &request()).await.unwrap();
        assert_eq!(value, json!({"ok": true}));

        let repairs = client.repairs.lock().unwrap();
        assert_eq!(repairs.len(), 2);
        assert!(repairs[0].is_none(), "the first call carries no hint");
        let hint = repairs[1]
            .as_ref()
            .expect("the second call must be told why");
        assert!(hint.previous_output.contains("not json at all"));
        assert!(hint.as_message().contains("could not be parsed"));
    }

    #[tokio::test]
    async fn twice_malformed_is_an_error_not_a_third_attempt() {
        let client = Replies::new(&["nonsense", "still nonsense", r#"{"ok": true}"#]);
        let error = structured_from_text(&client, &request()).await.unwrap_err();

        assert!(matches!(error, ChatError::Malformed(_)), "got {error:?}");
        assert_eq!(
            client.repairs.lock().unwrap().len(),
            2,
            "a third call would spend money to learn nothing"
        );
    }

    #[tokio::test]
    async fn a_transient_failure_is_not_swallowed_by_the_repair_path() {
        struct Failing;
        #[async_trait::async_trait]
        impl TextCompletion for Failing {
            async fn complete_text(
                &self,
                _request: &StructuredRequest,
                _repair: Option<&RepairHint>,
            ) -> ChatResult<String> {
                Err(ChatError::Transient("429".to_string()))
            }
        }

        let error = structured_from_text(&Failing, &request())
            .await
            .unwrap_err();
        assert!(
            matches!(error, ChatError::Transient(_)),
            "a rate limit must stay retryable, not be reported as bad output: {error:?}"
        );
    }
}
