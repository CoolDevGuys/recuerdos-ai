//! `GET /v1/map` over HTTP against the real binary and the real embedding
//! model: related memories link, unrelated ones do not, the category
//! summary is right, and nothing crosses users.

mod common;

use common::TestApp;
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn save(
    app: &TestApp,
    key: &str,
    content: &str,
    category: &str,
    subcategory: Option<&str>,
) -> String {
    let response = reqwest::Client::new()
        .post(format!("{}/v1/memories:direct", app.base_url))
        .bearer_auth(key)
        .json(&json!({"content": content, "category": category, "subcategory": subcategory}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn map(app: &TestApp, key: &str, query: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{}/v1/map{query}", app.base_url))
        .bearer_auth(key)
        .send()
        .await
        .unwrap()
}

fn linked(body: &Value, a: &str, b: &str) -> bool {
    body["edges"].as_array().unwrap().iter().any(|edge| {
        edge["kind"] == "similar"
            && ((edge["source"] == a && edge["target"] == b)
                || (edge["source"] == b && edge["target"] == a))
    })
}

#[tokio::test]
async fn related_memories_link_and_the_categories_are_summarised() {
    let app = TestApp::spawn().await;
    let key = app.create_user_with_key("alex", "read,write");
    let vitest = save(
        &app,
        &key,
        "Unit tests are written with Vitest",
        "preference.coding",
        Some("testing"),
    )
    .await;
    let tdd = save(
        &app,
        &key,
        "Tests are written before the code, test-driven development",
        "preference.coding",
        Some("testing"),
    )
    .await;
    let sqlite = save(
        &app,
        &key,
        "We chose SQLite over Postgres for a single-binary deploy",
        "decision",
        None,
    )
    .await;
    let pasta = save(
        &app,
        &key,
        "Favourite dinner is pasta with pesto",
        "preference.personal",
        None,
    )
    .await;

    let response = map(&app, &key, "?k=1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();

    assert_eq!(body["nodes"].as_array().unwrap().len(), 4);
    assert!(linked(&body, &vitest, &tdd), "{}", body["edges"]);
    assert!(!linked(&body, &sqlite, &pasta), "{}", body["edges"]);
    for edge in body["edges"].as_array().unwrap() {
        let weight = edge["weight"].as_f64().unwrap();
        assert!((0.15..=1.0).contains(&weight), "{edge}");
    }

    let coding = body["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "preference.coding")
        .unwrap();
    assert_eq!(coding["label"], "Coding preference");
    assert_eq!(coding["count"], 2);
    assert_eq!(
        coding["subcategories"],
        json!([{"name": "testing", "count": 2}])
    );
    assert!(body["generation"].as_str().is_some());
}

#[tokio::test]
async fn the_map_never_shows_another_users_memories() {
    let app = TestApp::spawn().await;
    let alex = app.create_user_with_key("alex", "read,write");
    let sam = app.create_user_with_key("sam", "read,write");
    save(&app, &alex, "alex deploys on Hetzner", "fact.project", None).await;
    save(&app, &sam, "sam deploys on Hetzner", "fact.project", None).await;

    let body: Value = map(&app, &alex, "").await.json().await.unwrap();

    let nodes = body["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["content"], "alex deploys on Hetzner");
    assert!(body["edges"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn the_generation_moves_when_a_memory_changes() {
    let app = TestApp::spawn().await;
    let key = app.create_user_with_key("alex", "read,write");
    let id = save(&app, &key, "first", "fact.project", None).await;
    let before: Value = map(&app, &key, "").await.json().await.unwrap();

    reqwest::Client::new()
        .delete(format!("{}/v1/memories/{id}", app.base_url))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    let after: Value = map(&app, &key, "").await.json().await.unwrap();

    assert_ne!(before["generation"], after["generation"]);
    assert!(after["nodes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn k_out_of_range_is_rejected_and_a_write_only_key_cannot_read_the_map() {
    let app = TestApp::spawn().await;
    let key = app.create_user_with_key("alex", "read");
    let writer = app.create_user_with_key("agent", "write");

    assert_eq!(
        map(&app, &key, "?k=0").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        map(&app, &key, "?k=99").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(map(&app, &writer, "").await.status(), StatusCode::FORBIDDEN);
}
