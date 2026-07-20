//! A `ChatModel` that answers from a script.
//!
//! The pipeline's use cases are mostly *prompt assembly* and *output
//! validation*: what we ask, and what we do with what comes back. Both
//! are testable without a model, and a real one would make those tests
//! non-deterministic, slow, and dependent on a network and a credit card.
//!
//! It also records every request, so a test can assert on the prompt —
//! that the taxonomy reached the model, that neighbours were included in
//! a reconciliation call — which is the part most likely to regress
//! silently, because a dropped instruction still produces plausible JSON.
//!
//! Lives with the use cases rather than in `providers` because it is
//! their test fixture, not a technology. Boundary rule 2 also forbids
//! application code from importing infrastructure, so a double kept over
//! there would be unusable by the tests that need it.

use crate::understanding::domain::chat_model::{
    ChatError, ChatModel, ChatResult, StructuredRequest,
};
use serde_json::Value;
use std::sync::Mutex;

/// One queued outcome.
enum Reply {
    Value(Value),
    Failure(ChatError),
}

pub struct ScriptedChatModel {
    replies: Mutex<Vec<Reply>>,
    requests: Mutex<Vec<StructuredRequest>>,
    model: String,
}

impl Default for ScriptedChatModel {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedChatModel {
    pub fn new() -> Self {
        Self {
            replies: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            model: "scripted".to_string(),
        }
    }

    /// Queues a JSON answer. Replies are returned in the order queued.
    pub fn queue(self, value: Value) -> Self {
        self.replies.lock().unwrap().push(Reply::Value(value));
        self
    }

    /// Queues a failure, for testing how a use case degrades.
    pub fn queue_error(self, error: ChatError) -> Self {
        self.replies.lock().unwrap().push(Reply::Failure(error));
        self
    }

    /// Every request made so far, in order.
    pub fn requests(&self) -> Vec<StructuredRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// The system + user text of the nth call, for prompt assertions.
    pub fn prompt(&self, index: usize) -> String {
        let requests = self.requests.lock().unwrap();
        let request = requests
            .get(index)
            .unwrap_or_else(|| panic!("no call {index}; there were {}", requests.len()));
        format!("{}\n{}", request.system, request.user)
    }
}

#[async_trait::async_trait]
impl ChatModel for ScriptedChatModel {
    fn model_id(&self) -> &str {
        &self.model
    }

    async fn complete_structured(&self, request: &StructuredRequest) -> ChatResult<Value> {
        self.requests.lock().unwrap().push(request.clone());

        let mut replies = self.replies.lock().unwrap();
        if replies.is_empty() {
            // Louder than returning an empty result: a use case that made
            // one more call than the test expected is a real change in
            // behaviour (an extra LLM call is real money), and silently
            // absorbing it would hide that.
            return Err(ChatError::Permanent(
                "ScriptedChatModel ran out of queued replies — the use case made more \
                 calls than the test scripted"
                    .to_string(),
            ));
        }

        match replies.remove(0) {
            Reply::Value(value) => Ok(value),
            Reply::Failure(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(user: &str) -> StructuredRequest {
        StructuredRequest::new("system rules", user, "answer", json!({"type": "object"}))
    }

    #[tokio::test]
    async fn replies_come_back_in_the_order_queued() {
        let model = ScriptedChatModel::new()
            .queue(json!({"n": 1}))
            .queue(json!({"n": 2}));

        assert_eq!(
            model.complete_structured(&request("a")).await.unwrap(),
            json!({"n": 1})
        );
        assert_eq!(
            model.complete_structured(&request("b")).await.unwrap(),
            json!({"n": 2})
        );
    }

    #[tokio::test]
    async fn requests_are_recorded_for_prompt_assertions() {
        let model = ScriptedChatModel::new().queue(json!({}));
        model
            .complete_structured(&request("the raw text"))
            .await
            .unwrap();

        assert_eq!(model.call_count(), 1);
        assert!(model.prompt(0).contains("system rules"));
        assert!(model.prompt(0).contains("the raw text"));
    }

    #[tokio::test]
    async fn an_unscripted_call_fails_loudly() {
        let model = ScriptedChatModel::new();
        let error = model.complete_structured(&request("a")).await.unwrap_err();
        assert!(
            error.to_string().contains("ran out of queued replies"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn failures_can_be_scripted_too() {
        let model = ScriptedChatModel::new().queue_error(ChatError::Transient("429".to_string()));
        let error = model.complete_structured(&request("a")).await.unwrap_err();
        assert!(matches!(error, ChatError::Transient(_)), "got {error:?}");
    }
}
