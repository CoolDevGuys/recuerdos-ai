//! Tests for the worker pool.
//!
//! Driven by a scripted pipeline rather than a real one: what is under
//! test is the machinery around the work — retry, dead-lettering, crash
//! recovery, shutdown — and a real extraction would make every case slow
//! and none of them more convincing.

use super::ingest_workers::{IngestWorkers, STALE_AFTER, WorkerHandle};
use super::sqlite_job_queue::SqliteJobQueue;
use crate::identity::application::background_user_resolver::BackgroundUserResolver;
use crate::identity::domain::user_context::UserContext;
use crate::shared::clock::{Clock, SystemClock};
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use crate::shared::sqlite::SqliteDatabase;
use crate::understanding::domain::ingest_job::{IngestPayload, JobQueue, JobStatus};
use crate::understanding::domain::ingest_pipeline::IngestPipeline;
use chrono::Utc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// A pipeline that follows a script and records what it saw.
struct ScriptedPipeline {
    outcomes: Mutex<Vec<Result<Vec<MemoryId>>>>,
    seen: Mutex<Vec<(String, String)>>,
}

impl ScriptedPipeline {
    fn new(outcomes: Vec<Result<Vec<MemoryId>>>) -> Arc<Self> {
        Arc::new(Self {
            outcomes: Mutex::new(outcomes),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// `(handle, content)` for every job processed, in order.
    fn seen(&self) -> Vec<(String, String)> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl IngestPipeline for ScriptedPipeline {
    async fn execute(
        &self,
        context: &UserContext,
        payload: &IngestPayload,
    ) -> Result<Vec<MemoryId>> {
        self.seen
            .lock()
            .unwrap()
            .push((context.handle().to_string(), payload.content.clone()));

        let mut outcomes = self.outcomes.lock().unwrap();
        if outcomes.is_empty() {
            return Ok(vec![]);
        }
        outcomes.remove(0)
    }
}

struct Harness {
    queue: Arc<dyn JobQueue>,
    alex: UserContext,
    workers: WorkerHandle,
}

async fn start(pipeline: Arc<dyn IngestPipeline>, max_attempts: u32) -> Harness {
    let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
    let identity =
        crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();
    let alex = authenticate(&identity, "alex");

    let queue: Arc<dyn JobQueue> = Arc::new(SqliteJobQueue::new(Arc::clone(&database)));
    let workers = IngestWorkers {
        queue: Arc::clone(&queue),
        pipeline,
        users: Arc::new(BackgroundUserResolver::new(Arc::clone(&identity.users))),
        clock: Arc::new(SystemClock) as Arc<dyn Clock>,
        max_attempts,
        wake: Arc::new(Notify::new()),
    }
    // One worker: these assertions are about ordering and counts, and a
    // pool would make "which worker got it" a source of flake.
    .start(1)
    .await
    .unwrap();

    Harness {
        queue,
        alex,
        workers,
    }
}

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

fn payload(content: &str) -> IngestPayload {
    IngestPayload {
        content: content.to_string(),
        category: None,
        tags: vec![],
        client: None,
        session_id: None,
    }
}

/// Waits for `check` to hold, so a test asserts on an outcome rather than
/// on a sleep long enough to "probably" be safe.
async fn eventually(mut check: impl FnMut() -> bool, what: &str) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn a_job_is_picked_up_and_marked_done() {
    let produced = vec![MemoryId::new()];
    let pipeline = ScriptedPipeline::new(vec![Ok(produced.clone())]);
    let harness = start(Arc::clone(&pipeline) as Arc<dyn IngestPipeline>, 3).await;

    let id = harness
        .queue
        .enqueue(&harness.alex, &payload("I prefer pnpm"), Utc::now())
        .unwrap();

    let queue = Arc::clone(&harness.queue);
    let context = harness.alex.clone();
    eventually(
        || {
            queue
                .find(&context, id)
                .unwrap()
                .is_some_and(|job| job.status == JobStatus::Succeeded)
        },
        "the job to succeed",
    )
    .await;

    let record = harness.queue.find(&harness.alex, id).unwrap().unwrap();
    assert_eq!(record.memory_ids, produced);
    assert_eq!(
        pipeline.seen(),
        vec![("alex".to_string(), "I prefer pnpm".to_string())],
        "the pipeline must run as the job's owner, with the submitted content"
    );

    harness.workers.shutdown().await;
}

#[tokio::test]
async fn a_transient_failure_is_retried_and_then_succeeds() {
    // The provider-blip case: an ingestion must survive a 429 without a
    // user ever learning it happened.
    let pipeline = ScriptedPipeline::new(vec![
        Err(RaError::Internal("provider returned 503".to_string())),
        Ok(vec![MemoryId::new()]),
    ]);
    let harness = start(Arc::clone(&pipeline) as Arc<dyn IngestPipeline>, 5).await;

    let id = harness
        .queue
        .enqueue(&harness.alex, &payload("retry me"), Utc::now())
        .unwrap();

    // The first attempt fails and the job goes back to pending with a
    // backoff, so it is claimable again only in the future. Assert on the
    // recorded failure rather than waiting out the real delay.
    let queue = Arc::clone(&harness.queue);
    let context = harness.alex.clone();
    eventually(
        || {
            queue
                .find(&context, id)
                .unwrap()
                .is_some_and(|job| job.attempts == 1 && job.error.is_some())
        },
        "the first attempt to be recorded as failed",
    )
    .await;

    let record = harness.queue.find(&harness.alex, id).unwrap().unwrap();
    assert_eq!(record.status, JobStatus::Pending, "it must come back");
    assert!(record.error.unwrap().contains("503"));

    harness.workers.shutdown().await;
}

#[tokio::test]
async fn a_job_that_keeps_failing_dead_letters_with_its_error() {
    // `max_attempts = 1` so the first failure is also the last, keeping
    // the test off the backoff clock.
    let pipeline = ScriptedPipeline::new(vec![Err(RaError::Internal("model is down".to_string()))]);
    let harness = start(Arc::clone(&pipeline) as Arc<dyn IngestPipeline>, 1).await;

    let id = harness
        .queue
        .enqueue(&harness.alex, &payload("poison"), Utc::now())
        .unwrap();

    let queue = Arc::clone(&harness.queue);
    let context = harness.alex.clone();
    eventually(
        || {
            queue
                .find(&context, id)
                .unwrap()
                .is_some_and(|job| job.status == JobStatus::DeadLetter)
        },
        "the job to dead-letter",
    )
    .await;

    let record = harness.queue.find(&harness.alex, id).unwrap().unwrap();
    assert!(
        record.error.unwrap().contains("model is down"),
        "a dead-lettered job must say why, or the memory is lost silently"
    );

    harness.workers.shutdown().await;
}

#[tokio::test]
async fn unacceptable_content_fails_immediately_rather_than_burning_attempts() {
    // Re-running a validation failure gets the same answer three times
    // and delays the operator's error by a minute of backoff.
    let pipeline = ScriptedPipeline::new(vec![Err(RaError::Validation(
        "content is longer than 4000 characters".to_string(),
    ))]);
    let harness = start(Arc::clone(&pipeline) as Arc<dyn IngestPipeline>, 5).await;

    let id = harness
        .queue
        .enqueue(&harness.alex, &payload("too long"), Utc::now())
        .unwrap();

    let queue = Arc::clone(&harness.queue);
    let context = harness.alex.clone();
    eventually(
        || {
            queue
                .find(&context, id)
                .unwrap()
                .is_some_and(|job| job.status.is_terminal())
        },
        "the job to fail",
    )
    .await;

    let record = harness.queue.find(&harness.alex, id).unwrap().unwrap();
    assert_eq!(record.status, JobStatus::DeadLetter);
    assert_eq!(record.attempts, 1, "it should not have been retried");

    harness.workers.shutdown().await;
}

#[tokio::test]
async fn an_empty_extraction_is_success_not_failure() {
    // Small talk produces no memories. Treating that as an error would
    // dead-letter every "thanks!" a user sends.
    let pipeline = ScriptedPipeline::new(vec![Ok(vec![])]);
    let harness = start(Arc::clone(&pipeline) as Arc<dyn IngestPipeline>, 3).await;

    let id = harness
        .queue
        .enqueue(&harness.alex, &payload("thanks!"), Utc::now())
        .unwrap();

    let queue = Arc::clone(&harness.queue);
    let context = harness.alex.clone();
    eventually(
        || {
            queue
                .find(&context, id)
                .unwrap()
                .is_some_and(|job| job.status == JobStatus::Succeeded)
        },
        "the job to succeed with nothing to show",
    )
    .await;

    assert!(
        harness
            .queue
            .find(&harness.alex, id)
            .unwrap()
            .unwrap()
            .memory_ids
            .is_empty()
    );

    harness.workers.shutdown().await;
}

#[tokio::test]
async fn a_restart_resumes_work_left_pending() {
    // The promise behind a 202: a job accepted before a deploy still runs
    // after it.
    let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
    let identity =
        crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();
    let alex = authenticate(&identity, "alex");
    let queue: Arc<dyn JobQueue> = Arc::new(SqliteJobQueue::new(Arc::clone(&database)));

    // Enqueued while nothing is running.
    let id = queue
        .enqueue(&alex, &payload("survive the deploy"), Utc::now())
        .unwrap();

    let pipeline = ScriptedPipeline::new(vec![Ok(vec![MemoryId::new()])]);
    let workers = IngestWorkers {
        queue: Arc::clone(&queue),
        pipeline: Arc::clone(&pipeline) as Arc<dyn IngestPipeline>,
        users: Arc::new(BackgroundUserResolver::new(Arc::clone(&identity.users))),
        clock: Arc::new(SystemClock) as Arc<dyn Clock>,
        max_attempts: 3,
        wake: Arc::new(Notify::new()),
    }
    .start(1)
    .await
    .unwrap();

    let polling = Arc::clone(&queue);
    let context = alex.clone();
    eventually(
        || {
            polling
                .find(&context, id)
                .unwrap()
                .is_some_and(|job| job.status == JobStatus::Succeeded)
        },
        "the pre-existing job to be picked up",
    )
    .await;

    assert_eq!(pipeline.seen().len(), 1);
    workers.shutdown().await;
}

#[tokio::test]
async fn a_job_held_by_a_crashed_process_is_reclaimed_at_startup() {
    let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
    let identity =
        crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();
    let alex = authenticate(&identity, "alex");
    let queue: Arc<dyn JobQueue> = Arc::new(SqliteJobQueue::new(Arc::clone(&database)));

    // Simulate the previous process: enqueued and claimed long ago, then
    // never completed. Both timestamps have to be in the past — a job is
    // only claimable once its `run_after` has elapsed.
    let long_ago = Utc::now() - STALE_AFTER - chrono::Duration::minutes(5);
    let id = queue
        .enqueue(&alex, &payload("orphaned"), long_ago)
        .unwrap();
    queue
        .claim_next(long_ago)
        .unwrap()
        .expect("the job to claim");
    assert_eq!(
        queue.find(&alex, id).unwrap().unwrap().status,
        JobStatus::Running,
        "precondition: the job looks held"
    );

    let pipeline = ScriptedPipeline::new(vec![Ok(vec![MemoryId::new()])]);
    let workers = IngestWorkers {
        queue: Arc::clone(&queue),
        pipeline: Arc::clone(&pipeline) as Arc<dyn IngestPipeline>,
        users: Arc::new(BackgroundUserResolver::new(Arc::clone(&identity.users))),
        clock: Arc::new(SystemClock) as Arc<dyn Clock>,
        max_attempts: 3,
        wake: Arc::new(Notify::new()),
    }
    .start(1)
    .await
    .unwrap();

    let polling = Arc::clone(&queue);
    let context = alex.clone();
    eventually(
        || {
            polling
                .find(&context, id)
                .unwrap()
                .is_some_and(|job| job.status == JobStatus::Succeeded)
        },
        "the orphaned job to be reclaimed and run",
    )
    .await;

    workers.shutdown().await;
}

#[tokio::test]
async fn shutdown_stops_the_pool() {
    let pipeline = ScriptedPipeline::new(vec![]);
    let harness = start(Arc::clone(&pipeline) as Arc<dyn IngestPipeline>, 3).await;

    // Returns rather than hanging: the tasks must actually observe the
    // signal, not sit forever in their select.
    tokio::time::timeout(Duration::from_secs(10), harness.workers.shutdown())
        .await
        .expect("workers did not stop");
}
