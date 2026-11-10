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
    AuditEntryResponse, AuditResponse, CategoriesResponse, CategoryResponse, MemoryResponse,
    RerankResponse, SaveMemoryRequest, SearchHit, SearchRequest, SearchResponse,
    UpdateMemoryRequest,
};
use crate::bootstrap::state::AppState;
use crate::identity::infrastructure::http::authenticated::{ReadAccess, WriteAccess};
use crate::memories::application::memory_exporter::ExportFormat;
use crate::memories::domain::category::{Category, DEFAULT_CATEGORIES};
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory::MemoryEdit;
use crate::memories::domain::recall_query::RecallQuery;
use crate::shared::blocking::blocking;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;
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

    // The default floor exists for a caller who can only describe what they
    // want in prose: without it, a weak similarity and a strong one look
    // identical and unrelated memories arrive dressed as matches. A request
    // that names a category, subcategory or tag has already said what it wants
    // in the store's own vocabulary, so its prose is a preference *inside*
    // that slice rather than the definition of it — holding the floor there
    // would turn "what decisions have I stored?" into an empty answer merely
    // because the wording was a poor vector match.
    //
    // An explicit number is never second-guessed, filtered or not.
    let structurally_filtered = !request.categories.is_empty()
        || !request.subcategories.is_empty()
        || !request.tags.is_empty();
    let min_relevance = match request.min_relevance {
        Some(explicit) => explicit,
        None if structurally_filtered => 0.0,
        None => memories.min_relevance,
    }
    .clamp(0.0, 1.0);

    let mut query = RecallQuery::new(
        &request.query,
        request.limit.unwrap_or(memories.default_limit as i64),
    )?
    .with_categories(categories)
    .with_subcategories(request.subcategories)
    .with_tags(request.tags)
    .with_since(request.since)
    .with_as_of(request.as_of)
    .with_min_relevance(min_relevance);
    if request.include_superseded {
        query = query.including_superseded();
    }

    let started = Instant::now();
    // Traced, so a second stage that was configured and did not run is
    // visible in the response rather than only in the daemon's log.
    let report = blocking(move || memories.recaller.execute_traced(&context, &query)).await?;

    Ok(Json(SearchResponse {
        results: report.results.iter().map(SearchHit::from).collect(),
        took_ms: started.elapsed().as_millis() as u64,
        min_relevance,
        rerank: report.rerank.map(RerankResponse::from),
    }))
}

pub async fn get_memory(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
    Path(id): Path<String>,
) -> Result<impl IntoResponse> {
    let id = parse_id(&id)?;
    let memories = state.memories.clone();

    let memory = blocking(move || memories.finder.execute(&context, id)).await?;

    Ok(with_etag(&memory))
}

pub async fn update_memory(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateMemoryRequest>,
) -> Result<impl IntoResponse> {
    let id = parse_id(&id)?;
    let expected_updated_at = if_match(&headers)?;
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
        expected_updated_at,
    };

    let memory = blocking(move || memories.updater.execute(&context, id, edit, ACTOR)).await?;

    Ok(with_etag(&memory))
}

/// Brings back a memory forgotten with `DELETE` — the web UI's undo.
pub async fn restore_memory(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Path(id): Path<String>,
) -> Result<impl IntoResponse> {
    let id = parse_id(&id)?;
    let memories = state.memories.clone();

    let memory = blocking(move || memories.restorer.execute(&context, id, ACTOR)).await?;

    Ok(with_etag(&memory))
}

/// The categories a memory can be filed under, with counts.
///
/// Built-ins first in taxonomy order (even when empty: they are always
/// valid), then the configured extras, then any category still on a stored
/// memory but no longer configured — a row written when the taxonomy was
/// wider is still the user's, and they need to see it to move it.
pub async fn list_categories(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
) -> Result<Json<CategoriesResponse>> {
    let memories = state.memories.clone();
    let extras = memories.extra_categories.clone();
    let active = blocking(move || memories.repository.list(&context, false)).await?;

    Ok(Json(CategoriesResponse {
        categories: categories_with_counts(&active, &extras),
    }))
}

fn categories_with_counts(memories: &[Memory], extras: &[String]) -> Vec<CategoryResponse> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for memory in memories {
        *counts
            .entry(memory.category().as_str().to_string())
            .or_default() += 1;
    }

    let mut categories: Vec<CategoryResponse> = DEFAULT_CATEGORIES
        .iter()
        .map(|category| CategoryResponse {
            id: category.as_str().to_string(),
            label: category.label(),
            built_in: true,
            count: counts.remove(category.as_str()).unwrap_or(0),
        })
        .collect();

    let mut custom: BTreeMap<String, usize> = extras
        .iter()
        .map(|extra| (extra.trim().to_ascii_lowercase(), 0))
        .collect();
    for (id, count) in counts {
        *custom.entry(id).or_default() += count;
    }
    categories.extend(custom.into_iter().map(|(id, count)| CategoryResponse {
        label: Category::Custom(id.clone()).label(),
        id,
        built_in: false,
        count,
    }));

    categories
}

/// The memory, with its version as an `ETag` for a later `If-Match`.
fn with_etag(memory: &Memory) -> impl IntoResponse + use<> {
    let tag = format!("\"{}\"", version_of(memory.updated_at()));
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&tag) {
        headers.insert(header::ETAG, value);
    }
    (headers, Json(MemoryResponse::from(memory)))
}

/// A version is the memory's `updated_at`, spelled the way the JSON body
/// spells it, so a client may send either back.
fn version_of(updated_at: DateTime<Utc>) -> String {
    updated_at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// The `updated_at` an `If-Match` header names, if any. `*` (any version)
/// and an absent header mean no precondition; a weak tag is accepted, since
/// the version is a timestamp either way.
fn if_match(headers: &HeaderMap) -> Result<Option<DateTime<Utc>>> {
    let Some(raw) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .map_err(|_| RaError::Validation("If-Match is not valid text".to_string()))?
        .trim();
    if raw == "*" {
        return Ok(None);
    }
    let tag = raw.strip_prefix("W/").unwrap_or(raw).trim_matches('"');
    DateTime::parse_from_rfc3339(tag)
        .map(|at| Some(at.with_timezone(&Utc)))
        .map_err(|_| {
            RaError::Validation(format!(
                "If-Match must be a memory's ETag (its updated_at), got {raw:?}"
            ))
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::memory_repository::MemoryRepository;

    fn headers_with_if_match(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MATCH, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn no_if_match_is_no_precondition() {
        assert_eq!(if_match(&HeaderMap::new()).unwrap(), None);
        assert_eq!(if_match(&headers_with_if_match("*")).unwrap(), None);
    }

    #[test]
    fn an_etag_round_trips_into_the_version_it_names() {
        let at = DateTime::parse_from_rfc3339("2026-11-10T02:56:07.123456Z")
            .unwrap()
            .with_timezone(&Utc);
        let etag = format!("\"{}\"", version_of(at));

        assert_eq!(if_match(&headers_with_if_match(&etag)).unwrap(), Some(at));
        assert_eq!(
            if_match(&headers_with_if_match(&format!("W/{etag}"))).unwrap(),
            Some(at)
        );
    }

    #[test]
    fn an_if_match_that_is_not_a_version_is_a_validation_error() {
        let err = if_match(&headers_with_if_match("\"abc\"")).unwrap_err();
        assert!(matches!(err, RaError::Validation(_)), "got {err:?}");
    }

    #[test]
    fn categories_list_built_ins_extras_and_stored_leftovers() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "prefers pnpm");
        fixture.save(&fixture.alex, "prefers vitest");
        let mut leftover = crate::memories::application::test_doubles::new_memory("slept 7h");
        leftover.category = Category::Custom("health.sleep".to_string());
        fixture
            .saver()
            .execute(&fixture.alex, leftover, "test")
            .unwrap();
        let active = fixture.memories.list(&fixture.alex, false).unwrap();

        let categories = categories_with_counts(&active, &["Work.Meetings".to_string()]);

        let ids: Vec<&str> = categories.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            &ids[..8],
            DEFAULT_CATEGORIES
                .iter()
                .map(Category::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(&ids[8..], ["health.sleep", "work.meetings"]);

        let coding = &categories[0];
        assert_eq!(
            (coding.label.as_str(), coding.count, coding.built_in),
            ("Coding preference", 2, true)
        );
        let sleep = &categories[8];
        assert_eq!(
            (sleep.label.as_str(), sleep.count, sleep.built_in),
            ("Health sleep", 1, false)
        );
        assert_eq!(categories[9].count, 0);
        assert!(categories[1..8].iter().all(|c| c.count == 0));
    }
}
