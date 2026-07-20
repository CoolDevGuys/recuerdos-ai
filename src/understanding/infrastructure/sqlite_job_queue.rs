//! `JobQueue` backed by SQLite.
//!
//! # Claiming
//!
//! `UPDATE … WHERE id = (SELECT … LIMIT 1) RETURNING …` in one statement.
//! The obvious alternative — select a candidate, then update it — has a
//! window in which a second worker selects the same row, and the result
//! is the same content extracted twice and duplicate memories written.
//! SQLite serialises writers, so the single statement is atomic against
//! every other worker in the process and every other process on the file.

use crate::identity::domain::user_context::UserContext;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{JobId, MemoryId, UserId};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error, optional};
use crate::understanding::domain::ingest_job::{
    ClaimedJob, IngestPayload, JobQueue, JobRecord, JobStatus,
};
use chrono::{DateTime, Utc};
use std::str::FromStr;
use std::sync::Arc;

pub struct SqliteJobQueue {
    database: Arc<SqliteDatabase>,
}

impl SqliteJobQueue {
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }
}

impl JobQueue for SqliteJobQueue {
    fn enqueue(
        &self,
        context: &UserContext,
        payload: &IngestPayload,
        now: DateTime<Utc>,
    ) -> Result<JobId> {
        let id = JobId::new();
        let encoded = serde_json::to_string(payload)
            .map_err(|e| RaError::Internal(format!("failed to encode the job payload: {e}")))?;

        self.database.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO ingest_jobs
                         (id, user_id, payload, status, attempts, memory_ids,
                          created_at, updated_at, run_after)
                     VALUES (?1, ?2, ?3, 'pending', 0, '[]', ?4, ?4, ?4)",
                    rusqlite::params![
                        id.to_string(),
                        context.user_id().to_string(),
                        encoded,
                        now.to_rfc3339(),
                    ],
                )
                .map_err(|e| map_sqlite_error(e, "the job's user no longer exists"))?;
            Ok(())
        })?;

        Ok(id)
    }

    fn claim_next(&self, now: DateTime<Utc>) -> Result<Option<ClaimedJob>> {
        let timestamp = now.to_rfc3339();

        self.database.with_connection(|connection| {
            optional(connection.query_row(
                // Oldest claimable first, so a burst is drained in the
                // order it arrived rather than newest-wins.
                "UPDATE ingest_jobs
                    SET status = 'running',
                        attempts = attempts + 1,
                        claimed_at = ?1,
                        updated_at = ?1
                  WHERE id = (
                      SELECT id FROM ingest_jobs
                       WHERE status = 'pending' AND run_after <= ?1
                       ORDER BY run_after, created_at
                       LIMIT 1
                  )
              RETURNING id, user_id, payload, attempts",
                rusqlite::params![timestamp],
                |row| {
                    Ok(claimed_from_row(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            ))
        })
    }

    fn succeed(&self, id: JobId, memory_ids: &[MemoryId], now: DateTime<Utc>) -> Result<()> {
        let encoded = serde_json::to_string(
            &memory_ids
                .iter()
                .map(MemoryId::to_string)
                .collect::<Vec<_>>(),
        )
        .map_err(|e| RaError::Internal(format!("failed to encode job results: {e}")))?;

        self.database.with_connection(|connection| {
            connection
                .execute(
                    "UPDATE ingest_jobs
                        SET status = 'succeeded',
                            memory_ids = ?2,
                            claimed_at = NULL,
                            updated_at = ?3
                      WHERE id = ?1",
                    rusqlite::params![id.to_string(), encoded, now.to_rfc3339()],
                )
                .map_err(|e| map_sqlite_error(e, "job update conflict"))?;
            Ok(())
        })
    }

    fn fail(
        &self,
        id: JobId,
        error: &str,
        retry_after: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let timestamp = now.to_rfc3339();

        self.database.with_connection(|connection| {
            match retry_after {
                Some(when) => connection.execute(
                    "UPDATE ingest_jobs
                        SET status = 'pending',
                            error = ?2,
                            run_after = ?3,
                            claimed_at = NULL,
                            updated_at = ?4
                      WHERE id = ?1",
                    rusqlite::params![id.to_string(), error, when.to_rfc3339(), timestamp],
                ),
                None => connection.execute(
                    "UPDATE ingest_jobs
                        SET status = 'dead_letter',
                            error = ?2,
                            claimed_at = NULL,
                            updated_at = ?3
                      WHERE id = ?1",
                    rusqlite::params![id.to_string(), error, timestamp],
                ),
            }
            .map_err(|e| map_sqlite_error(e, "job update conflict"))?;
            Ok(())
        })
    }

    fn reclaim_stale(&self, stale_before: DateTime<Utc>, now: DateTime<Utc>) -> Result<usize> {
        self.database.with_connection(|connection| {
            let reclaimed = connection
                .execute(
                    // `run_after = ?2` rather than leaving the old value:
                    // a reclaimed job should be retried now, not held
                    // behind a backoff that was set for a different
                    // failure.
                    "UPDATE ingest_jobs
                        SET status = 'pending',
                            claimed_at = NULL,
                            run_after = ?2,
                            updated_at = ?2,
                            error = COALESCE(error, 'the worker holding this job stopped')
                      WHERE status = 'running' AND claimed_at <= ?1",
                    rusqlite::params![stale_before.to_rfc3339(), now.to_rfc3339()],
                )
                .map_err(|e| map_sqlite_error(e, "job reclaim conflict"))?;
            Ok(reclaimed)
        })
    }

    fn find(&self, context: &UserContext, id: JobId) -> Result<Option<JobRecord>> {
        self.database.with_connection(|connection| {
            optional(connection.query_row(
                // `user_id = ?2` is the isolation: polling someone else's
                // job id reads as "no such job", not as their status.
                "SELECT id, status, attempts, error, memory_ids, created_at, updated_at
                   FROM ingest_jobs
                  WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id.to_string(), context.user_id().to_string()],
                |row| {
                    Ok(record_from_row(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            ))
        })
    }
}

fn claimed_from_row(
    id: String,
    user_id: String,
    payload: String,
    attempts: i64,
) -> Result<ClaimedJob> {
    Ok(ClaimedJob {
        id: JobId::from_str(&id)
            .map_err(|e| RaError::Internal(format!("stored job id {id:?} is not a uuid: {e}")))?,
        user_id: UserId::from_str(&user_id).map_err(|e| {
            RaError::Internal(format!("stored user id {user_id:?} is not a uuid: {e}"))
        })?,
        payload: serde_json::from_str(&payload)
            .map_err(|e| RaError::Internal(format!("stored job payload is not readable: {e}")))?,
        attempts: attempts.max(0) as u32,
    })
}

#[allow(clippy::too_many_arguments)]
fn record_from_row(
    id: String,
    status: String,
    attempts: i64,
    error: Option<String>,
    memory_ids: String,
    created_at: String,
    updated_at: String,
) -> Result<JobRecord> {
    let ids: Vec<String> = serde_json::from_str(&memory_ids)
        .map_err(|e| RaError::Internal(format!("stored job results are not readable: {e}")))?;

    Ok(JobRecord {
        id: JobId::from_str(&id)
            .map_err(|e| RaError::Internal(format!("stored job id {id:?} is not a uuid: {e}")))?,
        status: JobStatus::from_stored(&status),
        attempts: attempts.max(0) as u32,
        error,
        memory_ids: ids
            .iter()
            .filter_map(|raw| MemoryId::from_str(raw).ok())
            .collect(),
        created_at: parse_time(&created_at)?,
        updated_at: parse_time(&updated_at)?,
    })
}

fn parse_time(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|e| RaError::Internal(format!("stored timestamp {raw:?} is not readable: {e}")))
}
