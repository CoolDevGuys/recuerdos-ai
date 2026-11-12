//! Wire types for the memories API.
//!
//! Separate from the domain on purpose. The domain is free to change
//! shape; the wire contract is a promise to clients, and these structs
//! are where that promise is written down.

use crate::memories::application::memory_recaller::{RerankSkip, RerankTrace};
use crate::memories::application::structured_memory_saver::{
    AppliesToInput, DetectedProject, SaveNote, SavedMemory, StructuredSave,
};
use crate::memories::domain::applies_to::AppliesTo;
use crate::memories::domain::memory::{Memory, MemorySource};
use crate::memories::domain::recall_ranker::ScoredMemory;
use crate::shared::error::{RaError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `POST /v1/memories:direct`: a memory and its structure
/// (ai-specs/memory-structure-plan.md §3.1). `kind`, `topic` and `origin` are
/// required; the rules in `StructuredMemorySaver` decide the rest.
#[derive(Debug, Deserialize)]
pub struct SaveMemoryRequest {
    pub content: String,
    pub kind: Option<String>,
    pub topic: Option<String>,
    pub applies_to: Option<AppliesToRequest>,
    pub origin: Option<String>,
    pub subtopic: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub valid_until: Option<DateTime<Utc>>,
    /// Optional while categories still drive recall filters and the map;
    /// derived from kind and topic when absent.
    pub category: Option<String>,
    pub client: Option<String>,
    pub session_id: Option<String>,
    // Fields of the old shape, refused by name so a client learns the new
    // one instead of having its value silently dropped.
    subcategory: Option<serde_json::Value>,
    confidence: Option<serde_json::Value>,
    expires_at: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct AppliesToRequest {
    pub target: Option<String>,
    pub project_name: Option<String>,
    #[serde(default)]
    pub new_project: bool,
}

impl SaveMemoryRequest {
    pub fn into_structured(self, detected: Option<DetectedProject>) -> Result<StructuredSave> {
        let retired = [
            (
                self.subcategory.is_some(),
                "subcategory",
                "it is called subtopic now",
            ),
            (
                self.confidence.is_some(),
                "confidence",
                "it is derived from origin now",
            ),
            (
                self.expires_at.is_some(),
                "expires_at",
                "it is called valid_until now",
            ),
        ];
        if let Some((_, field, why)) = retired.iter().find(|(present, _, _)| *present) {
            return Err(RaError::Rejected {
                code: "invalid_value".to_string(),
                message: format!("{field} is no longer accepted: {why}."),
            });
        }
        if self.kind.is_none() && self.topic.is_none() && self.origin.is_none() {
            return Err(RaError::Rejected {
                code: "missing_field".to_string(),
                message: "This is the old save shape. A memory now needs kind, topic, \
                          applies_to and origin; a category alone is not enough. See \
                          docs/memory-model.md."
                    .to_string(),
            });
        }
        Ok(StructuredSave {
            content: self.content,
            kind: self.kind,
            topic: self.topic,
            applies_to: self.applies_to.map(|applies_to| AppliesToInput {
                target: applies_to.target,
                project_name: applies_to.project_name,
                new_project: applies_to.new_project,
            }),
            origin: self.origin,
            subtopic: self.subtopic,
            tags: self.tags,
            valid_until: self.valid_until,
            category: self.category,
            source: MemorySource {
                client: self.client,
                session_id: self.session_id,
            },
            detected_project: detected,
        })
    }
}

/// A saved memory, plus what to tell the user about how it was filed.
#[derive(Debug, Serialize)]
pub struct SavedMemoryResponse {
    #[serde(flatten)]
    pub memory: MemoryResponse,
    /// The sentence to repeat to the user: "Saved as your personal coding
    /// preference (applies to you everywhere)."
    pub message: String,
    /// Machine-readable notes: `applies_to_defaulted`, `filed_from_document`,
    /// `project_created`.
    pub notes: Vec<&'static str>,
}

impl From<&SavedMemory> for SavedMemoryResponse {
    fn from(saved: &SavedMemory) -> Self {
        Self {
            memory: MemoryResponse::from(&saved.memory),
            message: saved.message(),
            notes: saved
                .notes
                .iter()
                .map(|note| match note {
                    SaveNote::AppliesToDefaulted => "applies_to_defaulted",
                    SaveNote::FiledFromDocument => "filed_from_document",
                    SaveNote::ProjectCreated(_) => "project_created",
                })
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemoryRequest {
    pub content: Option<String>,
    pub category: Option<String>,
    /// `null` clears the subcategory; omitting the field leaves it alone.
    #[serde(default, with = "serde_with_double_option")]
    pub subcategory: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    /// `null` clears the expiry; omitting the field leaves it alone.
    #[serde(default, with = "serde_with_double_option")]
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    /// Signed so a client sending `-5` gets the domain's complaint about
    /// the allowed range rather than a deserializer's about Rust's number
    /// types.
    pub limit: Option<i64>,
    /// Overrides `[retrieval].min_relevance` for this query. `Some(0.0)`
    /// means "show me everything the indexes returned, however weak";
    /// omitted means use the server's floor — except on a request that
    /// filters by category, subcategory or tag, where omitting it lifts the
    /// floor: a caller who named a slice has said what they want without
    /// needing their wording to carry it.
    pub min_relevance: Option<f32>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub subcategories: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub since: Option<DateTime<Utc>>,
    /// Read the graph hop as of this instant in valid time. Omitted means
    /// "now"; a value asks what was true then — "what did we deploy on
    /// before the migration?". Only the graph leg is affected.
    pub as_of: Option<DateTime<Utc>>,
    #[serde(default)]
    pub include_superseded: bool,
}

#[derive(Debug, Serialize)]
pub struct MemoryResponse {
    pub id: String,
    pub content: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subcategory: Option<String>,
    pub tags: Vec<String>,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// What sort of statement it is (ai-specs/memory-structure-plan.md §3).
    pub kind: String,
    /// What it is about.
    pub topic: String,
    /// Who or what it is true for.
    pub applies_to: AppliesToResponse,
    /// Where it came from: `user_said`, `user_confirmed`, `inferred` or
    /// `document`.
    pub origin: String,
    /// For goals only: `active`, `achieved` or `abandoned`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_status: Option<String>,
    /// The structure was derived from an old category and may be wrong.
    pub needs_review: bool,
}

#[derive(Debug, Serialize)]
pub struct AppliesToResponse {
    /// `me`, `project` or `general`.
    pub target: String,
    /// The project's name, when `target` is `project`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
}

impl From<&AppliesTo> for AppliesToResponse {
    fn from(applies_to: &AppliesTo) -> Self {
        Self {
            target: applies_to.target().as_str().to_string(),
            project_name: applies_to.project().map(|project| project.name.clone()),
        }
    }
}

impl From<&Memory> for MemoryResponse {
    fn from(memory: &Memory) -> Self {
        Self {
            id: memory.id().to_string(),
            content: memory.content().to_string(),
            category: memory.category().as_str().to_string(),
            subcategory: memory.subcategory().map(|s| s.to_string()),
            tags: memory.tags().to_vec(),
            confidence: memory.confidence(),
            created_at: memory.created_at(),
            updated_at: memory.updated_at(),
            expires_at: memory.expires_at(),
            superseded_by: memory.superseded_by().map(|id| id.to_string()),
            kind: memory.structure().kind().as_str().to_string(),
            topic: memory.structure().topic().as_str().to_string(),
            applies_to: memory.structure().applies_to().into(),
            origin: memory.structure().origin().as_str().to_string(),
            goal_status: memory
                .structure()
                .goal_status()
                .map(|status| status.as_str().to_string()),
            needs_review: memory.structure().needs_review(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub memory: MemoryResponse,
    /// How relevant this memory is to the query, `0.0..=1.0`. The number to
    /// read: it is measured from the matches themselves, so a query with no
    /// good answer produces low numbers rather than a healthy-looking
    /// top hit.
    pub relevance: f32,
    /// The fused rank score. Only meaningful as a sort key within one
    /// response — it is built from result positions, so it is always a
    /// small positive number regardless of match quality.
    pub score: f32,
    /// Why this result was returned. Surfaced so a surprising ranking can
    /// be explained rather than merely distrusted.
    pub matched: MatchResponse,
}

#[derive(Debug, Serialize)]
pub struct MatchResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_rank: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25_rank: Option<usize>,
    /// 1-based rank in the graph leg, present only when a hop reached this
    /// memory (Task 7.3.4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_rank: Option<usize>,
    /// 1-based rank in the rerank stage, present only when one scored this
    /// result (reranking-plan.md Phase 4). `mode = off` — the default —
    /// never emits it, and neither does `shadow`, whose whole promise is
    /// that the wire looks untouched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_rank: Option<usize>,
    /// The reranker's own score for this memory, same presence rules as
    /// `rerank_rank`. It is on the *model's* scale — a probability for one
    /// reranker, a logit for another — so it is a diagnostic, never a
    /// threshold. `relevance` keeps its meaning; this is additive beside it
    /// (invariant I-4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_score: Option<f32>,
}

impl From<&ScoredMemory> for SearchHit {
    fn from(scored: &ScoredMemory) -> Self {
        Self {
            memory: MemoryResponse::from(&scored.memory),
            relevance: scored.relevance,
            score: scored.score,
            matched: MatchResponse {
                vector_rank: scored.match_detail.vector_rank,
                bm25_rank: scored.match_detail.bm25_rank,
                graph_rank: scored.match_detail.graph_rank,
                rerank_rank: scored.match_detail.rerank_rank,
                rerank_score: scored.rerank_score,
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchHit>,
    pub took_ms: u64,
    /// The floor that produced this list. An empty list with a floor above
    /// `0.0` means "nothing cleared it", which is a different fact from
    /// "nothing matched at all" and lets a client decide between giving up
    /// and retrying with `min_relevance: 0.0`.
    pub min_relevance: f32,
    /// What the optional second stage did for this recall — absent when no
    /// stage is configured, so a `mode = "off"` response is exactly what it
    /// always was.
    ///
    /// It is here because the failure mode was invisible: a reranker that
    /// misses `[rerank].timeout_ms` serves the pre-rerank order by design,
    /// and the only sign was a log line. `"state": "skipped"` is that sign
    /// where a client can see it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank: Option<RerankResponse>,
}

/// The wire shape of the domain's `RerankTrace`. Kept separate so the
/// response contract can evolve (and be documented) without the recall path
/// knowing about JSON.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum RerankResponse {
    /// Configured, but the head was too short to reorder.
    NotNeeded { candidates: usize },
    /// Judged the head and folded its order in.
    Applied { judged: usize, took_ms: u64 },
    /// Judged the head and logged the comparison; the old order was served.
    Shadow {
        judged: usize,
        changed: bool,
        took_ms: u64,
    },
    /// Asked, and it did not contribute. The results are the pre-rerank order.
    Skipped {
        reason: RerankSkipReason,
        candidates: usize,
        took_ms: u64,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RerankSkipReason {
    /// Past `[rerank].timeout_ms`. Raise it, or lower `[rerank].depth`.
    Deadline,
    /// The model failed or misaligned its scores.
    Model,
}

impl From<RerankTrace> for RerankResponse {
    fn from(trace: RerankTrace) -> Self {
        match trace {
            RerankTrace::NotNeeded { candidates } => Self::NotNeeded { candidates },
            RerankTrace::Applied { judged, took_ms } => Self::Applied { judged, took_ms },
            RerankTrace::Shadow {
                judged,
                changed,
                took_ms,
            } => Self::Shadow {
                judged,
                changed,
                took_ms,
            },
            RerankTrace::Skipped {
                reason,
                candidates,
                took_ms,
                // Dropped, on purpose and by invariant: the wire has two reason
                // values because that is what every shipped SDK can parse. The
                // busy/partial split lives in the daemon's log and in
                // `recall audit`, which is where the operator who can act on it
                // looks. `a_busy_skip_says_exactly_what_a_slow_one_says` pins
                // that this stays true rather than drifting into "we'll just add
                // a field".
                busy: _,
            } => Self::Skipped {
                reason: match reason {
                    RerankSkip::Deadline => RerankSkipReason::Deadline,
                    RerankSkip::Model => RerankSkipReason::Model,
                },
                candidates,
                took_ms,
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AuditEntryResponse {
    pub memory_id: String,
    pub operation: String,
    pub actor: String,
    /// What changed, when the operation recorded anything worth saying.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct AuditResponse {
    pub entries: Vec<AuditEntryResponse>,
}

/// One category a memory can be filed under, with how many of the caller's
/// active memories are.
#[derive(Debug, Serialize)]
pub struct CategoryResponse {
    pub id: String,
    /// What to show: the user's label, else the default wording.
    pub label: String,
    /// The default wording, so a UI can offer "reset to default".
    pub default_label: String,
    pub built_in: bool,
    /// False for a category still on a stored memory but not in the
    /// user's catalog: shown so its memories can be moved.
    pub known: bool,
    pub count: usize,
}

/// `POST /v1/categories`.
#[derive(Debug, Deserialize)]
pub struct CreateCategoryRequest {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// `PATCH /v1/categories/{id}`: `label` (any category; `null` resets it)
/// and/or `id` (custom only: a rename that re-files the members).
#[derive(Debug, Deserialize)]
pub struct UpdateCategoryRequest {
    #[serde(default, with = "serde_with_double_option")]
    pub label: Option<Option<String>>,
    #[serde(default)]
    pub id: Option<String>,
}

/// `POST /v1/categories/{id}/merge`.
#[derive(Debug, Deserialize)]
pub struct MergeCategoryRequest {
    pub into: String,
}

/// The answer to a change that moved memories between categories.
#[derive(Debug, Serialize)]
pub struct CategoryChangeResponse {
    pub category: Option<CategoryResponse>,
    /// Memories re-filed.
    pub moved: usize,
    /// Of those, re-embedded because their embedded text changed.
    pub reembedded: usize,
}

/// `POST /v1/memories:bulk`.
#[derive(Debug, Deserialize)]
pub struct BulkRequest {
    pub ids: Vec<String>,
    #[serde(default)]
    pub set: BulkSet,
    #[serde(default)]
    pub add_tags: Vec<String>,
    #[serde(default)]
    pub remove_tags: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct BulkSet {
    #[serde(default)]
    pub category: Option<String>,
    /// `null` clears the sub-label; omitting it leaves it alone.
    #[serde(default, with = "serde_with_double_option")]
    pub subcategory: Option<Option<String>>,
}

#[derive(Debug, Serialize)]
pub struct BulkResponse {
    pub memories: Vec<MemoryResponse>,
    pub changed: usize,
    pub reembedded: usize,
}

#[derive(Debug, Serialize)]
pub struct CategoriesResponse {
    pub categories: Vec<CategoryResponse>,
}

/// `GET /v1/map`: everything the mental map draws, in one answer.
#[derive(Debug, Serialize)]
pub struct MapResponse {
    /// Changes whenever the memories do; a client may compare it to know
    /// whether a cached layout still fits.
    pub generation: String,
    pub k: usize,
    /// The least `weight` a similarity edge here has.
    pub min_weight: f32,
    /// True when the store is too large for similarity edges.
    pub edges_omitted: bool,
    /// `memories` when `nodes` holds every memory in scope; `categories`
    /// when the store is above the detail limit and only the category
    /// summary is sent.
    pub detail: &'static str,
    /// Memories in scope, whether or not `nodes` carries them.
    pub total: usize,
    /// The category this map is limited to, if any.
    pub category: Option<String>,
    pub nodes: Vec<MemoryResponse>,
    pub edges: Vec<MapEdge>,
    pub categories: Vec<MapCategory>,
    pub category_edges: Vec<MapCategoryEdge>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MapEdge {
    /// Mutual nearest neighbours; `weight` is similarity above the model's
    /// baseline, `0..=1`.
    Similar {
        source: String,
        target: String,
        weight: f32,
    },
    /// `source` asserted a relation about an entity `target` mentions.
    Relation {
        source: String,
        target: String,
        predicate: String,
        entity: String,
    },
}

#[derive(Debug, Serialize)]
pub struct MapCategory {
    pub id: String,
    pub label: String,
    pub built_in: bool,
    pub count: usize,
    pub subcategories: Vec<MapSubcategory>,
}

#[derive(Debug, Serialize)]
pub struct MapSubcategory {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct MapCategoryEdge {
    pub source: String,
    pub target: String,
    pub weight: f32,
}

/// serde has no built-in way to distinguish "absent" from "explicit
/// null", which is exactly the distinction a PATCH needs.
mod serde_with_double_option {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory::NewMemory;

    fn structured(body: &str) -> Result<StructuredSave> {
        serde_json::from_str::<SaveMemoryRequest>(body)
            .unwrap()
            .into_structured(None)
    }

    fn code(result: Result<StructuredSave>) -> String {
        match result {
            Err(RaError::Rejected { code, .. }) => code,
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn a_save_request_carries_its_structure() {
        let save = structured(
            r#"{"content":"x","kind":"preference","topic":"coding","origin":"user_said",
                "applies_to":{"target":"me"},"tags":["ts"],"subtopic":"testing"}"#,
        )
        .unwrap();

        assert_eq!(save.kind.as_deref(), Some("preference"));
        assert_eq!(save.applies_to.unwrap().target.as_deref(), Some("me"));
        assert_eq!(save.subtopic.as_deref(), Some("testing"));
        assert_eq!(save.tags, vec!["ts".to_string()]);
    }

    #[test]
    fn e14_the_old_category_only_shape_is_refused_naming_the_new_fields() {
        let error = structured(r#"{"content":"x","category":"preference.coding"}"#).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("missing_field"), "{message}");
        for field in ["kind", "topic", "applies_to", "origin"] {
            assert!(message.contains(field), "{field}: {message}");
        }
    }

    #[test]
    fn retired_fields_are_refused_with_their_new_name() {
        let base = r#""content":"x","kind":"fact","topic":"tech","origin":"user_said""#;
        assert_eq!(
            code(structured(&format!(r#"{{{base},"subcategory":"a"}}"#))),
            "invalid_value"
        );
        assert_eq!(
            code(structured(&format!(r#"{{{base},"confidence":0.4}}"#))),
            "invalid_value"
        );
        assert_eq!(
            code(structured(&format!(
                r#"{{{base},"expires_at":"2030-01-01T00:00:00Z"}}"#
            ))),
            "invalid_value"
        );
    }

    #[test]
    fn a_patch_distinguishes_an_absent_field_from_an_explicit_null() {
        let absent: UpdateMemoryRequest = serde_json::from_str(r#"{"content":"x"}"#).unwrap();
        assert_eq!(absent.expires_at, None, "absent means leave alone");

        let cleared: UpdateMemoryRequest = serde_json::from_str(r#"{"expires_at":null}"#).unwrap();
        assert_eq!(cleared.expires_at, Some(None), "null means clear it");
    }

    #[test]
    fn a_patch_distinguishes_absent_from_null_subcategory() {
        // Same three-way distinction the domain's Option<Option<String>>
        // relies on: omit to leave alone, null to clear, a value to set.
        let absent: UpdateMemoryRequest = serde_json::from_str(r#"{"content":"x"}"#).unwrap();
        assert_eq!(absent.subcategory, None, "absent means leave alone");

        let cleared: UpdateMemoryRequest = serde_json::from_str(r#"{"subcategory":null}"#).unwrap();
        assert_eq!(cleared.subcategory, Some(None), "null means clear it");

        let set: UpdateMemoryRequest =
            serde_json::from_str(r#"{"subcategory":"testing"}"#).unwrap();
        assert_eq!(
            set.subcategory,
            Some(Some("testing".to_string())),
            "a value sets it"
        );
    }

    #[test]
    fn a_search_response_omits_absent_ranks_rather_than_sending_null() {
        let hit = SearchHit {
            relevance: 0.8,
            memory: MemoryResponse {
                id: "m1".to_string(),
                content: "x".to_string(),
                category: "decision".to_string(),
                subcategory: None,
                tags: vec![],
                confidence: 1.0,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                expires_at: None,
                superseded_by: None,
                kind: "decision".to_string(),
                topic: "work".to_string(),
                applies_to: AppliesToResponse {
                    target: "general".to_string(),
                    project_name: None,
                },
                origin: "user_said".to_string(),
                goal_status: None,
                needs_review: true,
            },
            score: 0.5,
            matched: MatchResponse {
                vector_rank: Some(1),
                bm25_rank: None,
                graph_rank: None,
                rerank_rank: None,
                rerank_score: None,
            },
        };

        let json = serde_json::to_value(&hit).unwrap();
        assert_eq!(json["matched"]["vector_rank"], 1);
        assert!(json["matched"].get("bm25_rank").is_none());
        assert!(json["matched"].get("graph_rank").is_none());
        // The rerank stage is absent by default and must stay invisible to
        // old clients, not arrive as `null` (reranking-plan.md Phase 4.5).
        assert!(json["matched"].get("rerank_rank").is_none());
        assert!(json["matched"].get("rerank_score").is_none());
        // `flatten` should inline the memory fields, not nest them.
        assert_eq!(json["id"], "m1");
    }

    #[test]
    fn a_reranked_result_carries_its_rerank_fields() {
        // Only `mode = on` populates them, and when it does they ride
        // inside `matched` beside the leg ranks.
        let scored = ScoredMemory {
            memory: Memory::create(
                crate::shared::ids::UserId::new(),
                NewMemory {
                    content: "x".to_string(),
                    category: Category::Decision,
                    subcategory: None,
                    tags: vec![],
                    entities: vec![],
                    confidence: 1.0,
                    source: MemorySource::default(),
                    expires_at: None,
                },
                Utc::now(),
            )
            .unwrap(),
            score: 0.5,
            relevance: 0.8,
            rerank_score: Some(0.97),
            match_detail: crate::memories::domain::recall_ranker::MatchDetail {
                vector_rank: Some(3),
                bm25_rank: None,
                graph_rank: None,
                rerank_rank: Some(1),
            },
        };

        let json = serde_json::to_value(SearchHit::from(&scored)).unwrap();
        assert_eq!(json["matched"]["rerank_rank"], 1);
        let emitted = json["matched"]["rerank_score"].as_f64().unwrap() as f32;
        assert!((emitted - 0.97).abs() < 1e-6, "{emitted}");
    }

    // ── The second stage, on the wire ────────────────────────────────────
    //
    // The promise these pin is the one the fix was for: a client can tell a
    // working reranker from an absent one from a dropped one without reading
    // the daemon's log — and a deployment that never turned one on gets the
    // response it always got.

    fn response_with(rerank: Option<RerankTrace>) -> serde_json::Value {
        serde_json::to_value(SearchResponse {
            results: Vec::new(),
            took_ms: 12,
            min_relevance: 0.25,
            rerank: rerank.map(RerankResponse::from),
        })
        .unwrap()
    }

    #[test]
    fn a_response_without_a_second_stage_says_nothing_about_one() {
        let json = response_with(None);
        assert!(
            json.get("rerank").is_none(),
            "an absent stage must not add a field: {json}"
        );
    }

    #[test]
    fn a_second_stage_that_ran_reports_what_it_judged() {
        let json = response_with(Some(RerankTrace::Applied {
            judged: 10,
            took_ms: 71,
        }));
        assert_eq!(json["rerank"]["state"], "applied");
        assert_eq!(json["rerank"]["judged"], 10);
        assert_eq!(json["rerank"]["took_ms"], 71);
    }

    #[test]
    fn a_second_stage_that_missed_its_deadline_names_the_reason() {
        // The shape an operator on a busy box sees, and the reason they can
        // act on: `timeout_ms`, or a smaller `depth`.
        let json = response_with(Some(RerankTrace::Skipped {
            reason: RerankSkip::Deadline,
            candidates: 10,
            took_ms: 251,
            busy: false,
        }));
        assert_eq!(json["rerank"]["state"], "skipped");
        assert_eq!(json["rerank"]["reason"], "deadline");
        assert_eq!(json["rerank"]["candidates"], 10);

        let model = response_with(Some(RerankTrace::Skipped {
            reason: RerankSkip::Model,
            candidates: 10,
            took_ms: 4,
            busy: false,
        }));
        assert_eq!(model["rerank"]["reason"], "model");
    }

    #[test]
    fn a_busy_skip_says_exactly_what_a_slow_one_says() {
        // Invariant 3.5 read from this side. The SDK types `reason` as
        // Literal["deadline", "model"], so a host that never got a session has
        // to report the same JSON as one that ran out of inference time. The
        // domain does carry the difference (`RerankTrace::Skipped::busy`); the
        // transport is where it is dropped, and this is the test that notices if
        // someone starts forwarding it.
        let refused = response_with(Some(RerankTrace::Skipped {
            reason: RerankSkip::Deadline,
            candidates: 10,
            took_ms: 250,
            busy: true,
        }));
        let slow = response_with(Some(RerankTrace::Skipped {
            reason: RerankSkip::Deadline,
            candidates: 10,
            took_ms: 250,
            busy: false,
        }));
        assert_eq!(
            refused, slow,
            "a busy stage added, removed or renamed a client-visible key"
        );

        let mut keys: Vec<&str> = refused["rerank"]
            .as_object()
            .expect("a rerank object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["candidates", "reason", "state", "took_ms"],
            "the keys of the skipped object are the client's contract"
        );
    }

    #[test]
    fn a_head_too_short_to_reorder_says_so() {
        let json = response_with(Some(RerankTrace::NotNeeded { candidates: 1 }));
        assert_eq!(json["rerank"]["state"], "not_needed");
        assert_eq!(json["rerank"]["candidates"], 1);
    }

    #[test]
    fn shadow_reports_the_move_it_declined_to_make() {
        let json = response_with(Some(RerankTrace::Shadow {
            judged: 6,
            changed: true,
            took_ms: 180,
        }));
        assert_eq!(json["rerank"]["state"], "shadow");
        assert_eq!(json["rerank"]["changed"], true);
        assert_eq!(json["rerank"]["judged"], 6);
    }
}
