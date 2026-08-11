//! Wire types for ingestion and job polling.

use crate::shared::ids::MemoryId;
use crate::understanding::domain::ingest_job::{IngestPayload, JobRecord, JobStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct IngestRequest {
    /// Raw material: a sentence, a paragraph, a session summary.
    pub content: String,
    /// A category to suggest. Advisory — extraction may split the content
    /// into several memories that do not all share it.
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub client: Option<String>,
    pub session_id: Option<String>,
    /// Run the pipeline now and answer with the result, instead of
    /// returning a job to poll.
    ///
    /// Exists for callers that have nowhere to put a job id — an MCP tool
    /// has to tell the agent what happened in one turn, and "queued, ask
    /// again later" is not an answer an agent can act on. Everything else
    /// should take the 202: a model call can take seconds, and holding a
    /// request open for it makes the client's timeout our problem.
    #[serde(default)]
    pub wait: bool,
}

impl From<IngestRequest> for IngestPayload {
    fn from(request: IngestRequest) -> Self {
        Self {
            content: request.content,
            category: request.category,
            tags: request.tags,
            client: request.client,
            session_id: request.session_id,
        }
    }
}

/// Several submissions in one request.
///
/// Exists so a caller with a handful of things to remember at once — an
/// agent wrapping up a session, say — makes one request instead of one per
/// memory. Sending them separately works too, but N `wait: true` saves are
/// N races against the request timeout, and firing them concurrently makes
/// N pipeline runs contend for one model; a batch lets the daemon pace the
/// work behind a single, longer-lived request.
#[derive(Debug, Deserialize)]
pub struct BatchIngestRequest {
    pub items: Vec<BatchIngestItem>,
    /// Applied to every item's audit record. A per-item client would let
    /// one batch span several sources, which is not a thing a batch is.
    pub client: Option<String>,
    /// Run every item now and answer with the results, rather than
    /// returning job ids to poll. Same meaning, and same caveat, as on the
    /// single ingest — see [`IngestRequest::wait`].
    #[serde(default)]
    pub wait: bool,
}

#[derive(Debug, Deserialize)]
pub struct BatchIngestItem {
    pub content: String,
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub session_id: Option<String>,
}

impl BatchIngestItem {
    /// Turns one item into a payload, taking the batch-wide `client`.
    pub fn into_payload(self, client: Option<String>) -> IngestPayload {
        IngestPayload {
            content: self.content,
            category: self.category,
            tags: self.tags,
            client,
            session_id: self.session_id,
        }
    }
}

/// The `wait = false` batch answer: one job to poll per item, in the order
/// the items were sent.
#[derive(Debug, Serialize)]
pub struct BatchAcceptedResponse {
    pub jobs: Vec<AcceptedResponse>,
}

/// The `wait = true` batch answer: what each item produced, in order.
///
/// Reports per item rather than all-or-nothing: one item whose content the
/// model chokes on must not discard the memories the others yielded, so a
/// failed item is recorded in place and the rest still return.
#[derive(Debug, Serialize)]
pub struct BatchIngestedResponse {
    /// False when no provider is configured — the same signal the single
    /// ingest carries, and batch-wide because it is a property of the
    /// server, not the item.
    pub understanding: bool,
    pub results: Vec<BatchItemResult>,
}

#[derive(Debug, Serialize)]
pub struct BatchItemResult {
    pub job_id: String,
    pub status: &'static str,
    pub memory_ids: Vec<String>,
    /// Present only for an item that failed, carrying why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The 202 answer: what to poll.
#[derive(Debug, Serialize)]
pub struct AcceptedResponse {
    pub job_id: String,
    pub status: &'static str,
    /// Where to look. Spelled out rather than left for the client to
    /// construct, so the polling URL is part of the contract.
    pub poll: String,
}

/// The `wait = true` answer: what actually happened.
#[derive(Debug, Serialize)]
pub struct IngestedResponse {
    pub job_id: String,
    pub status: &'static str,
    pub memory_ids: Vec<String>,
    /// False when no provider is configured, so a caller can tell
    /// "extracted and reconciled" from "stored as sent".
    pub understanding: bool,
}

#[derive(Debug, Serialize)]
pub struct JobResponse {
    pub job_id: String,
    pub status: &'static str,
    pub attempts: u32,
    /// Present while a job is retrying, and after it dead-letters. Kept
    /// even once a later attempt succeeds — "it worked eventually, but
    /// here is what went wrong" is the more useful record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What the job produced. Empty until it finishes — and possibly
    /// still empty after, because "nothing here was worth remembering" is
    /// a legitimate outcome.
    pub memory_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&JobRecord> for JobResponse {
    fn from(record: &JobRecord) -> Self {
        Self {
            job_id: record.id.to_string(),
            status: status_name(record.status),
            attempts: record.attempts,
            error: record.error.clone(),
            memory_ids: record.memory_ids.iter().map(MemoryId::to_string).collect(),
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

/// The wire spelling of a status.
///
/// Deliberately its own function rather than reusing `JobStatus::as_str`:
/// the internal names are free to change, this is a promise to clients.
pub fn status_name(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Pending => "pending",
        JobStatus::Running => "running",
        JobStatus::Succeeded => "succeeded",
        JobStatus::DeadLetter => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_content_is_required() {
        let request: IngestRequest = serde_json::from_value(json!({"content": "hi"})).unwrap();
        assert_eq!(request.content, "hi");
        assert!(!request.wait, "waiting must be opt-in");
        assert!(request.tags.is_empty());
    }

    #[test]
    fn a_batch_item_defaults_its_tags_and_carries_the_batch_client() {
        let request: BatchIngestRequest = serde_json::from_value(json!({
            "items": [{"content": "prefers pnpm"}, {"content": "on hetzner", "category": "fact.project"}],
            "client": "claude",
        }))
        .unwrap();

        assert_eq!(request.items.len(), 2);
        assert!(!request.wait, "waiting must be opt-in for a batch too");

        let payload = request
            .items
            .into_iter()
            .next()
            .unwrap()
            .into_payload(request.client.clone());
        assert_eq!(payload.content, "prefers pnpm");
        assert_eq!(payload.client.as_deref(), Some("claude"));
        assert!(payload.tags.is_empty());
    }

    #[test]
    fn a_batch_needs_at_least_an_items_key() {
        // An absent `items` is a malformed request, not an empty batch.
        assert!(serde_json::from_value::<BatchIngestRequest>(json!({})).is_err());
    }

    #[test]
    fn dead_letter_is_reported_as_failed() {
        // "dead_letter" is queue jargon. A client polling a job wants to
        // know it failed.
        assert_eq!(status_name(JobStatus::DeadLetter), "failed");
    }

    #[test]
    fn a_job_response_omits_a_missing_error_rather_than_sending_null() {
        let record = JobRecord {
            id: crate::shared::ids::JobId::new(),
            status: JobStatus::Succeeded,
            attempts: 1,
            error: None,
            memory_ids: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let encoded = serde_json::to_value(JobResponse::from(&record)).unwrap();
        assert!(encoded.get("error").is_none(), "{encoded}");
        assert_eq!(encoded["status"], "succeeded");
    }
}
