//! `GET /v1/about`, the web UI's About page: the version for every caller,
//! and for an admin where the config came from and the settings in effect —
//! never a secret's value.

mod common;

use common::TestApp;
use reqwest::StatusCode;
use serde_json::Value;

async fn about(app: &TestApp, key: &str) -> (StatusCode, Value) {
    let response = reqwest::Client::new()
        .get(format!("{}/v1/about", app.base_url))
        .bearer_auth(key)
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn every_key_sees_the_version_and_only_an_admin_sees_the_config() {
    let app = TestApp::spawn().await;
    let reader = app.create_user_with_key("alex", "read,write");
    let admin = app.create_user_with_key("ops", "admin");

    let (status, body) = about(&app, &reader).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["user"], "alex");
    assert_eq!(body["auth_mode"], "api-key");
    assert!(body["started_at"].is_string(), "{body}");
    assert!(
        body["daemon"].is_null(),
        "config shown to a non-admin: {body}"
    );

    let (status, body) = about(&app, &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let daemon = &body["daemon"];
    assert_eq!(daemon["config_file"]["from"], "none", "{daemon}");
    assert_eq!(daemon["command"][0], "serve", "{daemon}");
    let overrides: Vec<&str> = daemon["overrides"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    assert!(
        overrides.contains(&"RECUERDOS_AI_SERVER__PORT"),
        "{overrides:?}"
    );
    let sections: Vec<&str> = daemon["settings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|section| section["name"].as_str().unwrap())
        .collect();
    assert!(sections.contains(&"retrieval"), "{sections:?}");
}

#[tokio::test]
async fn an_api_key_is_named_by_its_variable_and_its_value_never_leaves() {
    let app = TestApp::spawn_with(&[
        ("RECUERDOS_AI_UNDERSTANDING__PROVIDER", "openai-compat"),
        ("RECUERDOS_AI_UNDERSTANDING__MODEL", "mock"),
        ("RECUERDOS_AI_UNDERSTANDING__BASE_URL", "http://127.0.0.1:9"),
        (
            "RECUERDOS_AI_UNDERSTANDING__API_KEY_ENV",
            "ABOUT_TEST_MODEL_KEY",
        ),
        ("ABOUT_TEST_MODEL_KEY", "sk-about-test-not-a-real-key"),
    ])
    .await;
    let admin = app.create_user_with_key("ops", "admin");

    let (_, body) = about(&app, &admin).await;

    let text = body.to_string();
    assert!(!text.contains("sk-about-test"), "a secret leaked: {text}");
    assert!(text.contains("ABOUT_TEST_MODEL_KEY (set)"), "{text}");
}

#[tokio::test]
async fn without_auth_the_one_user_is_the_operator() {
    let app = TestApp::spawn_with(&[("RECUERDOS_AI_AUTH__MODE", "none")]).await;

    let body: Value = reqwest::get(format!("{}/v1/about", app.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["auth_mode"], "none");
    assert!(body["daemon"].is_object(), "{body}");
}
