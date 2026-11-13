//! REST handlers for the consolidation context.

use super::dto::{DistillRequest, DistillResponse};
use crate::bootstrap::state::AppState;
use crate::consolidation::domain::distillation::SessionTranscript;
use crate::identity::infrastructure::http::authenticated::{ReadAccess, WriteAccess};
use crate::memories::application::structured_memory_saver::DetectedProject;
use crate::shared::blocking::blocking;
use crate::shared::error::Result;
use crate::understanding::domain::ingest_job::ProjectHint;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;

/// `POST /v1/sessions/distill` — reduce a finished session to what
/// outlives it.
pub async fn distill_session(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    headers: HeaderMap,
    Json(request): Json<DistillRequest>,
) -> Result<Json<DistillResponse>> {
    let mut transcript = SessionTranscript::new(request.content)?
        .from(request.client, request.session_id)
        .tagged(request.tags);
    transcript.project = crate::shared::project_headers::project_from(&headers)
        .map(|(name, alias)| ProjectHint { name, alias });

    let distillation = state
        .consolidation
        .session_distiller
        .execute(&context, &transcript)
        .await?;

    Ok(Json(DistillResponse::from(&distillation)))
}

#[derive(Debug, serde::Deserialize)]
pub struct ProfileParams {
    #[serde(default)]
    project: Option<String>,
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
    request_headers: HeaderMap,
    Query(params): Query<ProfileParams>,
) -> Result<impl IntoResponse> {
    // The project the caller works in (memory-structure-plan.md §9): named
    // in `?project=`, or the folder the local MCP server sends. Its
    // memories follow the user's own; other projects' are left out. A
    // folder no project is known by simply has none.
    let detected = crate::shared::project_headers::project_from(&request_headers)
        .map(|(name, alias)| DetectedProject { name, alias });
    let memories = state.memories.clone();
    let lookup = context.clone();
    let project = blocking(move || {
        let curator = &memories.project_curator;
        match (params.project, detected) {
            (Some(named), _) => curator.find(&lookup, &named).map(Some),
            (None, Some(detected)) => curator.locate(&lookup, &detected),
            (None, None) => Ok(None),
        }
    })
    .await?;

    let profile = state
        .consolidation
        .profile_digest_writer
        .execute_in(&context, project.as_ref())
        .await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "text/markdown; charset=utf-8".parse().unwrap(),
    );
    Ok((headers, profile))
}
