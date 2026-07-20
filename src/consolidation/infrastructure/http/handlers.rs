//! REST handlers for the consolidation context.

use super::dto::{DistillRequest, DistillResponse};
use crate::bootstrap::state::AppState;
use crate::consolidation::domain::distillation::SessionTranscript;
use crate::identity::infrastructure::http::authenticated::WriteAccess;
use crate::shared::error::Result;
use axum::Json;
use axum::extract::State;

/// `POST /v1/sessions/distill` — reduce a finished session to what
/// outlives it.
pub async fn distill_session(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Json(request): Json<DistillRequest>,
) -> Result<Json<DistillResponse>> {
    let transcript = SessionTranscript::new(request.content)?
        .from(request.client, request.session_id)
        .tagged(request.tags);

    let distillation = state
        .consolidation
        .session_distiller
        .execute(&context, &transcript)
        .await?;

    Ok(Json(DistillResponse::from(&distillation)))
}
