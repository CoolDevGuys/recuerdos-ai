//! REST handlers for `/v1/memories`.
//!
//! Thin by design: parse, call exactly one use case, format. Every
//! handler takes a scoped extractor (`ReadAccess`/`WriteAccess`) so the
//! permission check is in the signature rather than in a line someone has
//! to remember to write.
//!
//! Every use case blocks (SQLite, tantivy, ONNX), so each handler runs
//! its call on `spawn_blocking` rather than stalling a runtime worker.

use super::dto::{
    AuditEntryResponse, AuditResponse, MemoryResponse, SaveMemoryRequest, SearchHit, SearchRequest,
    SearchResponse, UpdateMemoryRequest,
};
use crate::bootstrap::state::AppState;
use crate::identity::infrastructure::http::authenticated::{ReadAccess, WriteAccess};
use crate::memories::application::memory_exporter::ExportFormat;
use crate::memories::domain::category::Category;
use crate::memories::domain::memory::MemoryEdit;
use crate::memories::domain::recall_query::RecallQuery;
use crate::shared::blocking::blocking;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use serde::Deserialize;
use std::str::FromStr;
use std::time::Instant;

/// The client name recorded in the audit trail for REST writes.
const ACTOR: &str = "rest";

pub async fn save_memory(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Json(request): Json<SaveMemoryRequest>,
) -> Result<impl IntoResponse> {
    let memories = state.memories.clone();
    let new = request.into_new_memory(&memories.extra_categories)?;

    let memory = blocking(move || memories.saver.execute(&context, new, ACTOR)).await?;

    Ok((StatusCode::CREATED, Json(MemoryResponse::from(&memory))))
}

pub async fn search_memories(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
    Json(request): Json<SearchRequest>,
) -> Result<Json<SearchResponse>> {
    let memories = state.memories.clone();

    let categories = request
        .categories
        .iter()
        .map(|raw| Category::parse_with_extras(raw, &memories.extra_categories))
        .collect::<Result<Vec<_>>>()?;

    let mut query = RecallQuery::new(
        &request.query,
        request.limit.unwrap_or(memories.default_limit),
    )?
    .with_categories(categories)
    .with_subcategories(request.subcategories)
    .with_tags(request.tags)
    .with_since(request.since);
    if request.include_superseded {
        query = query.including_superseded();
    }

    let started = Instant::now();
    let results = blocking(move || memories.recaller.execute(&context, &query)).await?;

    Ok(Json(SearchResponse {
        results: results.iter().map(SearchHit::from).collect(),
        took_ms: started.elapsed().as_millis() as u64,
    }))
}

pub async fn get_memory(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
    Path(id): Path<String>,
) -> Result<Json<MemoryResponse>> {
    let id = parse_id(&id)?;
    let memories = state.memories.clone();

    let memory = blocking(move || memories.finder.execute(&context, id)).await?;

    Ok(Json(MemoryResponse::from(&memory)))
}

pub async fn update_memory(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Path(id): Path<String>,
    Json(request): Json<UpdateMemoryRequest>,
) -> Result<Json<MemoryResponse>> {
    let id = parse_id(&id)?;
    let memories = state.memories.clone();

    let category = request
        .category
        .as_deref()
        .map(|raw| Category::parse_with_extras(raw, &memories.extra_categories))
        .transpose()?;

    let edit = MemoryEdit {
        content: request.content,
        category,
        subcategory: request.subcategory,
        tags: request.tags,
        expires_at: request.expires_at,
    };

    let memory = blocking(move || memories.updater.execute(&context, id, edit, ACTOR)).await?;

    Ok(Json(MemoryResponse::from(&memory)))
}

pub async fn forget_memory(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let id = parse_id(&id)?;
    let memories = state.memories.clone();

    blocking(move || memories.forgetter.execute(&context, id, ACTOR, "")).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ExportParams {
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    include_inactive: bool,
}

pub async fn export_memories(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
    Query(params): Query<ExportParams>,
) -> Result<impl IntoResponse> {
    let format = match params.format.as_deref().unwrap_or("markdown") {
        "markdown" | "md" => ExportFormat::Markdown,
        "json" => ExportFormat::Json,
        other => {
            return Err(RaError::Validation(format!(
                "unknown export format {other:?} (expected markdown or json)"
            )));
        }
    };

    let memories = state.memories.clone();
    let body = blocking(move || {
        memories
            .exporter
            .execute(&context, format, params.include_inactive)
    })
    .await?;

    let content_type = match format {
        ExportFormat::Markdown => "text/markdown; charset=utf-8",
        ExportFormat::Json => "application/json",
    };

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, content_type.parse().unwrap());

    Ok((headers, body))
}

#[derive(Debug, Deserialize)]
pub struct AuditParams {
    #[serde(default)]
    limit: Option<usize>,
}

pub async fn read_audit(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
    Query(params): Query<AuditParams>,
) -> Result<Json<AuditResponse>> {
    const MAX_AUDIT_LIMIT: usize = 500;
    let limit = params.limit.unwrap_or(100).clamp(1, MAX_AUDIT_LIMIT);

    let memories = state.memories.clone();
    let entries = blocking(move || memories.repository.audit_trail(&context, limit)).await?;

    Ok(Json(AuditResponse {
        entries: entries
            .iter()
            .map(|entry| AuditEntryResponse {
                memory_id: entry.memory_id.to_string(),
                operation: entry.operation.as_str().to_string(),
                actor: entry.actor.clone(),
                detail: entry.detail.clone(),
                at: entry.at,
            })
            .collect(),
    }))
}

fn parse_id(raw: &str) -> Result<MemoryId> {
    // A malformed id is the caller's mistake, not a missing resource.
    MemoryId::from_str(raw)
        .map_err(|_| RaError::Validation(format!("{raw:?} is not a valid memory id")))
}
