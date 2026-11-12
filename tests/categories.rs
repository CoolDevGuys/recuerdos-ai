//! Each user's own taxonomy and bulk edits, over HTTP against the real
//! binary (ai-specs/web-ui-plan.md P3): creating, relabelling, renaming,
//! merging and deleting categories, `POST /v1/memories:bulk`, and that a
//! category made in the UI is immediately valid for saving and filtering.

mod common;

use common::TestApp;
use reqwest::{Method, StatusCode};
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

    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.send_as(&self.key, method, path, body).await
    }

    async fn send_as(
        &self,
        key: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.app.base_url))
            .bearer_auth(key);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn save(&self, content: &str, category: &str) -> String {
        let (status, body) = self
            .send(
                Method::POST,
                "/v1/memories:direct",
                Some(common::structured(
                    json!({"content": content, "category": category}),
                )),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["id"].as_str().unwrap().to_string()
    }

    async fn category_of(&self, id: &str) -> String {
        let (_, body) = self
            .send(Method::GET, &format!("/v1/memories/{id}"), None)
            .await;
        body["category"].as_str().unwrap().to_string()
    }

    async fn categories(&self) -> Vec<Value> {
        let (status, body) = self.send(Method::GET, "/v1/categories", None).await;
        assert_eq!(status, StatusCode::OK);
        body["categories"].as_array().unwrap().clone()
    }

    async fn search_in(&self, query: &str, category: &str) -> Vec<String> {
        let (status, body) = self
            .send(
                Method::POST,
                "/v1/memories/search",
                Some(json!({"query": query, "categories": [category]})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| hit["id"].as_str().unwrap().to_string())
            .collect()
    }
}

fn find<'a>(categories: &'a [Value], id: &str) -> Option<&'a Value> {
    categories.iter().find(|c| c["id"] == id)
}

#[tokio::test]
async fn a_created_category_is_immediately_valid_for_saving_and_filtering() {
    let session = Session::spawn().await;
    let (status, _) = session
        .send(
            Method::POST,
            "/v1/memories:direct",
            Some(common::structured(
                json!({"content": "slept 7 hours", "category": "health.sleep"}),
            )),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unknown before it exists"
    );

    let (status, created) = session
        .send(
            Method::POST,
            "/v1/categories",
            Some(json!({"id": "Health.Sleep", "label": "Sleep log"})),
        )
        .await;

    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["id"], "health.sleep");
    assert_eq!(created["label"], "Sleep log");
    assert_eq!(created["built_in"], false);
    let id = session.save("slept 7 hours", "health.sleep").await;
    assert_eq!(
        session.search_in("how did I sleep", "health.sleep").await,
        vec![id]
    );
}

#[tokio::test]
async fn categories_are_per_user() {
    let session = Session::spawn().await;
    session
        .send(
            Method::POST,
            "/v1/categories",
            Some(json!({"id": "homelab"})),
        )
        .await;
    let sam = session.app.create_user_with_key("sam", "read,write");

    let (_, body) = session
        .send_as(&sam, Method::GET, "/v1/categories", None)
        .await;
    let (status, _) = session
        .send_as(
            &sam,
            Method::POST,
            "/v1/memories:direct",
            Some(common::structured(
                json!({"content": "x", "category": "homelab"}),
            )),
        )
        .await;

    assert!(find(body["categories"].as_array().unwrap(), "homelab").is_none());
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn relabelling_a_built_in_is_display_only() {
    let session = Session::spawn().await;
    let id = session.save("prefers pnpm", "preference.coding").await;

    let (status, body) = session
        .send(
            Method::PATCH,
            "/v1/categories/preference.coding",
            Some(json!({"label": "How I code"})),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["category"]["label"], "How I code");
    assert_eq!(body["category"]["default_label"], "Coding preference");
    assert_eq!(body["moved"], 0);
    assert_eq!(session.category_of(&id).await, "preference.coding");
    let (_, map) = session.send(Method::GET, "/v1/map", None).await;
    let coding = find(map["categories"].as_array().unwrap(), "preference.coding").unwrap();
    assert_eq!(
        coding["label"], "How I code",
        "the map shows the user's words"
    );

    // A built-in's id never changes.
    let (status, _) = session
        .send(
            Method::PATCH,
            "/v1/categories/preference.coding",
            Some(json!({"id": "coding"})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn renaming_a_custom_category_refiles_its_members_and_recall_still_finds_them() {
    let session = Session::spawn().await;
    session
        .send(
            Method::POST,
            "/v1/categories",
            Some(json!({"id": "health"})),
        )
        .await;
    let id = session.save("sleeps eight hours a night", "health").await;

    let (status, body) = session
        .send(
            Method::PATCH,
            "/v1/categories/health",
            Some(json!({"id": "health.sleep", "label": "Sleep"})),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["category"]["id"], "health.sleep");
    assert_eq!(body["category"]["label"], "Sleep");
    assert_eq!(body["moved"], 1);
    assert_eq!(session.category_of(&id).await, "health.sleep");
    let categories = session.categories().await;
    assert!(find(&categories, "health").is_none());
    assert_eq!(find(&categories, "health.sleep").unwrap()["count"], 1);
    assert_eq!(
        session
            .search_in("how long do I sleep", "health.sleep")
            .await,
        vec![id]
    );
}

#[tokio::test]
async fn merging_moves_every_member_then_deletes_the_category() {
    let session = Session::spawn().await;
    session
        .send(
            Method::POST,
            "/v1/categories",
            Some(json!({"id": "homelab"})),
        )
        .await;
    let a = session.save("runs proxmox at home", "homelab").await;
    let b = session.save("has a synology nas", "homelab").await;

    let (status, body) = session
        .send(
            Method::POST,
            "/v1/categories/homelab/merge",
            Some(json!({"into": "fact.project"})),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["moved"], 2);
    assert_eq!(body["category"]["id"], "fact.project");
    for id in [&a, &b] {
        assert_eq!(session.category_of(id).await, "fact.project");
    }
    assert!(find(&session.categories().await, "homelab").is_none());
}

#[tokio::test]
async fn a_category_with_members_is_deleted_only_with_a_destination() {
    let session = Session::spawn().await;
    session
        .send(
            Method::POST,
            "/v1/categories",
            Some(json!({"id": "homelab"})),
        )
        .await;
    let id = session.save("runs proxmox", "homelab").await;

    let (refused, body) = session
        .send(Method::DELETE, "/v1/categories/homelab", None)
        .await;
    assert_eq!(refused, StatusCode::CONFLICT, "{body}");

    let (status, body) = session
        .send(Method::DELETE, "/v1/categories/homelab?move_to=skill", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["moved"], 1);
    assert_eq!(session.category_of(&id).await, "skill");

    let (built_in, _) = session
        .send(Method::DELETE, "/v1/categories/decision", None)
        .await;
    assert_eq!(built_in, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_bulk_edit_refiles_and_retags_in_one_request() {
    let session = Session::spawn().await;
    let a = session.save("uses pnpm", "fact.project").await;
    let b = session.save("uses vitest", "fact.project").await;

    let (status, body) = session
        .send(
            Method::POST,
            "/v1/memories:bulk",
            Some(json!({
                "ids": [a, b],
                "set": {"category": "preference.coding", "subcategory": "tooling"},
                "add_tags": ["frontend"],
            })),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], 2);
    for memory in body["memories"].as_array().unwrap() {
        assert_eq!(memory["category"], "preference.coding");
        assert_eq!(memory["subcategory"], "tooling");
        assert_eq!(memory["tags"], json!(["frontend"]));
    }
    let (_, audit) = session.send(Method::GET, "/v1/audit?limit=2", None).await;
    for entry in audit["entries"].as_array().unwrap() {
        assert!(
            entry["detail"]
                .as_str()
                .unwrap()
                .starts_with("bulk edit of 2 memories"),
            "{entry}"
        );
    }
}

#[tokio::test]
async fn a_bulk_edit_naming_someone_elses_memory_changes_nothing() {
    let session = Session::spawn().await;
    let mine = session.save("mine", "fact.project").await;
    let sam = session.app.create_user_with_key("sam", "read,write");
    let (_, theirs) = session
        .send_as(
            &sam,
            Method::POST,
            "/v1/memories:direct",
            Some(common::structured(json!({"content": "theirs"}))),
        )
        .await;
    let theirs = theirs["id"].as_str().unwrap();

    let (status, body) = session
        .send(
            Method::POST,
            "/v1/memories:bulk",
            Some(json!({"ids": [mine, theirs], "set": {"category": "skill"}})),
        )
        .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains(theirs));
    assert_eq!(session.category_of(&mine).await, "fact.project");
}

#[tokio::test]
async fn a_read_only_key_can_list_but_not_change_categories() {
    let session = Session::spawn().await;
    let reader = session.app.create_user_with_key("reader", "read");

    let (listed, _) = session
        .send_as(&reader, Method::GET, "/v1/categories", None)
        .await;
    let (created, _) = session
        .send_as(
            &reader,
            Method::POST,
            "/v1/categories",
            Some(json!({"id": "homelab"})),
        )
        .await;
    let (bulk, _) = session
        .send_as(
            &reader,
            Method::POST,
            "/v1/memories:bulk",
            Some(json!({"ids": [], "set": {"category": "skill"}})),
        )
        .await;

    assert_eq!(listed, StatusCode::OK);
    assert_eq!(created, StatusCode::FORBIDDEN);
    assert_eq!(bulk, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_audit_trail_names_the_client_that_made_a_change() {
    let session = Session::spawn().await;
    let id = session.save("uses pnpm", "fact.project").await;

    let response = session
        .http
        .post(format!("{}/v1/memories:bulk", session.app.base_url))
        .bearer_auth(&session.key)
        .header("X-Recuerdos-Client", "web-ui")
        .json(&json!({"ids": [id], "set": {"category": "skill"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (_, audit) = session.send(Method::GET, "/v1/audit?limit=2", None).await;

    let actors: Vec<&str> = audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["actor"].as_str().unwrap())
        .collect();
    assert_eq!(
        actors,
        ["web-ui", "rest"],
        "a client that does not say stays `rest`"
    );
}
