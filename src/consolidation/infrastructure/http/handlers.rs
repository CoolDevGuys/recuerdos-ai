//! REST handlers for the consolidation context.

use super::dto::{DistillRequest, DistillResponse};
use crate::bootstrap::state::AppState;
use crate::consolidation::domain::distillation::SessionTranscript;
use crate::identity::infrastructure::http::authenticated::{ReadAccess, WriteAccess};
use crate::shared::error::Result;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;

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

/// `GET /v1/profile` — the same briefing the MCP `memory://profile`
/// resource returns.
///
/// Lives in this context rather than in `memories` because consolidation
/// is what writes it now: `ProfileDigestWriter` generates and caches the
/// digest, falling back to the memories context's assembler when there
/// is no model. The route and the media type are unchanged, so no client
/// — including the stdio shim, which reads the profile from here —
/// notices the move.
pub async fn read_profile(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
) -> Result<impl IntoResponse> {
    let profile = state
        .consolidation
        .profile_digest_writer
        .execute(&context)
        .await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "text/markdown; charset=utf-8".parse().unwrap(),
    );
    Ok((headers, profile))
}
