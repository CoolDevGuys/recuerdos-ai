//! REST handlers for ingestion and job polling.

use super::dto::{AcceptedResponse, IngestRequest, IngestedResponse, JobResponse, status_name};
use crate::bootstrap::state::AppState;
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
    let understanding = state.understanding.clone();

    // The job row is written either way. A synchronous ingestion still
    // leaves a record of what was submitted and what came of it, so the
    // audit story does not depend on which flag the caller used.
    let queue = understanding.queue.clone();
    let (enqueue_context, enqueue_payload) = (context.clone(), payload.clone());
    let now = state.identity.clock.now();
    let job_id = blocking(move || queue.enqueue(&enqueue_context, &enqueue_payload, now)).await?;

    if !wait {
        understanding.wake.notify_one();
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

    // Claim it ourselves so a worker cannot pick it up in parallel and
    // run the same content twice.
    let queue = understanding.queue.clone();
    let claim_now = state.identity.clock.now();
    blocking(move || queue.claim_next(claim_now)).await?;

    let outcome = understanding.pipeline.execute(&context, &payload).await;
    let queue = understanding.queue.clone();
    let finish_now = state.identity.clock.now();

    match outcome {
        Ok(memory_ids) => {
            let recorded = memory_ids.clone();
            blocking(move || queue.succeed(job_id, &recorded, finish_now)).await?;

            Ok((
                StatusCode::CREATED,
                Json(IngestedResponse {
                    job_id: job_id.to_string(),
                    status: status_name(JobStatus::Succeeded),
                    memory_ids: memory_ids.iter().map(MemoryId::to_string).collect(),
                    understanding: understanding.enabled,
                }),
            )
                .into_response())
        }
        Err(error) => {
            // No retry: the caller is waiting and will decide for
            // themselves whether to try again. Dead-lettering keeps a
            // worker from picking up work whose result nobody will read.
            let message = error.to_string();
            blocking(move || queue.fail(job_id, &message, None, finish_now)).await?;
            Err(error)
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
