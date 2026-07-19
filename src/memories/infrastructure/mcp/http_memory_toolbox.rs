//! `MemoryToolbox` that forwards to a running daemon over HTTP.
//!
//! This is what makes `recordagent mcp` a shim rather than a second copy
//! of the service. See `memory_toolbox.rs` for why that matters: an
//! in-process engine per editor session would load its own 130 MB model
//! and contend for the same SQLite file.
//!
//! It speaks the same REST API any other client would, so there is no
//! privileged back door — the shim is subject to the same authentication
//! and the same per-user scoping.

use super::memory_toolbox::{MemoryToolbox, RecallRequest, SaveRequest, ToolMemory};
use crate::shared::error::{RaError, Result};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

pub struct HttpMemoryToolbox {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

impl HttpMemoryToolbox {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            http: reqwest::Client::new(),
        }
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(&self.api_key);
        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request.send().await.map_err(|e| {
            // The overwhelmingly likely cause, so say it rather than
            // making someone decode a connection error.
            RaError::Internal(format!(
                "could not reach the RecordAgent daemon at {}: {e}. Is it running?",
                self.base_url
            ))
        })?;

        let status = response.status();
        let payload: Value = response.json().await.unwrap_or_else(|_| json!({}));

        if status.is_success() {
            return Ok(payload);
        }

        // Re-inflate the daemon's error envelope so the shim reports the
        // same failure the REST caller would have seen.
        let message = payload["error"]["message"]
            .as_str()
            .unwrap_or("request failed")
            .to_string();
        Err(match status.as_u16() {
            400 => RaError::Validation(message),
            401 => RaError::Unauthorized(message),
            403 => RaError::Forbidden(message),
            404 => RaError::NotFound(message),
            409 => RaError::Conflict(message),
            _ => RaError::Internal(format!("daemon returned {status}: {message}")),
        })
    }
}

#[async_trait::async_trait]
impl MemoryToolbox for HttpMemoryToolbox {
    async fn save(&self, request: SaveRequest) -> Result<ToolMemory> {
        let mut body = json!({
            "content": request.content,
            "tags": request.tags,
        });
        if let Some(category) = request.category {
            body["category"] = json!(category);
        }
        if let Some(client) = request.client {
            body["client"] = json!(client);
        }

        let saved = self
            .request(reqwest::Method::POST, "/v1/memories:direct", Some(body))
            .await?;
        parse_memory(&saved)
    }

    async fn recall(&self, request: RecallRequest) -> Result<Vec<ToolMemory>> {
        let mut body = json!({
            "query": request.query,
            "categories": request.categories,
        });
        if let Some(limit) = request.limit {
            body["limit"] = json!(limit);
        }

        let response = self
            .request(reqwest::Method::POST, "/v1/memories/search", Some(body))
            .await?;

        response["results"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(parse_memory)
            .collect()
    }

    async fn find_candidates(&self, query: &str, limit: usize) -> Result<Vec<ToolMemory>> {
        self.recall(RecallRequest {
            query: query.to_string(),
            categories: Vec::new(),
            limit: Some(limit),
        })
        .await
    }

    async fn forget(&self, ids: &[String]) -> Result<usize> {
        let mut deleted = 0;
        for id in ids {
            self.request(reqwest::Method::DELETE, &format!("/v1/memories/{id}"), None)
                .await?;
            deleted += 1;
        }
        Ok(deleted)
    }

    async fn profile(&self) -> Result<String> {
        // The profile has no REST route of its own — it is assembled from
        // the same export the user can already fetch, so the shim asks
        // for that and the daemon does the assembling for the HTTP
        // transport. Keeping one assembler means both transports show the
        // same profile.
        let response = self
            .http
            .get(format!("{}/v1/profile", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| {
                RaError::Internal(format!(
                    "could not reach the RecordAgent daemon at {}: {e}. Is it running?",
                    self.base_url
                ))
            })?;

        if !response.status().is_success() {
            return Err(RaError::Internal(format!(
                "daemon returned {} for the profile",
                response.status()
            )));
        }

        response
            .text()
            .await
            .map_err(|e| RaError::Internal(format!("failed to read the profile: {e}")))
    }
}

fn parse_memory(value: &Value) -> Result<ToolMemory> {
    let field = |name: &str| -> Result<String> {
        value[name]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| RaError::Internal(format!("daemon response is missing {name:?}")))
    };

    Ok(ToolMemory {
        id: field("id")?,
        content: field("content")?,
        category: field("category")?,
        tags: value["tags"]
            .as_array()
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| tag.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        created_at: parse_timestamp(&field("created_at")?)?,
        score: value["score"].as_f64().map(|score| score as f32),
    })
}

fn parse_timestamp(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|e| RaError::Internal(format!("daemon sent an unparseable timestamp: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_memory_from_the_daemons_json() {
        let value = json!({
            "id": "019f7c5a-0000-7000-8000-000000000001",
            "content": "User prefers pnpm",
            "category": "preference.coding",
            "tags": ["typescript"],
            "created_at": "2026-06-02T12:00:00Z",
            "score": 0.91
        });

        let memory = parse_memory(&value).unwrap();

        assert_eq!(memory.content, "User prefers pnpm");
        assert_eq!(memory.category, "preference.coding");
        assert_eq!(memory.tags, vec!["typescript".to_string()]);
        assert_eq!(memory.score, Some(0.91));
    }

    #[test]
    fn a_memory_without_a_score_parses_as_unscored() {
        // Saves return no score; only search results carry one.
        let value = json!({
            "id": "019f7c5a-0000-7000-8000-000000000001",
            "content": "x",
            "category": "fact.project",
            "created_at": "2026-06-02T12:00:00Z"
        });

        let memory = parse_memory(&value).unwrap();

        assert_eq!(memory.score, None);
        assert!(memory.tags.is_empty());
    }

    #[test]
    fn a_malformed_response_is_an_error_rather_than_a_panic() {
        let error = parse_memory(&json!({"content": "no id"})).unwrap_err();
        assert!(error.to_string().contains("missing"), "{error}");
    }

    #[test]
    fn trailing_slashes_in_the_base_url_do_not_double_up() {
        let toolbox = HttpMemoryToolbox::new("http://localhost:7070/", "key");
        assert_eq!(toolbox.base_url, "http://localhost:7070");
    }
}
