//! The cross-tenant suite.
//!
//! This file exists to answer one question, over and over, for the life
//! of the project: **can one user ever reach another user's data?**
//!
//! Phase 1 can only ask it of identity itself — whose key resolves to
//! whom. Every later phase adds its own surface here: Phase 2 asks it of
//! memory recall, Phase 3 of the MCP tools, Phase 4 of the ingest
//! pipeline. When a new read path ships without a case in this file, the
//! isolation claim in project-plan.md §11 has quietly stopped being
//! tested.
//!
//! The `two_users` harness below is the shared entry point for all of it.

mod common;

use common::TestApp;

/// Two users on one server, each with a key — the setup every
/// cross-tenant assertion starts from.
struct TwoUsers {
    app: TestApp,
    alex_key: String,
    sam_key: String,
}

async fn two_users() -> TwoUsers {
    let app = TestApp::spawn().await;
    let alex_key = app.create_user_with_key("alex", "read,write");
    let sam_key = app.create_user_with_key("sam", "read,write");

    assert_ne!(alex_key, sam_key, "two users must not share a key");

    TwoUsers {
        app,
        alex_key,
        sam_key,
    }
}

async fn ping_as(app: &TestApp, key: &str) -> serde_json::Value {
    let response = reqwest::Client::new()
        .get(format!("{}/v1/ping", app.base_url))
        .bearer_auth(key)
        .send()
        .await
        .expect("GET /v1/ping");

    assert_eq!(response.status(), 200, "key should authenticate");
    response.json().await.expect("json body")
}

#[tokio::test]
async fn each_key_resolves_to_its_own_owner() {
    let users = two_users().await;

    let alex = ping_as(&users.app, &users.alex_key).await;
    let sam = ping_as(&users.app, &users.sam_key).await;

    assert_eq!(alex["user"], "alex");
    assert_eq!(sam["user"], "sam");
}

#[tokio::test]
async fn a_key_cannot_be_made_to_resolve_to_another_user() {
    let users = two_users().await;

    // Splice Alex's prefix onto Sam's secret and vice versa: the prefix
    // is the public half, so if identity trusted it alone, this would
    // hand an attacker the other account.
    let scheme = "ra_live_";
    let alex_prefix = &users.alex_key[scheme.len()..scheme.len() + 8];
    let sam_secret = &users.sam_key[scheme.len() + 8..];
    let forged = format!("{scheme}{alex_prefix}{sam_secret}");

    let response = reqwest::Client::new()
        .get(format!("{}/v1/ping", users.app.base_url))
        .bearer_auth(&forged)
        .send()
        .await
        .expect("GET /v1/ping");

    assert_eq!(
        response.status(),
        401,
        "a key spliced from two users' halves was accepted"
    );
}

#[tokio::test]
async fn revoking_one_users_key_does_not_affect_another() {
    let users = two_users().await;
    let scheme = "ra_live_";
    let alex_prefix = &users.alex_key[scheme.len()..scheme.len() + 8];

    users.app.revoke_key(alex_prefix);

    let client = reqwest::Client::new();
    let alex = client
        .get(format!("{}/v1/ping", users.app.base_url))
        .bearer_auth(&users.alex_key)
        .send()
        .await
        .unwrap();
    assert_eq!(alex.status(), 401, "revoked key still works");

    let sam = ping_as(&users.app, &users.sam_key).await;
    assert_eq!(
        sam["user"], "sam",
        "revoking one key locked out another user"
    );
}

#[tokio::test]
async fn an_unauthenticated_request_reaches_no_user_at_all() {
    let app = TestApp::spawn().await;
    app.create_user_with_key("alex", "read,write");

    let response = reqwest::Client::new()
        .get(format!("{}/v1/ping", app.base_url))
        .send()
        .await
        .expect("GET /v1/ping");

    assert_eq!(response.status(), 401);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn the_error_envelope_is_stable_for_clients() {
    let app = TestApp::spawn().await;

    let response = reqwest::Client::new()
        .get(format!("{}/v1/ping", app.base_url))
        .bearer_auth("ra_live_deadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .map(|v| v.to_str().unwrap()),
        Some("Bearer"),
    );

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");
    assert!(body["error"]["message"].is_string());
}

#[tokio::test]
async fn scopes_are_carried_from_the_key_that_authenticated() {
    let app = TestApp::spawn().await;
    let read_only = app.create_user_with_key("reader", "read");
    let writer = app.create_user_with_key("writer", "write");

    let reader_scopes = ping_as(&app, &read_only).await;
    assert_eq!(reader_scopes["scopes"], serde_json::json!(["read"]));

    let writer_scopes = ping_as(&app, &writer).await;
    assert_eq!(writer_scopes["scopes"], serde_json::json!(["write"]));
}
