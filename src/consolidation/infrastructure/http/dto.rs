//! Wire types for session distillation.

use crate::consolidation::domain::distillation::Distillation;
use crate::shared::ids::MemoryId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct DistillRequest {
    /// The session: a transcript, or a summary of one. A summary is
    /// usually the better input — it is what a PreCompact hook already
    /// has, and it costs a fraction of the tokens.
    pub content: String,
    /// The client's own id for the session, recorded on every memory it
    /// produces.
    pub session_id: Option<String>,
    pub client: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// What the session left behind.
///
/// Synchronous, unlike `POST /v1/memories`. The caller is a session-end
/// hook with nowhere to put a job id and no next turn in which to poll —
/// and if it disconnects, the session it was summarising is over and
/// nobody will come back for the result.
#[derive(Debug, Serialize)]
pub struct DistillResponse {
    pub memory_ids: Vec<String>,
    /// How many memories survived the session. Spelled out because zero
    /// is the common answer and a client should be able to report
    /// "nothing worth keeping" without inspecting an array's length.
    pub distilled: usize,
}

impl From<&Distillation> for DistillResponse {
    fn from(distillation: &Distillation) -> Self {
        Self {
            memory_ids: distillation
                .memory_ids
                .iter()
                .map(MemoryId::to_string)
                .collect(),
            distilled: distillation.memory_ids.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_content_is_required() {
        let request: DistillRequest =
            serde_json::from_value(json!({"content": "a session"})).unwrap();

        assert_eq!(request.content, "a session");
        assert!(request.tags.is_empty());
        assert!(request.session_id.is_none());
    }

    #[test]
    fn an_empty_distillation_says_so_rather_than_looking_like_a_failure() {
        let encoded =
            serde_json::to_value(DistillResponse::from(&Distillation::default())).unwrap();

        assert_eq!(encoded["distilled"], 0);
        assert_eq!(encoded["memory_ids"], json!([]));
    }
}
