//! Wire types for the memories API.
//!
//! Separate from the domain on purpose. The domain is free to change
//! shape; the wire contract is a promise to clients, and these structs
//! are where that promise is written down.

use crate::memories::domain::category::Category;
use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
use crate::memories::domain::recall_ranker::ScoredMemory;
use crate::shared::error::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

fn default_confidence() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
pub struct SaveMemoryRequest {
    pub content: String,
    /// Defaults to `fact.project` when a client has no opinion. Phase 4's
    /// pipeline is what makes this a real classification; a verbatim save
    /// should not be forced to guess.
    pub category: Option<String>,
    pub subcategory: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    pub client: Option<String>,
    pub session_id: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl SaveMemoryRequest {
    pub fn into_new_memory(self, extra_categories: &[String]) -> Result<NewMemory> {
        let category = match self.category.as_deref() {
            Some(raw) => Category::parse_with_extras(raw, extra_categories)?,
            None => Category::FactProject,
        };

        Ok(NewMemory {
            content: self.content,
            category,
            subcategory: self.subcategory,
            tags: self.tags,
            entities: Vec::new(),
            confidence: self.confidence,
            source: MemorySource {
                client: self.client,
                session_id: self.session_id,
            },
            expires_at: self.expires_at,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemoryRequest {
    pub content: Option<String>,
    pub category: Option<String>,
    pub subcategory: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    /// `null` clears the expiry; omitting the field leaves it alone.
    #[serde(default, with = "serde_with_double_option")]
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub limit: Option<usize>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub subcategories: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub since: Option<DateTime<Utc>>,
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
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub memory: MemoryResponse,
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
}

impl From<&ScoredMemory> for SearchHit {
    fn from(scored: &ScoredMemory) -> Self {
        Self {
            memory: MemoryResponse::from(&scored.memory),
            score: scored.score,
            matched: MatchResponse {
                vector_rank: scored.match_detail.vector_rank,
                bm25_rank: scored.match_detail.bm25_rank,
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchHit>,
    pub took_ms: u64,
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

    #[test]
    fn a_save_request_defaults_its_optional_fields() {
        let request: SaveMemoryRequest =
            serde_json::from_str(r#"{"content":"User prefers pnpm"}"#).unwrap();

        assert_eq!(request.confidence, 1.0);
        assert!(request.tags.is_empty());

        let new = request.into_new_memory(&[]).unwrap();
        assert_eq!(new.category, Category::FactProject);
    }

    #[test]
    fn a_save_request_parses_its_category() {
        let request: SaveMemoryRequest =
            serde_json::from_str(r#"{"content":"x","category":"preference.coding","tags":["ts"]}"#)
                .unwrap();

        let new = request.into_new_memory(&[]).unwrap();
        assert_eq!(new.category, Category::PreferenceCoding);
        assert_eq!(new.tags, vec!["ts".to_string()]);
    }

    #[test]
    fn an_unknown_category_is_rejected() {
        let request: SaveMemoryRequest =
            serde_json::from_str(r#"{"content":"x","category":"nonsense"}"#).unwrap();

        assert!(request.into_new_memory(&[]).is_err());
    }

    #[test]
    fn a_configured_extra_category_is_accepted() {
        let request: SaveMemoryRequest =
            serde_json::from_str(r#"{"content":"x","category":"fact.homelab"}"#).unwrap();

        let new = request
            .into_new_memory(&["fact.homelab".to_string()])
            .unwrap();
        assert_eq!(new.category.as_str(), "fact.homelab");
    }

    #[test]
    fn a_patch_distinguishes_an_absent_field_from_an_explicit_null() {
        let absent: UpdateMemoryRequest = serde_json::from_str(r#"{"content":"x"}"#).unwrap();
        assert_eq!(absent.expires_at, None, "absent means leave alone");

        let cleared: UpdateMemoryRequest = serde_json::from_str(r#"{"expires_at":null}"#).unwrap();
        assert_eq!(cleared.expires_at, Some(None), "null means clear it");
    }

    #[test]
    fn a_search_response_omits_absent_ranks_rather_than_sending_null() {
        let hit = SearchHit {
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
            },
            score: 0.5,
            matched: MatchResponse {
                vector_rank: Some(1),
                bm25_rank: None,
            },
        };

        let json = serde_json::to_value(&hit).unwrap();
        assert_eq!(json["matched"]["vector_rank"], 1);
        assert!(json["matched"].get("bm25_rank").is_none());
        // `flatten` should inline the memory fields, not nest them.
        assert_eq!(json["id"], "m1");
    }
}
