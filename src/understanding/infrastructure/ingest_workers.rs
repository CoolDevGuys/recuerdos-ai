//! The worker pool that drains the ingest queue.
//!
//! # Waking up
//!
//! Workers wait on a [`Notify`] that the enqueue path pings, so a
//! submission is normally picked up within microseconds rather than
//! after a poll interval. The poll interval still exists as a backstop:
//! a notify delivered while every worker was busy is not queued, and
//! without the timeout that job would sit until the *next* submission
//! happened to arrive. Notification for latency, polling for correctness.
//!
//! # Blocking work on an async runtime
//!
//! Claiming and completing a job are synchronous SQLite calls, so they go
//! through `spawn_blocking`. Running them inline would stall a runtime
//! worker thread and, with enough concurrency, deadlock the HTTP server
//! that shares it.

use crate::identity::application::background_user_resolver::BackgroundUserResolver;
use crate::shared::clock::Clock;
use crate::shared::error::Result;
use crate::understanding::domain::ingest_job::{ClaimedJob, JobQueue};
use crate::understanding::domain::ingest_pipeline::{IngestPipeline, is_retryable};
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
use tokio::sync::{Notify, watch};

/// How long a job may be held before it is assumed the worker died.
///
/// Comfortably longer than the slowest plausible job: a local model on
/// CPU can take minutes, and reclaiming a job that is merely slow would
/// run it twice.
pub const STALE_AFTER: Duration = Duration::minutes(15);

/// The backstop poll, for the case where a notify arrived while every
/// worker was busy.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// First retry delay; doubles per attempt (5s, 10s, 20s…).
const BASE_BACKOFF_SECONDS: i64 = 5;

/// Everything the pool needs, gathered so `start` does not take eight
/// positional arguments.
pub struct IngestWorkers {
    pub queue: Arc<dyn JobQueue>,
    pub pipeline: Arc<dyn IngestPipeline>,
    pub users: Arc<BackgroundUserResolver>,
    pub clock: Arc<dyn Clock>,
    /// Attempts before a job dead-letters.
    pub max_attempts: u32,
    /// Pinged by the enqueue path so a new job is picked up immediately.
    pub wake: Arc<Notify>,
}

/// Stops the pool and waits for in-flight jobs to finish.
pub struct WorkerHandle {
    shutdown: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl WorkerHandle {
    /// Signals shutdown and waits.
    ///
    /// Workers finish the job they hold rather than abandoning it: a job
    /// dropped mid-flight would be reclaimed and re-run, and re-running
    /// an extraction that already wrote memories duplicates them.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        for task in self.tasks {
            let _ = task.await;
        }
    }
}

impl IngestWorkers {
    /// Reclaims anything a previous process was holding, then starts
    /// `count` workers.
    ///
    /// The reclaim happens before any worker starts so that a job the
    /// last process died on is picked up now rather than sitting until
    /// something else triggers a sweep.
    pub async fn start(self, count: usize) -> Result<WorkerHandle> {
        let now = self.clock.now();
        let queue = Arc::clone(&self.queue);
        let reclaimed =
            tokio::task::spawn_blocking(move || queue.reclaim_stale(now - STALE_AFTER, now))
                .await
                .map_err(|e| {
                    crate::shared::error::RaError::Internal(format!("reclaim task panicked: {e}"))
                })??;

        if reclaimed > 0 {
            tracing::info!(
                reclaimed,
                "requeued ingest jobs left running by a previous process"
            );
        }

        let (shutdown, _) = watch::channel(false);
        let shared = Arc::new(self);
        let mut tasks = Vec::with_capacity(count);

        for index in 0..count {
            let worker = Arc::clone(&shared);
            let mut stop = shutdown.subscribe();
            tasks.push(tokio::spawn(async move {
                worker.run(index, &mut stop).await;
            }));
        }

        tracing::info!(workers = count, "ingest workers started");
        Ok(WorkerHandle { shutdown, tasks })
    }

    async fn run(&self, index: usize, stop: &mut watch::Receiver<bool>) {
        loop {
            if *stop.borrow() {
                return;
            }

            match self.claim().await {
                Ok(Some(job)) => {
                    self.process(job).await;
                    // Straight back round: a burst should drain without
                    // waiting on the notify between each job.
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    // The queue itself is unreachable. Backing off avoids
                    // a hot loop of failing queries against a database
                    // that is, say, locked by a backup.
                    tracing::error!(worker = index, %error, "could not claim an ingest job");
                    tokio::time::sleep(POLL_INTERVAL).await;
                    continue;
                }
            }

            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
                _ = stop.changed() => return,
            }
        }
    }

    async fn claim(&self) -> Result<Option<ClaimedJob>> {
        let queue = Arc::clone(&self.queue);
        let now = self.clock.now();
        tokio::task::spawn_blocking(move || queue.claim_next(now))
            .await
            .map_err(|e| {
                crate::shared::error::RaError::Internal(format!("claim task panicked: {e}"))
            })?
    }

    async fn process(&self, job: ClaimedJob) {
        let id = job.id;

        // Resolving the user is itself fallible — the account may have
        // been deleted since the job was enqueued — and that is not
        // worth retrying.
        let users = Arc::clone(&self.users);
        let user_id = job.user_id;
        let context = match tokio::task::spawn_blocking(move || users.execute(user_id)).await {
            Ok(Ok(context)) => context,
            Ok(Err(error)) => {
                self.record_failure(&job, &error.to_string(), false).await;
                return;
            }
            Err(error) => {
                self.record_failure(&job, &format!("resolver task panicked: {error}"), true)
                    .await;
                return;
            }
        };

        match self.pipeline.execute(&context, &job.payload).await {
            Ok(memory_ids) => {
                tracing::info!(
                    job = %id,
                    memories = memory_ids.len(),
                    "ingest job finished"
                );
                let queue = Arc::clone(&self.queue);
                let now = self.clock.now();
                let produced = memory_ids.clone();
                let outcome =
                    tokio::task::spawn_blocking(move || queue.succeed(id, &produced, now)).await;

                if let Some(error) = failure_text(outcome) {
                    // The memories exist but the job still reads as
                    // running. It will be reclaimed and re-run, which
                    // duplicates them — worth a loud log, and the reason
                    // reconciliation treats duplicates as NOOP.
                    tracing::error!(
                        job = %id,
                        %error,
                        "ingest job succeeded but could not be marked done"
                    );
                }
            }
            Err(error) => {
                let retryable = is_retryable(&error);
                self.record_failure(&job, &error.to_string(), retryable)
                    .await;
            }
        }
    }

    async fn record_failure(&self, job: &ClaimedJob, message: &str, retryable: bool) {
        let now = self.clock.now();
        let attempts_left = job.attempts < self.max_attempts;
        let retry_after = (retryable && attempts_left).then(|| backoff_from(now, job.attempts));

        if retry_after.is_none() {
            tracing::error!(
                job = %job.id,
                attempts = job.attempts,
                retryable,
                error = message,
                "ingest job dead-lettered"
            );
        } else {
            tracing::warn!(
                job = %job.id,
                attempts = job.attempts,
                error = message,
                "ingest job failed; will retry"
            );
        }

        let queue = Arc::clone(&self.queue);
        let id = job.id;
        let message = message.to_string();
        let outcome =
            tokio::task::spawn_blocking(move || queue.fail(id, &message, retry_after, now)).await;

        if let Some(error) = failure_text(outcome) {
            tracing::error!(
                job = %job.id,
                %error,
                "could not record an ingest job failure"
            );
        }
    }
}

/// Flattens "the task panicked" and "the task returned an error" into one
/// optional message, so callers have a single thing to log.
fn failure_text<T>(
    outcome: std::result::Result<Result<T>, tokio::task::JoinError>,
) -> Option<String> {
    match outcome {
        Ok(Ok(_)) => None,
        Ok(Err(error)) => Some(error.to_string()),
        Err(error) => Some(format!("task panicked: {error}")),
    }
}

/// 5s, 10s, 20s… — long enough for a rate-limit window to pass, short
/// enough that a transient blip does not delay a memory by minutes.
fn backoff_from(now: DateTime<Utc>, attempts: u32) -> DateTime<Utc> {
    let exponent = attempts.saturating_sub(1).min(6);
    now + Duration::seconds(BASE_BACKOFF_SECONDS * (1_i64 << exponent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_per_attempt() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        assert_eq!(backoff_from(now, 1), now + Duration::seconds(5));
        assert_eq!(backoff_from(now, 2), now + Duration::seconds(10));
        assert_eq!(backoff_from(now, 3), now + Duration::seconds(20));
    }

    #[test]
    fn backoff_is_capped_so_a_long_lived_job_does_not_wait_for_hours() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        assert_eq!(
            backoff_from(now, 50),
            now + Duration::seconds(BASE_BACKOFF_SECONDS * 64)
        );
    }
}
