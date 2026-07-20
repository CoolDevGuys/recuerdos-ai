//! The unit of async ingestion, and the queue that holds it.
//!
//! # Why the queue contract is not user-scoped
//!
//! Every other repository in this codebase takes `&UserContext`, so that
//! reaching another user's data cannot compile. This one splits: the
//! caller-facing methods (`enqueue`, `find`) take a context and are
//! scoped exactly like everything else, while `claim_next` and the
//! completion methods do not — because a worker is not acting for anyone
//! yet. It claims a job, reads whose it is, and *then* resolves a context
//! for that user.
//!
//! The guarantee survives because the user id on a job row can only have
//! been written by `enqueue`, which had a context. A worker cannot ask
//! for a particular user's work, and nothing a caller sends can change
//! whose memories a job writes.

use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use crate::shared::ids::{JobId, MemoryId, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What was submitted for ingestion.
///
/// Serialised into the job row whole, so a job can be replayed exactly as
/// it was received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestPayload {
    /// The raw material — a sentence, a paragraph, a session summary.
    pub content: String,
    /// A category the caller suggests. Advisory: extraction may split the
    /// content into several memories that do not all share it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Succeeded,
    /// Out of attempts. Terminal, and the signal that a human should look
    /// — a job that silently vanished after N failures would lose a
    /// user's memory with no trace.
    DeadLetter,
}

impl JobStatus {
    /// Rebuilds from storage. An unrecognised value means the row was
    /// written by a newer version; treating it as pending would re-run
    /// work that may already have happened, so it reads as running —
    /// "something else has this", the safest thing to assume.
    pub fn from_stored(raw: &str) -> Self {
        match raw {
            "pending" => JobStatus::Pending,
            "succeeded" => JobStatus::Succeeded,
            "dead_letter" => JobStatus::DeadLetter,
            _ => JobStatus::Running,
        }
    }
}

/// A job as a caller sees it, for `GET /v1/jobs/{id}`.
#[derive(Debug, Clone)]
pub struct JobRecord {
    pub id: JobId,
    pub status: JobStatus,
    pub attempts: u32,
    pub error: Option<String>,
    /// The memories this job produced. Empty until it succeeds — and
    /// possibly still empty after, because "nothing here was worth
    /// remembering" is a legitimate outcome.
    pub memory_ids: Vec<MemoryId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A job a worker has taken responsibility for.
#[derive(Debug, Clone)]
pub struct ClaimedJob {
    pub id: JobId,
    /// Whose work this is — the only permitted source of that answer.
    pub user_id: UserId,
    pub payload: IngestPayload,
    /// How many attempts have been made *including* this one.
    pub attempts: u32,
}

pub trait JobQueue: Send + Sync {
    /// Records a submission. Fast by construction — one insert, no model
    /// call — because the caller is waiting on it.
    fn enqueue(
        &self,
        context: &UserContext,
        payload: &IngestPayload,
        now: DateTime<Utc>,
    ) -> Result<JobId>;

    /// Takes the oldest claimable job, marking it running.
    ///
    /// Must be atomic against other workers: two workers claiming the
    /// same job would extract the same content twice and produce
    /// duplicate memories.
    fn claim_next(&self, now: DateTime<Utc>) -> Result<Option<ClaimedJob>>;

    /// Marks a job done and records what it produced.
    fn succeed(&self, id: JobId, memory_ids: &[MemoryId], now: DateTime<Utc>) -> Result<()>;

    /// Records a failed attempt.
    ///
    /// `retry_after` present means try again then; absent means the job
    /// is out of attempts and dead-letters.
    fn fail(
        &self,
        id: JobId,
        error: &str,
        retry_after: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<()>;

    /// Returns jobs held by a worker that no longer exists to `pending`.
    ///
    /// Called at startup: a process killed mid-job leaves a row marked
    /// running forever, and a user's memory quietly never arrives.
    /// Returns how many were reclaimed.
    fn reclaim_stale(&self, stale_before: DateTime<Utc>, now: DateTime<Utc>) -> Result<usize>;

    fn find(&self, context: &UserContext, id: JobId) -> Result<Option<JobRecord>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_spellings_are_the_ones_the_schema_writes() {
        // Asserted against literals rather than against a helper, because
        // these strings are what `V3__jobs.sql` and the queue's SQL put in
        // the column. A helper here would only prove it agreed with itself.
        assert_eq!(JobStatus::from_stored("pending"), JobStatus::Pending);
        assert_eq!(JobStatus::from_stored("running"), JobStatus::Running);
        assert_eq!(JobStatus::from_stored("succeeded"), JobStatus::Succeeded);
        assert_eq!(JobStatus::from_stored("dead_letter"), JobStatus::DeadLetter);
    }

    #[test]
    fn an_unknown_stored_status_reads_as_running_not_pending() {
        // A row written by a newer version must not be re-run: at worst
        // it stalls until reclaimed, rather than duplicating memories.
        assert_eq!(JobStatus::from_stored("quantum"), JobStatus::Running);
    }

    #[test]
    fn a_payload_round_trips_through_json() {
        // The row stores this verbatim so a failed job can be replayed
        // exactly as received.
        let payload = IngestPayload {
            content: "I prefer pnpm".to_string(),
            category: Some("preference.coding".to_string()),
            tags: vec!["tooling".to_string()],
            client: Some("claude-code".to_string()),
            session_id: None,
        };

        let json = serde_json::to_string(&payload).unwrap();
        assert_eq!(
            serde_json::from_str::<IngestPayload>(&json).unwrap(),
            payload
        );
    }

    #[test]
    fn a_minimal_payload_needs_only_content() {
        let payload: IngestPayload = serde_json::from_str(r#"{"content": "hi"}"#).unwrap();
        assert_eq!(payload.content, "hi");
        assert!(payload.tags.is_empty());
        assert!(payload.category.is_none());
    }
}
