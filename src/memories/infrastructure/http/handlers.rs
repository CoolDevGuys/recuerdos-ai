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
    AuditEntryResponse, AuditResponse, BulkRequest, BulkResponse, CategoriesResponse,
    CategoryChangeResponse, CategoryResponse, CreateCategoryRequest, CreateTopicRequest, MapEdge,
    MapResponse, MapSubtopic, MapTopic, MapTopicEdge, MemoryResponse, MergeCategoryRequest,
    MergeProjectRequest, ProjectChangeResponse, ProjectResponse, ProjectsResponse,
    RelabelTopicRequest, RenameProjectRequest, RerankResponse, SaveMemoryRequest,
    SavedMemoryResponse, SearchHit, SearchRequest, SearchResponse, SelfNamesResponse,
    SetSelfNamesRequest, TopicResponse, TopicsResponse, UpdateCategoryRequest, UpdateMemoryRequest,
    UpdatedMemoryResponse,
};
use crate::bootstrap::state::AppState;
use crate::identity::domain::user_context::UserContext;
use crate::identity::infrastructure::http::authenticated::{ReadAccess, WriteAccess};
use crate::memories::application::category_curator::{CategoryCurator, CategoryView};
use crate::memories::application::memory_bulk_editor::{BulkEdit, StructureChange};
use crate::memories::application::memory_exporter::ExportFormat;
use crate::memories::application::memory_mapper::{MapDetail, MapQuery};
use crate::memories::application::structured_memory_saver::{DetectedProject, SaveResult};
use crate::memories::domain::applies_to::{AppliesTo, Target};
use crate::memories::domain::category::Category;
use crate::memories::domain::kind::Kind;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::recall_query::RecallQuery;
use crate::shared::blocking::blocking;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use axum::Json;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

/// The client name recorded in the audit trail for a REST write that does
/// not name itself.
const ACTOR: &str = "rest";

/// Who the audit trail says made a REST write: the client's own name from
/// `X-Recuerdos-Client` when it sends a well-formed one (the web UI sends
/// `web-ui`), else `rest`. Informational, like a memory's `source.client`:
/// any caller can claim any name, so nothing is ever decided by it — it
/// only lets "why did these 25 memories move?" be answered from the trail.
pub struct Actor(String);

impl<S: Send + Sync> FromRequestParts<S> for Actor {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        let named = parts
            .headers
            .get("x-recuerdos-client")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|name| {
                (1..=32).contains(&name.len())
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            });
        Ok(Actor(named.unwrap_or(ACTOR).to_string()))
    }
}

pub use crate::shared::project_headers::{PROJECT_HEADER, PROJECT_REMOTE_HEADER};

fn detected_project(headers: &HeaderMap) -> Option<DetectedProject> {
    crate::shared::project_headers::project_from(headers)
        .map(|(name, alias)| DetectedProject { name, alias })
}

pub async fn save_memory(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Actor(actor): Actor,
    headers: HeaderMap,
    Json(request): Json<SaveMemoryRequest>,
) -> Result<impl IntoResponse> {
    let memories = state.memories.clone();
    let save = request.into_structured(detected_project(&headers))?;

    let catalog = Arc::clone(&memories.catalog);
    let lookup = context.clone();
    let extras = blocking(move || catalog.custom_ids(&lookup)).await?;
    let result = memories
        .structured_saver
        .save(&context, save, extras, &actor)
        .await?;

    let status = match result {
        SaveResult::Saved(_) => StatusCode::CREATED,
        SaveResult::AlreadyKnown { .. } => StatusCode::OK,
    };
    Ok((status, Json(SavedMemoryResponse::from(&result))))
}

pub async fn search_memories(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Json<SearchResponse>> {
    let memories = state.memories.clone();

    // The default floor exists for a caller who can only describe what they
    // want in prose: without it, a weak similarity and a strong one look
    // identical and unrelated memories arrive dressed as matches. A request
    // that names a category, subcategory, tag or any structure filter has
    // already said what it wants in the store's own vocabulary, so its prose
    // is a preference *inside* that slice rather than the definition of it —
    // holding the floor there would turn "what decisions have I stored?"
    // into an empty answer merely because the wording was a poor vector
    // match.
    //
    // An explicit number is never second-guessed, filtered or not.
    let structurally_filtered = !request.categories.is_empty()
        || !request.subcategories.is_empty()
        || !request.tags.is_empty()
        || !request.kinds.is_empty()
        || !request.topics.is_empty()
        || !request.targets.is_empty()
        || !request.projects.is_empty();
    let min_relevance = match request.min_relevance {
        Some(explicit) => explicit,
        None if structurally_filtered => 0.0,
        None => memories.min_relevance,
    }
    .clamp(0.0, 1.0);

    let detected = detected_project(&headers);
    let lookup = memories.clone();
    let filter_context = context.clone();
    let query = blocking(move || {
        let categories = if request.categories.is_empty() {
            Vec::new()
        } else {
            let custom = lookup.catalog.custom_ids(&filter_context)?;
            request
                .categories
                .iter()
                .map(|raw| Category::parse_with_extras(raw, &custom))
                .collect::<Result<Vec<_>>>()?
        };
        let curator = &lookup.project_curator;
        let kinds = request
            .kinds
            .iter()
            .map(|raw| raw.parse::<Kind>())
            .collect::<Result<Vec<_>>>()?;
        let targets = request
            .targets
            .iter()
            .map(|raw| raw.parse::<Target>())
            .collect::<Result<Vec<_>>>()?;
        let projects = request
            .projects
            .iter()
            .map(|raw| curator.find(&filter_context, raw).map(|project| project.id))
            .collect::<Result<Vec<_>>>()?;

        let mut query = RecallQuery::new(
            &request.query,
            request.limit.unwrap_or(lookup.default_limit as i64),
        )?
        .with_categories(categories)
        .with_subcategories(request.subcategories)
        .with_tags(request.tags)
        .with_since(request.since)
        .with_as_of(request.as_of)
        .with_min_relevance(min_relevance)
        .with_kinds(kinds)
        .with_topics(request.topics)
        .with_targets(targets)
        .with_projects(projects);
        if request.include_superseded {
            query = query.including_superseded();
        }
        match request.scope.as_deref().map(str::trim) {
            None | Some("context") => {
                let current = match (&request.project, &detected) {
                    (Some(named), _) => Some(curator.find(&filter_context, named)?),
                    (None, Some(detected)) => curator.locate(&filter_context, detected)?,
                    (None, None) => None,
                };
                query = query.in_context(current);
            }
            Some("all") => {}
            Some(other) => {
                return Err(RaError::Validation(format!(
                    "unknown scope {other:?}: \"context\" (the default) or \"all\""
                )));
            }
        }
        Ok(query)
    })
    .await?;

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
    Actor(actor): Actor,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateMemoryRequest>,
) -> Result<impl IntoResponse> {
    let id = parse_id(&id)?;
    let expected_updated_at = if_match(&headers)?;
    let detected = detected_project(&headers);
    let memories = state.memories.clone();

    let updated = blocking(move || {
        let category = request
            .category
            .as_deref()
            .map(|raw| memories.catalog.resolve(&context, raw))
            .transpose()?;
        let edit = request.into_structured(category, expected_updated_at, detected);
        memories
            .structured_updater
            .execute(&context, id, edit, &actor)
    })
    .await?;

    Ok((
        etag_of(&updated.memory),
        Json(UpdatedMemoryResponse::from(&updated)),
    ))
}

/// Brings back a memory forgotten with `DELETE` — the web UI's undo.
pub async fn restore_memory(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Actor(actor): Actor,
    Path(id): Path<String>,
) -> Result<impl IntoResponse> {
    let id = parse_id(&id)?;
    let memories = state.memories.clone();

    let memory = blocking(move || memories.restorer.execute(&context, id, &actor)).await?;

    Ok(with_etag(&memory))
}

#[derive(Debug, Deserialize)]
pub struct MapParams {
    #[serde(default)]
    k: Option<usize>,
    #[serde(default)]
    include_superseded: bool,
    /// Only this topic's memories.
    #[serde(default)]
    topic: Option<String>,
}

/// The mental map: every memory, how they link, and the topics they are
/// grouped by (ai-specs/memory-structure-plan.md D13). Above the detail
/// limit, the topics alone (`detail: "topics"`); ask with `?topic=` for one
/// topic's memories.
pub async fn read_map(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
    Query(params): Query<MapParams>,
) -> Result<Json<MapResponse>> {
    let query = MapQuery::new(params.k, params.include_superseded)?.in_topic(params.topic)?;
    let scope = query.topic.clone();
    let memories = state.memories.clone();
    let (map, labels) = blocking(move || {
        let map = memories.mapper.execute(&context, query)?;
        // The user's own labels: the map shows the words the Topics tab shows.
        let labels: BTreeMap<String, String> = memories
            .topic_curator
            .list(&context)?
            .into_iter()
            .map(|view| (view.entry.topic.as_str().to_string(), view.entry.label()))
            .collect();
        Ok((map, labels))
    })
    .await?;

    let similar = map.similar.iter().map(|edge| MapEdge::Similar {
        source: edge.source.to_string(),
        target: edge.target.to_string(),
        weight: edge.weight,
    });
    let relations = map.links.iter().map(|link| MapEdge::Relation {
        source: link.from.to_string(),
        target: link.to.to_string(),
        predicate: link.predicate.clone(),
        entity: link.entity.clone(),
    });

    Ok(Json(MapResponse {
        generation: map.generation.to_string(),
        k: map.k,
        min_weight: map.min_weight,
        edges_omitted: map.edges_omitted,
        detail: match map.detail {
            MapDetail::Memories => "memories",
            MapDetail::Topics => "topics",
        },
        total: map.total,
        topic: scope,
        nodes: map.memories.iter().map(MemoryResponse::from).collect(),
        edges: similar.chain(relations).collect(),
        topics: map
            .topics
            .into_iter()
            .map(|t| MapTopic {
                label: labels.get(&t.id).cloned().unwrap_or(t.label),
                id: t.id,
                built_in: t.built_in,
                count: t.count,
                subtopics: t
                    .subcategories
                    .into_iter()
                    .map(|(name, count)| MapSubtopic { name, count })
                    .collect(),
            })
            .collect(),
        topic_edges: map
            .topic_edges
            .into_iter()
            .map(|e| MapTopicEdge {
                source: e.source,
                target: e.target,
                weight: e.weight,
            })
            .collect(),
    }))
}

/// The categories a memory can be filed under, with counts.
///
/// Built-ins first in taxonomy order (even when empty: they are always
/// valid), then the user's custom categories, then any category still on
/// a stored memory but missing from their catalog — a row written when
/// the taxonomy was wider is still the user's, and they need to see it to
/// move it.
pub async fn list_categories(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
) -> Result<Json<CategoriesResponse>> {
    let memories = state.memories.clone();
    let views = blocking(move || memories.category_curator.list(&context)).await?;

    Ok(Json(CategoriesResponse {
        categories: views.iter().map(category_response).collect(),
    }))
}

fn category_response(view: &CategoryView) -> CategoryResponse {
    CategoryResponse {
        id: view.category.as_str().to_string(),
        label: view.label.clone(),
        default_label: view.category.label(),
        built_in: view.category.is_built_in(),
        known: view.known,
        count: view.count,
    }
}

/// The category `id` as the list now shows it.
fn listed(curator: &CategoryCurator, context: &UserContext, id: &str) -> Result<CategoryResponse> {
    curator
        .list(context)?
        .iter()
        .find(|view| view.category.as_str() == id)
        .map(category_response)
        .ok_or_else(|| RaError::NotFound(format!("category {id:?} not found")))
}

pub async fn create_category(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Json(request): Json<CreateCategoryRequest>,
) -> Result<impl IntoResponse> {
    let memories = state.memories.clone();
    let created = blocking(move || {
        let curator = &memories.category_curator;
        let entry = curator.create(&context, &request.id, request.label.as_deref())?;
        listed(curator, &context, entry.category.as_str())
    })
    .await?;

    Ok((StatusCode::CREATED, Json(created)))
}

/// Relabels any category, and/or renames a custom one (re-filing and
/// re-embedding its members).
pub async fn update_category(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Actor(actor): Actor,
    Path(id): Path<String>,
    Json(request): Json<UpdateCategoryRequest>,
) -> Result<Json<CategoryChangeResponse>> {
    if request.label.is_none() && request.id.is_none() {
        return Err(RaError::Validation(
            "send a new `label`, a new `id`, or both".to_string(),
        ));
    }
    let memories = state.memories.clone();
    let response = blocking(move || {
        let curator = &memories.category_curator;
        let mut current = id;
        let (mut moved, mut reembedded) = (0, 0);
        if let Some(to) = request.id.as_deref() {
            let (entry, outcome) = curator.rename(&context, &current, to, &actor)?;
            current = entry.category.as_str().to_string();
            moved = outcome.changed;
            reembedded = outcome.reembedded;
        }
        if let Some(label) = request.label {
            curator.relabel(&context, &current, label.as_deref())?;
        }
        Ok(CategoryChangeResponse {
            category: Some(listed(curator, &context, &current)?),
            moved,
            reembedded,
        })
    })
    .await?;

    Ok(Json(response))
}

/// Moves every member of a custom category into another, then deletes it.
pub async fn merge_category(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Actor(actor): Actor,
    Path(id): Path<String>,
    Json(request): Json<MergeCategoryRequest>,
) -> Result<Json<CategoryChangeResponse>> {
    let memories = state.memories.clone();
    let response = blocking(move || {
        let curator = &memories.category_curator;
        let outcome = curator.merge(&context, &id, &request.into, &actor)?;
        let into = memories.catalog.resolve(&context, &request.into)?;
        Ok(CategoryChangeResponse {
            category: Some(listed(curator, &context, into.as_str())?),
            moved: outcome.changed,
            reembedded: outcome.reembedded,
        })
    })
    .await?;

    Ok(Json(response))
}

#[derive(Debug, Deserialize)]
pub struct DeleteCategoryParams {
    #[serde(default)]
    move_to: Option<String>,
}

/// Deletes a custom category: 409 while it has members, unless
/// `?move_to=` says where they go.
pub async fn delete_category(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Actor(actor): Actor,
    Path(id): Path<String>,
    Query(params): Query<DeleteCategoryParams>,
) -> Result<Json<CategoryChangeResponse>> {
    let memories = state.memories.clone();
    let moved = blocking(move || {
        memories
            .category_curator
            .delete(&context, &id, params.move_to.as_deref(), &actor)
    })
    .await?;

    Ok(Json(CategoryChangeResponse {
        category: None,
        moved,
        reembedded: 0,
    }))
}

/// Re-files, re-labels or re-tags many memories in one transaction.
pub async fn bulk_edit_memories(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Actor(actor): Actor,
    Json(request): Json<BulkRequest>,
) -> Result<Json<BulkResponse>> {
    let ids = request
        .ids
        .iter()
        .map(|raw| parse_id(raw))
        .collect::<Result<Vec<_>>>()?;
    let memories = state.memories.clone();
    let outcome = blocking(move || {
        let category = request
            .set
            .category
            .clone()
            .as_deref()
            .map(|raw| memories.catalog.resolve(&context, raw))
            .transpose()?;
        let set = request.set;
        let applies_to = match set.applies_to {
            None => None,
            Some(applies_to) => Some(
                match (
                    applies_to.target.parse::<Target>()?,
                    applies_to.project_name,
                ) {
                    (Target::Me, None) => AppliesTo::Me,
                    (Target::General, None) => AppliesTo::General,
                    (Target::Project, Some(name)) => {
                        AppliesTo::Project(memories.project_curator.find(&context, &name)?)
                    }
                    (Target::Project, None) => {
                        return Err(RaError::rejected(
                            "no_project",
                            "Name the project: applies_to.project_name.",
                        ));
                    }
                    (_, Some(_)) => {
                        return Err(RaError::rejected(
                            "invalid_value",
                            "applies_to.project_name is only for target \"project\".",
                        ));
                    }
                },
            ),
        };
        let structure = StructureChange {
            kind: set.kind.as_deref().map(str::parse::<Kind>).transpose()?,
            topic: set
                .topic
                .as_deref()
                .map(|raw| memories.topic_curator.known(&context, raw))
                .transpose()?,
            applies_to,
            reviewed: set.reviewed,
        };
        let edit = BulkEdit {
            category,
            subcategory: set.subcategory,
            add_tags: request.add_tags,
            remove_tags: request.remove_tags,
            structure,
        };
        memories.bulk_editor.execute(&context, &ids, &edit, &actor)
    })
    .await?;

    Ok(Json(BulkResponse {
        memories: outcome.memories.iter().map(MemoryResponse::from).collect(),
        changed: outcome.changed,
        reembedded: outcome.reembedded,
    }))
}

/// The memory, with its version as an `ETag` for a later `If-Match`.
fn with_etag(memory: &Memory) -> impl IntoResponse + use<> {
    (etag_of(memory), Json(MemoryResponse::from(memory)))
}

/// The `ETag` header naming this version of the memory.
fn etag_of(memory: &Memory) -> HeaderMap {
    let tag = format!("\"{}\"", version_of(memory.updated_at()));
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&tag) {
        headers.insert(header::ETAG, value);
    }
    headers
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
    Actor(actor): Actor,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let id = parse_id(&id)?;
    let memories = state.memories.clone();

    blocking(move || memories.forgetter.execute(&context, id, &actor, "")).await?;

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

/// `GET /v1/topics`: built-ins, then the user's own, with counts.
pub async fn list_topics(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
) -> Result<Json<TopicsResponse>> {
    let memories = state.memories.clone();
    let views = blocking(move || memories.topic_curator.list(&context)).await?;
    Ok(Json(TopicsResponse {
        topics: views.iter().map(TopicResponse::from).collect(),
    }))
}

/// `GET /v1/me`: the names "the user" goes by in this store.
pub async fn get_self_names(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
) -> Result<Json<SelfNamesResponse>> {
    let memories = state.memories.clone();
    let names = blocking(move || memories.self_names_curator.get(&context)).await?;
    Ok(Json(SelfNamesResponse {
        names: names.names().to_vec(),
        sentence: names.sentence(),
        flagged: None,
    }))
}

/// `PUT /v1/me`: replaces the user's names, and sends the memories they show
/// to be the user's own (filed as general) to the review queue.
pub async fn set_self_names(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Actor(actor): Actor,
    Json(request): Json<SetSelfNamesRequest>,
) -> Result<Json<SelfNamesResponse>> {
    let memories = state.memories.clone();
    let saved = blocking(move || {
        memories
            .self_names_curator
            .set(&context, &request.names, &actor)
    })
    .await?;
    Ok(Json(SelfNamesResponse {
        names: saved.names.names().to_vec(),
        sentence: saved.names.sentence(),
        flagged: Some(saved.flagged),
    }))
}

/// `POST /v1/topics`: adds a topic of the user's own.
pub async fn create_topic(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Json(request): Json<CreateTopicRequest>,
) -> Result<impl IntoResponse> {
    let memories = state.memories.clone();
    let view = blocking(move || {
        memories
            .topic_curator
            .create(&context, &request.id, request.label.as_deref())
    })
    .await?;
    Ok((StatusCode::CREATED, Json(TopicResponse::from(&view))))
}

/// `PATCH /v1/topics/{id}`: relabels any topic.
pub async fn relabel_topic(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Path(id): Path<String>,
    Json(request): Json<RelabelTopicRequest>,
) -> Result<Json<TopicResponse>> {
    let memories = state.memories.clone();
    let view = blocking(move || {
        memories
            .topic_curator
            .relabel(&context, &id, request.label.as_deref())
    })
    .await?;
    Ok(Json(TopicResponse::from(&view)))
}

/// `GET /v1/projects`: the user's projects, with what finds each one and
/// how many memories it holds.
pub async fn list_projects(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
) -> Result<Json<ProjectsResponse>> {
    let memories = state.memories.clone();
    let views = blocking(move || memories.project_curator.list(&context)).await?;
    Ok(Json(ProjectsResponse {
        projects: views.iter().map(ProjectResponse::from).collect(),
    }))
}

/// `GET /v1/projects/{project}`, by id, name or alias.
pub async fn get_project(
    State(state): State<AppState>,
    ReadAccess(context): ReadAccess,
    Path(project): Path<String>,
) -> Result<Json<ProjectResponse>> {
    let memories = state.memories.clone();
    let view = blocking(move || {
        let curator = &memories.project_curator;
        let found = curator.find(&context, &project)?;
        curator.view(&context, &found)
    })
    .await?;
    Ok(Json(ProjectResponse::from(&view)))
}

/// `PATCH /v1/projects/{project}`: renames it. Sent from inside a project
/// folder (the local MCP server's headers) whose project is the new name,
/// the folder's remote is learned too — the fix for a repository renamed on
/// its host (plan E15).
pub async fn rename_project(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    headers: HeaderMap,
    Path(project): Path<String>,
    Json(request): Json<RenameProjectRequest>,
) -> Result<Json<ProjectChangeResponse>> {
    let detected = detected_project(&headers);
    let memories = state.memories.clone();
    let renamed = blocking(move || {
        memories
            .project_curator
            .rename(&context, &project, &request.name, detected.as_ref())
    })
    .await?;
    Ok(Json(ProjectChangeResponse::from(&renamed)))
}

/// `POST /v1/projects/{project}/merge`: folds it into `into`.
pub async fn merge_project(
    State(state): State<AppState>,
    WriteAccess(context): WriteAccess,
    Actor(actor): Actor,
    Path(project): Path<String>,
    Json(request): Json<MergeProjectRequest>,
) -> Result<Json<ProjectChangeResponse>> {
    let memories = state.memories.clone();
    let merged = blocking(move || {
        memories
            .project_curator
            .merge(&context, &project, &request.into, &actor)
    })
    .await?;
    Ok(Json(ProjectChangeResponse::from(&merged)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;

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
    fn categories_list_built_ins_the_catalog_and_stored_leftovers() {
        use crate::memories::domain::category::DEFAULT_CATEGORIES;
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "prefers pnpm");
        fixture.save(&fixture.alex, "prefers vitest");
        let mut leftover = crate::memories::application::test_doubles::new_memory("slept 7h");
        leftover.category = Category::Custom("health.sleep".to_string());
        fixture
            .saver()
            .execute(&fixture.alex, leftover, "test")
            .unwrap();
        let curator = fixture.category_curator(&["Work.Meetings"]);

        let categories: Vec<CategoryResponse> = curator
            .list(&fixture.alex)
            .unwrap()
            .iter()
            .map(category_response)
            .collect();

        let ids: Vec<&str> = categories.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            &ids[..8],
            DEFAULT_CATEGORIES
                .iter()
                .map(Category::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(&ids[8..], ["work.meetings", "health.sleep"]);

        let coding = &categories[0];
        assert_eq!(
            (coding.label.as_str(), coding.count, coding.built_in),
            ("Coding preference", 2, true)
        );
        let meetings = &categories[8];
        assert_eq!((meetings.count, meetings.known), (0, true));
        let sleep = &categories[9];
        assert_eq!(
            (
                sleep.label.as_str(),
                sleep.count,
                sleep.built_in,
                sleep.known
            ),
            ("Health sleep", 1, false, false)
        );
        assert!(categories[1..8].iter().all(|c| c.count == 0));
    }
}
