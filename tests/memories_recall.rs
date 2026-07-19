//! Black-box scenario tests for the memories API.
//!
//! These drive the real binary over HTTP with the real embedding model,
//! the real sqlite-vec index and the real tantivy index — the same path a
//! client takes. They are the tests that would catch a wiring mistake the
//! use-case tests (which run over doubles) cannot see.
//!
//! They implement the project-plan.md §12 scenarios directly.

mod common;

use common::TestApp;
use serde_json::{Value, json};

struct Client {
    app: TestApp,
    key: String,
    http: reqwest::Client,
}

impl Client {
    async fn spawn() -> Self {
        let app = TestApp::spawn().await;
        let key = app.create_user_with_key("alex", "read,write");
        Self {
            app,
            key,
            http: reqwest::Client::new(),
        }
    }

    async fn save(&self, body: Value) -> Value {
        let response = self
            .http
            .post(format!("{}/v1/memories:direct", self.app.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .expect("save request");

        assert_eq!(response.status(), 201, "save should succeed");
        response.json().await.expect("json body")
    }

    async fn search(&self, body: Value) -> Value {
        let response = self
            .http
            .post(format!("{}/v1/memories/search", self.app.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .expect("search request");

        assert_eq!(response.status(), 200, "search should succeed");
        response.json().await.expect("json body")
    }
}

fn contents(results: &Value) -> Vec<String> {
    results["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|hit| hit["content"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn the_core_loop_save_then_recall_by_meaning() {
    // project-plan.md §12.1: a coding preference stated once is found
    // later by a differently-worded question. This is the whole product
    // in one test.
    let client = Client::spawn().await;

    client
        .save(json!({
            "content": "User forbids barrel files and index.ts re-exports; import directly from source modules",
            "category": "preference.coding",
            "tags": ["typescript", "imports"]
        }))
        .await;
    client
        .save(json!({"content": "The office coffee machine is on the third floor"}))
        .await;

    let results = client
        .search(json!({"query": "how should I structure my typescript imports?", "limit": 3}))
        .await;

    let found = contents(&results);
    assert!(
        found
            .first()
            .is_some_and(|top| top.contains("barrel files")),
        "expected the imports preference first, got {found:?}"
    );
}

#[tokio::test]
async fn recall_finds_an_exact_identifier_a_vector_alone_would_blur() {
    // project-plan.md §7.7: the case that justifies the keyword leg.
    let client = Client::spawn().await;

    client
        .save(json!({"content": "The useQuery cache key must always include the tenant id"}))
        .await;
    client
        .save(json!({"content": "Prefer useState over useReducer for simple local state"}))
        .await;
    client
        .save(json!({"content": "Prefer useEffect cleanup functions to cancel in-flight work"}))
        .await;

    let results = client
        .search(json!({"query": "useQuery", "limit": 3}))
        .await;

    let found = contents(&results);
    assert!(
        found.first().is_some_and(|top| top.contains("useQuery")),
        "the exact identifier should rank first, got {found:?}"
    );
}

#[tokio::test]
async fn a_search_hit_explains_which_leg_found_it() {
    let client = Client::spawn().await;
    client
        .save(json!({"content": "User prefers pnpm as their package manager"}))
        .await;

    let results = client.search(json!({"query": "pnpm"})).await;

    let hit = &results["results"][0];
    assert!(hit["score"].as_f64().unwrap() > 0.0);
    let matched = &hit["matched"];
    assert!(
        matched["vector_rank"].is_number() || matched["bm25_rank"].is_number(),
        "a hit should say how it was found: {matched}"
    );
    assert!(results["took_ms"].is_number());
}

#[tokio::test]
async fn search_filters_by_category() {
    let client = Client::spawn().await;
    client
        .save(json!({
            "content": "We chose SQLite over Postgres because installer size matters more than concurrent writes",
            "category": "decision"
        }))
        .await;
    client
        .save(json!({"content": "We chose pnpm over npm", "category": "preference.coding"}))
        .await;

    let results = client
        .search(json!({"query": "we chose", "categories": ["decision"]}))
        .await;

    let found = contents(&results);
    assert_eq!(found.len(), 1, "got {found:?}");
    assert!(found[0].contains("SQLite"));
}

#[tokio::test]
async fn search_filters_by_tag() {
    let client = Client::spawn().await;
    client
        .save(json!({"content": "uses typescript with react", "tags": ["typescript", "react"]}))
        .await;
    client
        .save(json!({"content": "uses typescript on the server", "tags": ["typescript"]}))
        .await;

    let results = client
        .search(json!({"query": "uses typescript", "tags": ["react"]}))
        .await;

    assert_eq!(contents(&results), vec!["uses typescript with react"]);
}

#[tokio::test]
async fn a_memory_can_be_read_edited_and_forgotten() {
    let client = Client::spawn().await;
    let saved = client
        .save(json!({"content": "deploys on flyio", "category": "fact.project"}))
        .await;
    let id = saved["id"].as_str().unwrap();

    let fetched: Value = client
        .http
        .get(format!("{}/v1/memories/{id}", client.app.base_url))
        .bearer_auth(&client.key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fetched["content"], "deploys on flyio");

    let updated: Value = client
        .http
        .patch(format!("{}/v1/memories/{id}", client.app.base_url))
        .bearer_auth(&client.key)
        .json(&json!({"content": "deploys on hetzner"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["content"], "deploys on hetzner");

    // The edit must reach the indexes, not just the row.
    let results = client.search(json!({"query": "hetzner"})).await;
    assert_eq!(contents(&results), vec!["deploys on hetzner"]);

    let deleted = client
        .http
        .delete(format!("{}/v1/memories/{id}", client.app.base_url))
        .bearer_auth(&client.key)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);

    let after = client.search(json!({"query": "hetzner"})).await;
    assert!(contents(&after).is_empty(), "a forgotten memory came back");
}

#[tokio::test]
async fn a_forgotten_memory_remains_in_the_audit_trail() {
    let client = Client::spawn().await;
    let saved = client.save(json!({"content": "temporary note"})).await;
    let id = saved["id"].as_str().unwrap().to_string();

    client
        .http
        .delete(format!("{}/v1/memories/{id}", client.app.base_url))
        .bearer_auth(&client.key)
        .send()
        .await
        .unwrap();

    let audit: Value = client
        .http
        .get(format!("{}/v1/audit", client.app.base_url))
        .bearer_auth(&client.key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let entries = audit["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry["memory_id"] == id && entry["operation"] == "delete"),
        "the deletion should be recorded: {audit}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["memory_id"] == id && entry["operation"] == "add"),
        "the creation should still be recorded"
    );
}

#[tokio::test]
async fn memories_export_as_markdown_and_json() {
    let client = Client::spawn().await;
    client
        .save(json!({"content": "User prefers pnpm", "category": "preference.coding"}))
        .await;
    client
        .save(json!({"content": "We chose SQLite", "category": "decision"}))
        .await;

    let markdown = client
        .http
        .get(format!("{}/v1/memories/export", client.app.base_url))
        .bearer_auth(&client.key)
        .send()
        .await
        .unwrap();
    assert!(
        markdown
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/markdown")
    );
    let markdown = markdown.text().await.unwrap();
    assert!(markdown.contains("## decision"), "{markdown}");
    assert!(markdown.contains("User prefers pnpm"));

    let json_export: Value = client
        .http
        .get(format!(
            "{}/v1/memories/export?format=json",
            client.app.base_url
        ))
        .bearer_auth(&client.key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(json_export["memories"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_requests_are_rejected_with_a_readable_error() {
    let client = Client::spawn().await;

    let cases = [
        (json!({"content": ""}), "empty content"),
        (
            json!({"content": "x", "category": "nonsense"}),
            "bad category",
        ),
    ];

    for (body, description) in cases {
        let response = client
            .http
            .post(format!("{}/v1/memories:direct", client.app.base_url))
            .bearer_auth(&client.key)
            .json(&body)
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 400, "{description} should be rejected");
        let error: Value = response.json().await.unwrap();
        assert_eq!(error["error"]["code"], "validation_failed");
    }
}

#[tokio::test]
async fn a_malformed_id_is_a_bad_request_and_a_missing_one_is_not_found() {
    let client = Client::spawn().await;

    let malformed = client
        .http
        .get(format!("{}/v1/memories/not-a-uuid", client.app.base_url))
        .bearer_auth(&client.key)
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);

    let missing = client
        .http
        .get(format!(
            "{}/v1/memories/{}",
            client.app.base_url,
            uuid::Uuid::now_v7()
        ))
        .bearer_auth(&client.key)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn a_read_only_key_cannot_write_and_a_write_only_key_cannot_read() {
    let app = TestApp::spawn().await;
    let reader = app.create_user_with_key("reader", "read");
    let writer = app.create_user_with_key("writer", "write");
    let http = reqwest::Client::new();

    let write_with_read_key = http
        .post(format!("{}/v1/memories:direct", app.base_url))
        .bearer_auth(&reader)
        .json(&json!({"content": "should not be stored"}))
        .send()
        .await
        .unwrap();
    assert_eq!(write_with_read_key.status(), 403);

    let read_with_write_key = http
        .post(format!("{}/v1/memories/search", app.base_url))
        .bearer_auth(&writer)
        .json(&json!({"query": "anything"}))
        .send()
        .await
        .unwrap();
    assert_eq!(read_with_write_key.status(), 403);
}

#[tokio::test]
async fn memory_routes_require_a_credential() {
    let app = TestApp::spawn().await;
    app.create_user_with_key("alex", "read,write");
    let http = reqwest::Client::new();

    let response = http
        .post(format!("{}/v1/memories/search", app.base_url))
        .json(&json!({"query": "anything"}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
}
