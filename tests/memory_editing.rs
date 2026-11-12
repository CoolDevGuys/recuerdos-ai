//! The endpoints the web UI's editor stands on (ai-specs/web-ui-plan.md
//! P1), driven over HTTP against the real binary: conditional edits
//! (`ETag` / `If-Match` → 412), undoing a forget (`POST …/restore`), and
//! the category list the editor offers.

mod common;

use common::TestApp;
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Session {
    app: TestApp,
    key: String,
    http: reqwest::Client,
}

impl Session {
    async fn spawn() -> Self {
        let app = TestApp::spawn().await;
        let key = app.create_user_with_key("alex", "read,write");
        Self {
            app,
            key,
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.app.base_url)
    }

    async fn save(&self, content: &str) -> Value {
        let response = self
            .http
            .post(self.url("/v1/memories:direct"))
            .bearer_auth(&self.key)
            .json(&json!({"content": content}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        response.json().await.unwrap()
    }

    async fn patch(&self, id: &str, if_match: Option<&str>, body: Value) -> reqwest::Response {
        let mut request = self
            .http
            .patch(self.url(&format!("/v1/memories/{id}")))
            .bearer_auth(&self.key)
            .json(&body);
        if let Some(tag) = if_match {
            request = request.header("If-Match", tag);
        }
        request.send().await.unwrap()
    }

    async fn status_of(&self, method: reqwest::Method, path: &str) -> StatusCode {
        self.http
            .request(method, self.url(path))
            .bearer_auth(&self.key)
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn search(&self, query: &str) -> Vec<String> {
        let body: Value = self
            .http
            .post(self.url("/v1/memories/search"))
            .bearer_auth(&self.key)
            .json(&json!({"query": query, "min_relevance": 0.0}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        body["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| hit["id"].as_str().unwrap().to_string())
            .collect()
    }
}

fn etag(response: &reqwest::Response) -> String {
    response.headers()["etag"].to_str().unwrap().to_string()
}

#[tokio::test]
async fn an_edit_carrying_the_current_etag_applies_and_returns_the_next_one() {
    let session = Session::spawn().await;
    let saved = session.save("Tests use Jest").await;
    let id = saved["id"].as_str().unwrap();
    let read = session
        .http
        .get(session.url(&format!("/v1/memories/{id}")))
        .bearer_auth(&session.key)
        .send()
        .await
        .unwrap();
    let first = etag(&read);

    let response = session
        .patch(id, Some(&first), json!({"content": "Tests use Vitest"}))
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let next = etag(&response);
    assert_ne!(next, first, "an edit is a new version");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["content"], "Tests use Vitest");
    assert_eq!(
        next,
        format!("\"{}\"", body["updated_at"].as_str().unwrap())
    );
}

#[tokio::test]
async fn an_edit_carrying_a_stale_etag_is_refused_with_412() {
    let session = Session::spawn().await;
    let saved = session.save("Tests use Jest").await;
    let id = saved["id"].as_str().unwrap();
    let stale = format!("\"{}\"", saved["updated_at"].as_str().unwrap());
    // Someone else edits first (an agent, consolidation, another tab).
    let elsewhere = session.patch(id, None, json!({"tags": ["testing"]})).await;
    assert_eq!(elsewhere.status(), StatusCode::OK);

    let response = session
        .patch(id, Some(&stale), json!({"content": "Tests use Vitest"}))
        .await;

    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "precondition_failed");
}

#[tokio::test]
async fn an_unconditional_edit_still_works_for_existing_clients() {
    let session = Session::spawn().await;
    let saved = session.save("Tests use Jest").await;

    let response = session
        .patch(
            saved["id"].as_str().unwrap(),
            None,
            json!({"content": "Tests use Vitest"}),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_forgotten_memory_is_restored_and_recalled_again() {
    let session = Session::spawn().await;
    let saved = session.save("The team deploys on Hetzner").await;
    let id = saved["id"].as_str().unwrap();
    assert_eq!(
        session
            .status_of(reqwest::Method::DELETE, &format!("/v1/memories/{id}"))
            .await,
        StatusCode::NO_CONTENT
    );
    assert!(
        !session
            .search("where does the team deploy")
            .await
            .contains(&id.to_string())
    );

    let response = session
        .http
        .post(session.url(&format!("/v1/memories/{id}/restore")))
        .bearer_auth(&session.key)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"], saved["id"]);
    assert_eq!(body["created_at"], saved["created_at"]);
    assert!(
        session
            .search("where does the team deploy")
            .await
            .contains(&id.to_string())
    );
}

#[tokio::test]
async fn restoring_a_memory_that_is_not_forgotten_is_404() {
    let session = Session::spawn().await;
    let saved = session.save("still here").await;

    let status = session
        .status_of(
            reqwest::Method::POST,
            &format!("/v1/memories/{}/restore", saved["id"].as_str().unwrap()),
        )
        .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn another_user_cannot_restore_my_forgotten_memory() {
    let session = Session::spawn().await;
    let saved = session.save("alex's private memory").await;
    let id = saved["id"].as_str().unwrap();
    session
        .status_of(reqwest::Method::DELETE, &format!("/v1/memories/{id}"))
        .await;
    let sam = session.app.create_user_with_key("sam", "read,write");

    let response = session
        .http
        .post(session.url(&format!("/v1/memories/{id}/restore")))
        .bearer_auth(&sam)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_read_only_key_cannot_restore() {
    let session = Session::spawn().await;
    let saved = session.save("x").await;
    let reader = session.app.create_user_with_key("reader", "read");

    let response = session
        .http
        .post(session.url(&format!(
            "/v1/memories/{}/restore",
            saved["id"].as_str().unwrap()
        )))
        .bearer_auth(&reader)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn categories_list_every_built_in_with_the_callers_counts() {
    let session = Session::spawn().await;
    let saved = session
        .http
        .post(session.url("/v1/memories:direct"))
        .bearer_auth(&session.key)
        .json(&json!({"content": "prefers pnpm", "category": "preference.coding"}))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::CREATED);

    let body: Value = session
        .http
        .get(session.url("/v1/categories"))
        .bearer_auth(&session.key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let categories = body["categories"].as_array().unwrap();
    assert_eq!(categories.len(), 8);
    let coding = categories
        .iter()
        .find(|c| c["id"] == "preference.coding")
        .unwrap();
    assert_eq!(coding["label"], "Coding preference");
    assert_eq!(coding["built_in"], true);
    assert_eq!(coding["count"], 1);
    assert_eq!(
        categories
            .iter()
            .map(|c| c["count"].as_u64().unwrap())
            .sum::<u64>(),
        1
    );
}

/// Structured memories, phase S0 (ai-specs/memory-structure-plan.md §3, §8):
/// every memory the API returns carries its kind, topic, scope and origin,
/// derived from its category until the save path takes them (S1), and
/// re-derived when it is re-filed.
#[tokio::test]
async fn every_memory_carries_its_structure() {
    let session = Session::spawn().await;
    let response = session
        .http
        .post(session.url("/v1/memories:direct"))
        .bearer_auth(&session.key)
        .json(&json!({"content": "User prefers pnpm", "category": "preference.coding"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved: Value = response.json().await.unwrap();

    assert_eq!(saved["kind"], "preference");
    assert_eq!(saved["topic"], "coding");
    assert_eq!(saved["applies_to"], json!({"target": "me"}));
    assert_eq!(saved["origin"], "user_said");
    assert_eq!(saved["needs_review"], true);
    assert!(saved.get("goal_status").is_none(), "only goals have one");

    let id = saved["id"].as_str().unwrap();
    let response = session
        .patch(id, None, json!({"category": "skill", "tags": ["docker"]}))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let refiled: Value = response.json().await.unwrap();

    assert_eq!(refiled["kind"], "procedure");
    assert_eq!(refiled["topic"], "tech", "from the docker tag");
    assert_eq!(refiled["applies_to"], json!({"target": "general"}));
}
