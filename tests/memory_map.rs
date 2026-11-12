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
        .json(&{
            let mut body = common::structured(
                json!({"content": content, "category": category, "subcategory": subcategory}),
            );
            // The map groups by topic: give each category its own.
            body["topic"] = json!(match category {
                "preference.coding" => "coding",
                "preference.personal" => "food",
                _ => "work",
            });
            body
        })
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
async fn related_memories_link_and_the_topics_are_summarised() {
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

    // Grouped by topic (memory-structure-plan.md D13).
    let coding = body["topics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "coding")
        .unwrap();
    assert_eq!(coding["label"], "Coding");
    assert_eq!(coding["count"], 2);
    assert_eq!(
        coding["subtopics"],
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

#[tokio::test]
async fn a_store_above_the_detail_limit_maps_topics_then_one_topic_at_a_time() {
    let app = TestApp::spawn_with(&[("RECUERDOS_AI_UI__MAP_DETAIL_LIMIT", "2")]).await;
    let key = app.create_user_with_key("alex", "read,write");
    let vitest = save(
        &app,
        &key,
        "Unit tests are written with Vitest",
        "preference.coding",
        None,
    )
    .await;
    let jest = save(
        &app,
        &key,
        "Jest was replaced by Vitest for unit tests",
        "preference.coding",
        None,
    )
    .await;
    save(&app, &key, "Chose SQLite for storage", "decision", None).await;

    let whole = map(&app, &key, "").await;
    assert_eq!(whole.status(), StatusCode::OK);
    let whole: Value = whole.json().await.unwrap();
    assert_eq!(whole["detail"], "topics");
    assert_eq!(whole["total"], 3);
    assert!(whole["nodes"].as_array().unwrap().is_empty());
    assert_eq!(whole["topics"][0]["id"], "coding");
    assert_eq!(whole["topics"][0]["count"], 2);
    assert_eq!(whole["topic_edges"].as_array().unwrap().len(), 1);

    let coding: Value = map(&app, &key, "?topic=coding&k=1")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(coding["detail"], "memories");
    assert_eq!(coding["topic"], "coding");
    assert_eq!(coding["total"], 2);
    assert_eq!(coding["nodes"].as_array().unwrap().len(), 2);
    assert!(linked(&coding, &vitest, &jest), "{coding}");

    let blank = map(&app, &key, "?topic=").await;
    assert_eq!(blank.status(), StatusCode::BAD_REQUEST);
}
