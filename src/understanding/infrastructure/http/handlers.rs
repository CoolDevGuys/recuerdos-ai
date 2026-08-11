//! REST handlers for ingestion and job polling.

use super::dto::{
    AcceptedResponse, BatchAcceptedResponse, BatchIngestRequest, BatchIngestedResponse,
    BatchItemResult, IngestRequest, IngestedResponse, JobResponse, status_name,
};
use crate::bootstrap::state::AppState;
use crate::identity::domain::user_context::UserContext;
use crate::identity::infrastructure::http::authenticated::{ReadAccess, WriteAccess};
use crate::shared::blocking::blocking;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{JobId, MemoryId};
use crate::understanding::domain::ingest_job::{IngestPayload, JobStatus};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::str::FromStr;

/// `POST /v1/memories` — submit raw content for understanding.
///
/// Returns `202` with a job id. The work is an LLM pipeline that takes
/// seconds; holding the request open for it would make every client's
/// timeout our problem and lose the work on a disconnect.
///
/// `wait: true` runs it inline instead, for callers with nowhere to put a
/// job id.
pub async fn ingest(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Json(request): Json<IngestRequest>,
) -> Result<axum::response::Response> {
    if request.content.trim().is_empty() {
        return Err(RaError::Validation("content is empty".to_string()));
    }

    let wait = request.wait;
    let payload: IngestPayload = request.into();

    if !wait {
        let job_id = enqueue_only(&state, &context, &payload).await?;
        state.understanding.wake.notify_one();
        return Ok((
            StatusCode::ACCEPTED,
            Json(AcceptedResponse {
                job_id: job_id.to_string(),
                status: status_name(JobStatus::Pending),
                poll: format!("/v1/jobs/{job_id}"),
            }),
        )
            .into_response());
    }

    let (job_id, outcome) = ingest_inline(&state, &context, payload).await?;
    match outcome {
        Ok(memory_ids) => Ok((
            StatusCode::CREATED,
            Json(IngestedResponse {
                job_id: job_id.to_string(),
                status: status_name(JobStatus::Succeeded),
                memory_ids: memory_ids.iter().map(MemoryId::to_string).collect(),
                understanding: state.understanding.enabled,
            }),
        )
            .into_response()),
        // No retry: the caller is waiting and will decide for themselves
        // whether to try again. `ingest_inline` has already dead-lettered
        // the job so a worker will not pick up work nobody will read.
        Err(error) => Err(error),
    }
}

/// How many items one batch may carry.
///
/// A ceiling on the work a single request commits to: a `wait: true` batch
/// runs the whole pipeline for every item inline, and an unbounded batch
/// would be an unbounded request. Callers with more should send several
/// batches, or submit `wait: false` and poll.
const MAX_BATCH_ITEMS: usize = 100;

/// `POST /v1/memories/batch` — submit several pieces of content at once.
///
/// The single-request answer to "the agent has five things to remember":
/// one round trip instead of five, and — for `wait: true` — the daemon
/// runs them one at a time so a batch paces the pipeline instead of five
/// concurrent saves stampeding one model. `wait: false` enqueues them all
/// and hands back a job id per item to poll.
pub async fn ingest_batch(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Json(request): Json<BatchIngestRequest>,
) -> Result<axum::response::Response> {
    if request.items.is_empty() {
        return Err(RaError::Validation("items is empty".to_string()));
    }
    if request.items.len() > MAX_BATCH_ITEMS {
        return Err(RaError::Validation(format!(
            "a batch may carry at most {MAX_BATCH_ITEMS} items, got {}",
            request.items.len()
        )));
    }
    if let Some(index) = request
        .items
        .iter()
        .position(|item| item.content.trim().is_empty())
    {
        return Err(RaError::Validation(format!(
            "item {index} has empty content"
        )));
    }

    let wait = request.wait;
    let client = request.client;
    let payloads: Vec<IngestPayload> = request
        .items
        .into_iter()
        .map(|item| item.into_payload(client.clone()))
        .collect();

    if !wait {
        let mut jobs = Vec::with_capacity(payloads.len());
        for payload in &payloads {
            let job_id = enqueue_only(&state, &context, payload).await?;
            // One wake per item: with several workers, this hands the
            // batch out in parallel rather than draining it one worker at
            // a time.
            state.understanding.wake.notify_one();
            jobs.push(AcceptedResponse {
                job_id: job_id.to_string(),
                status: status_name(JobStatus::Pending),
                poll: format!("/v1/jobs/{job_id}"),
            });
        }
        return Ok((StatusCode::ACCEPTED, Json(BatchAcceptedResponse { jobs })).into_response());
    }

    // Sequential on purpose: hit one model one item at a time. A failed
    // item is recorded in place and the rest still run — one piece of
    // content the model chokes on must not discard the others' memories.
    let mut results = Vec::with_capacity(payloads.len());
    for payload in payloads {
        let (job_id, outcome) = ingest_inline(&state, &context, payload).await?;
        results.push(match outcome {
            Ok(memory_ids) => BatchItemResult {
                job_id: job_id.to_string(),
                status: status_name(JobStatus::Succeeded),
                memory_ids: memory_ids.iter().map(MemoryId::to_string).collect(),
                error: None,
            },
            Err(error) => BatchItemResult {
                job_id: job_id.to_string(),
                status: status_name(JobStatus::DeadLetter),
                memory_ids: Vec::new(),
                error: Some(error.to_string()),
            },
        });
    }

    Ok((
        StatusCode::CREATED,
        Json(BatchIngestedResponse {
            understanding: state.understanding.enabled,
            results,
        }),
    )
        .into_response())
}

/// Writes a job row for `payload` and returns its id, without running it.
/// The audit record exists either way, so the story of what was submitted
/// does not depend on which flag the caller used.
async fn enqueue_only(
    state: &AppState,
    context: &UserContext,
    payload: &IngestPayload,
) -> Result<JobId> {
    let queue = state.understanding.queue.clone();
    let now = state.identity.clock.now();
    let (context, payload) = (context.clone(), payload.clone());
    blocking(move || queue.enqueue(&context, &payload, now)).await
}

/// Enqueues `payload` and runs it through the pipeline now, returning the
/// job id and what the pipeline produced (the inner `Result`) or the error
/// it failed with (already recorded as a dead-letter). The outer `Result`
/// is for the infrastructure failures — an unreachable queue — that should
/// abort the whole request rather than be reported as one item's outcome.
///
/// Factored out of both `ingest` and `ingest_batch` so a batch item is
/// processed exactly as a lone `wait: true` save would be.
async fn ingest_inline(
    state: &AppState,
    context: &UserContext,
    payload: IngestPayload,
) -> Result<(JobId, Result<Vec<MemoryId>>)> {
    let job_id = enqueue_only(state, context, &payload).await?;

    // Claim to lock: flips this job to running so a worker cannot pick it
    // up and run the same content in parallel. The claimed job's own
    // identity is unused — when the queue is otherwise idle, which is the
    // wait path's case, the next pending job is this one, and that is all
    // the lock needs.
    let queue = state.understanding.queue.clone();
    let claim_now = state.identity.clock.now();
    blocking(move || queue.claim_next(claim_now)).await?;

    let outcome = state
        .understanding
        .pipeline
        .execute(context, &payload)
        .await;
    let queue = state.understanding.queue.clone();
    let finish_now = state.identity.clock.now();

    match outcome {
        Ok(memory_ids) => {
            let recorded = memory_ids.clone();
            blocking(move || queue.succeed(job_id, &recorded, finish_now)).await?;
            Ok((job_id, Ok(memory_ids)))
        }
        Err(error) => {
            let message = error.to_string();
            blocking(move || queue.fail(job_id, &message, None, finish_now)).await?;
            Ok((job_id, Err(error)))
        }
    }
}

/// `GET /v1/jobs/{id}` — how an ingestion is going.
pub async fn get_job(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
    Path(id): Path<String>,
) -> Result<Json<JobResponse>> {
    let id = JobId::from_str(&id).map_err(|_| RaError::NotFound(format!("job {id} not found")))?;

    let queue = state.understanding.queue.clone();
    let record = blocking(move || queue.find(&context, id))
        .await?
        .ok_or_else(|| RaError::NotFound(format!("job {id} not found")))?;

    Ok(Json(JobResponse::from(&record)))
}
