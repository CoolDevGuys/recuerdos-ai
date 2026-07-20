//! Tests for the SQLite job queue.
//!
//! The queue is the durability promise behind a `202`: once the API says
//! it accepted a submission, that memory has to arrive even if the
//! process dies a millisecond later. Most of what follows is about the
//! ways that promise can quietly break — two workers taking one job, a
//! crash leaving a row held forever, a poison job retrying without limit.

use super::sqlite_job_queue::SqliteJobQueue;
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::RaError;
use crate::shared::ids::{JobId, MemoryId};
use crate::shared::sqlite::SqliteDatabase;
use crate::understanding::domain::ingest_job::{IngestPayload, JobQueue, JobStatus};
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;

struct Fixture {
    queue: SqliteJobQueue,
    database: Arc<SqliteDatabase>,
    alex: UserContext,
    sam: UserContext,
}

fn fixture() -> Fixture {
    let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
    let identity =
        crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();

    Fixture {
        queue: SqliteJobQueue::new(Arc::clone(&database)),
        database,
        alex: authenticate(&identity, "alex"),
        sam: authenticate(&identity, "sam"),
    }
}

/// Builds a real `UserContext` the only way anything can — by
/// authenticating. Tests cannot forge one, which is the point.
fn authenticate(identity: &crate::bootstrap::wiring::Identity, handle: &str) -> UserContext {
    identity.user_creator.execute(handle, None).unwrap();
    let issued = identity
        .api_key_issuer
        .execute(
            handle,
            vec![crate::identity::domain::scope::Scope::Admin],
            "test",
        )
        .unwrap();
    identity
        .key_authenticator
        .execute(&issued.token.render())
        .unwrap()
}

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000, 0).unwrap()
}

fn payload(content: &str) -> IngestPayload {
    IngestPayload {
        content: content.to_string(),
        category: None,
        tags: vec![],
        client: Some("rest".to_string()),
        session_id: None,
    }
}

#[test]
fn an_enqueued_job_is_pending_and_visible_to_its_owner() {
    let f = fixture();

    let id = f
        .queue
        .enqueue(&f.alex, &payload("I prefer pnpm"), now())
        .unwrap();

    let record = f.queue.find(&f.alex, id).unwrap().expect("the job");
    assert_eq!(record.status, JobStatus::Pending);
    assert_eq!(record.attempts, 0);
    assert!(record.memory_ids.is_empty());
    assert!(record.error.is_none());
}

#[test]
fn a_job_is_invisible_to_another_user() {
    // Job ids are uuids, so this is not the primary defence — but a
    // status endpoint that leaked "running, 2 attempts, error: …" for a
    // guessed id would leak the shape of someone else's activity.
    let f = fixture();
    let id = f.queue.enqueue(&f.alex, &payload("secret"), now()).unwrap();

    assert!(
        f.queue.find(&f.sam, id).unwrap().is_none(),
        "another user could see the job"
    );
}

#[test]
fn claiming_returns_the_payload_and_whose_it_is() {
    let f = fixture();
    f.queue
        .enqueue(&f.alex, &payload("I prefer pnpm"), now())
        .unwrap();

    let claimed = f.queue.claim_next(now()).unwrap().expect("a job");

    assert_eq!(claimed.payload.content, "I prefer pnpm");
    assert_eq!(claimed.payload.client.as_deref(), Some("rest"));
    assert_eq!(
        claimed.user_id,
        f.alex.user_id(),
        "the worker must learn whose memories to write from the row, not from a caller"
    );
    assert_eq!(claimed.attempts, 1, "attempts count the current try");
}

#[test]
fn a_claimed_job_is_not_handed_to_a_second_worker() {
    // The bug this prevents: both workers extract the same content and
    // the user ends up with every memory twice.
    let f = fixture();
    f.queue.enqueue(&f.alex, &payload("once"), now()).unwrap();

    assert!(f.queue.claim_next(now()).unwrap().is_some());
    assert!(
        f.queue.claim_next(now()).unwrap().is_none(),
        "the same job was claimed twice"
    );
}

#[test]
fn jobs_are_claimed_oldest_first() {
    let f = fixture();
    f.queue.enqueue(&f.alex, &payload("first"), now()).unwrap();
    f.queue
        .enqueue(&f.alex, &payload("second"), now() + Duration::seconds(1))
        .unwrap();

    let first = f.queue.claim_next(now() + Duration::seconds(2)).unwrap();
    assert_eq!(first.unwrap().payload.content, "first");
}

#[test]
fn succeeding_records_the_memories_the_job_produced() {
    let f = fixture();
    let id = f.queue.enqueue(&f.alex, &payload("text"), now()).unwrap();
    f.queue.claim_next(now()).unwrap();

    let produced = vec![MemoryId::new(), MemoryId::new()];
    f.queue.succeed(id, &produced, now()).unwrap();

    let record = f.queue.find(&f.alex, id).unwrap().unwrap();
    assert_eq!(record.status, JobStatus::Succeeded);
    assert_eq!(
        record.memory_ids, produced,
        "a caller who submitted raw text learns what it became from here"
    );
}

#[test]
fn a_successful_job_produces_no_further_work() {
    let f = fixture();
    let id = f.queue.enqueue(&f.alex, &payload("text"), now()).unwrap();
    f.queue.claim_next(now()).unwrap();
    f.queue.succeed(id, &[], now()).unwrap();

    assert!(
        f.queue
            .claim_next(now() + Duration::hours(1))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_retryable_failure_returns_the_job_to_the_queue_after_its_backoff() {
    let f = fixture();
    let id = f.queue.enqueue(&f.alex, &payload("text"), now()).unwrap();
    f.queue.claim_next(now()).unwrap();

    let retry_at = now() + Duration::seconds(30);
    f.queue
        .fail(id, "429 from the provider", Some(retry_at), now())
        .unwrap();

    assert!(
        f.queue
            .claim_next(now() + Duration::seconds(5))
            .unwrap()
            .is_none(),
        "the backoff was ignored — a rate-limited provider would be hammered"
    );

    let claimed = f.queue.claim_next(retry_at).unwrap().expect("retried");
    assert_eq!(
        claimed.attempts, 2,
        "the attempt counter must carry across retries"
    );

    // The reason for the previous failure is still readable while it retries.
    let record = f.queue.find(&f.alex, id).unwrap().unwrap();
    assert_eq!(record.error.as_deref(), Some("429 from the provider"));
}

#[test]
fn a_poison_job_dead_letters_rather_than_retrying_forever() {
    let f = fixture();
    let id = f.queue.enqueue(&f.alex, &payload("text"), now()).unwrap();
    f.queue.claim_next(now()).unwrap();

    f.queue
        .fail(id, "the model refuses this content", None, now())
        .unwrap();

    let record = f.queue.find(&f.alex, id).unwrap().unwrap();
    assert_eq!(record.status, JobStatus::DeadLetter);
    assert_eq!(
        record.error.as_deref(),
        Some("the model refuses this content")
    );
    assert!(
        f.queue
            .claim_next(now() + Duration::days(1))
            .unwrap()
            .is_none(),
        "a dead-lettered job must not be picked up again"
    );
}

#[test]
fn a_job_held_by_a_dead_worker_is_reclaimed() {
    // The crash case. Without this the row stays 'running' forever and
    // the user's memory silently never arrives — the worst failure mode
    // available, because nothing reports an error.
    let f = fixture();
    f.queue.enqueue(&f.alex, &payload("text"), now()).unwrap();
    f.queue.claim_next(now()).unwrap();

    let later = now() + Duration::minutes(30);
    let reclaimed = f
        .queue
        .reclaim_stale(later - Duration::minutes(10), later)
        .unwrap();

    assert_eq!(reclaimed, 1);
    let claimed = f.queue.claim_next(later).unwrap().expect("reclaimed job");
    assert_eq!(
        claimed.attempts, 2,
        "a crash should cost one attempt, so a job that reliably kills the \
         worker still dead-letters eventually"
    );
}

#[test]
fn a_job_claimed_moments_ago_is_not_reclaimed_from_a_live_worker() {
    // Reclaiming too eagerly is the mirror-image bug: a slow-but-healthy
    // job gets taken by a second worker and runs twice.
    let f = fixture();
    f.queue.enqueue(&f.alex, &payload("text"), now()).unwrap();
    f.queue.claim_next(now()).unwrap();

    let reclaimed = f
        .queue
        .reclaim_stale(now() - Duration::minutes(10), now())
        .unwrap();

    assert_eq!(reclaimed, 0);
}

#[test]
fn reclaiming_clears_any_backoff_so_the_retry_is_immediate() {
    let f = fixture();
    let id = f.queue.enqueue(&f.alex, &payload("text"), now()).unwrap();
    f.queue.claim_next(now()).unwrap();
    // A failure pushes run_after far out, then the retry crashes.
    f.queue
        .fail(id, "transient", Some(now() + Duration::hours(2)), now())
        .unwrap();
    let retry_at = now() + Duration::hours(2);
    f.queue.claim_next(retry_at).unwrap();

    let later = retry_at + Duration::hours(1);
    f.queue
        .reclaim_stale(later - Duration::minutes(1), later)
        .unwrap();

    assert!(
        f.queue.claim_next(later).unwrap().is_some(),
        "the reclaimed job stayed behind a backoff set for a different failure"
    );
}

#[test]
fn jobs_survive_a_restart() {
    // The whole point of persisting rather than queueing in memory: a
    // 202 that evaporates on deploy is a lie to the caller.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recordagent.db");

    let id = {
        let database = Arc::new(SqliteDatabase::open(&path).unwrap());
        let identity =
            crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();
        let alex = authenticate(&identity, "alex");
        SqliteJobQueue::new(database)
            .enqueue(&alex, &payload("survive this"), now())
            .unwrap()
    };

    // A fresh process, same file.
    let database = Arc::new(SqliteDatabase::open(&path).unwrap());
    let queue = SqliteJobQueue::new(database);

    let claimed = queue
        .claim_next(now())
        .unwrap()
        .expect("job lost on restart");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.payload.content, "survive this");
}

#[test]
fn a_job_for_a_missing_user_is_refused_at_enqueue() {
    // The foreign key doing its job: better to reject the submission than
    // to accept work nothing can ever run.
    let f = fixture();
    f.database
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO ingest_jobs (id, user_id, payload, created_at, updated_at, run_after)
                     VALUES ('j1', 'no-such-user', '{}', ?1, ?1, ?1)",
                    rusqlite::params![now().to_rfc3339()],
                )
                .map_err(|e| crate::shared::sqlite::map_sqlite_error(e, "orphan job"))?;
            Ok(())
        })
        .expect_err("a job for a nonexistent user must not insert");
}

#[test]
fn polling_an_unknown_job_is_not_found_rather_than_an_error() {
    let f = fixture();
    assert!(f.queue.find(&f.alex, JobId::new()).unwrap().is_none());
}

#[test]
fn an_unreadable_stored_payload_is_reported_as_internal_not_silently_skipped() {
    // If a corrupt row were skipped, the queue would look empty while a
    // job sat there forever. Failing loudly is the lesser evil.
    let f = fixture();
    f.queue.enqueue(&f.alex, &payload("text"), now()).unwrap();
    f.database
        .with_connection(|connection| {
            connection
                .execute("UPDATE ingest_jobs SET payload = 'not json'", [])
                .unwrap();
            Ok(())
        })
        .unwrap();

    let error = f.queue.claim_next(now()).unwrap_err();
    assert!(matches!(error, RaError::Internal(_)), "got {error:?}");
}
