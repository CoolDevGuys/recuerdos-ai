//! The streamable-HTTP MCP transport, mounted on the daemon at `/mcp`.
//!
//! `tests/mcp_tools.rs` covers the stdio shim. Nothing covered this surface,
//! which is a different shape: a tower service mounted inside the daemon,
//! authenticating lazily per tool call rather than per handler signature.
//! Those two facts together meant an unauthenticated request was *accepted* —
//! a session allocated, `initialize` answered, and only the tool call failing,
//! as a JSON-RPC error, for a caller who never proved who they were.

mod common;

use common::TestApp;
use reqwest::header::{ACCEPT, CONTENT_TYPE, WWW_AUTHENTICATE};
use serde_json::json;

/// A daemon with `[server].mcp.http` on. It is the default, but spelled out:
/// half these tests are about the route existing, and that should not depend
/// on a default someone may later flip.
async fn daemon() -> TestApp {
    TestApp::spawn_with(&[("RECUERDOS_AI_SERVER__MCP__HTTP", "true")]).await
}

fn initialize() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "integration-test", "version": "1"}
        }
    })
}

async fn post_mcp(app: &TestApp, key: Option<&str>) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .post(format!("{}/mcp", app.base_url))
        .header(ACCEPT, "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .json(&initialize());
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    request.send().await.expect("reaching /mcp")
}

#[tokio::test]
async fn an_unauthenticated_mcp_request_is_refused_at_the_http_layer() {
    let app = daemon().await;

    let response = post_mcp(&app, None).await;

    assert_eq!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "an unauthenticated request must not reach the MCP session layer"
    );
    assert_eq!(
        response
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer"),
        "a 401 has to say how to authenticate"
    );

    let body: serde_json::Value = response.json().await.expect("a JSON error body");
    assert_eq!(body["error"]["code"], "unauthorized", "{body}");
    // Specifically not `{"jsonrpc": "2.0", "error": ...}`: this is an HTTP
    // fact about the transport, not a failed tool call.
    assert!(body.get("jsonrpc").is_none(), "{body}");
}

#[tokio::test]
async fn a_bad_key_is_refused_exactly_like_a_missing_one() {
    let app = daemon().await;

    let missing = post_mcp(&app, None).await;
    let wrong = post_mcp(&app, Some("ra_live_0000000000000000000000000000")).await;

    assert_eq!(missing.status(), wrong.status());
    assert_eq!(missing.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_valid_key_completes_the_handshake() {
    // The gate must not be the thing that makes `/mcp` unusable: a real key
    // still gets a real session.
    let app = daemon().await;
    let key = app.create_user_with_key("alex", "read,write");

    let response = post_mcp(&app, Some(&key)).await;

    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "{:?}",
        response.headers()
    );

    // The reply arrives over SSE and the stream stays open, so read one
    // chunk with a timeout rather than the whole body.
    let chunk = tokio::time::timeout(std::time::Duration::from_secs(10), response.text())
        .await
        .expect("the handshake reply arrives")
        .expect("a readable reply");
    assert!(
        chunk.contains("\"result\"") && chunk.contains("protocolVersion"),
        "expected an initialize result, got: {chunk}"
    );
}

#[tokio::test]
async fn mounting_the_gate_leaves_every_other_route_alone() {
    // The gate is merged around one nested service; a mistake there would
    // quietly protect, or expose, the whole API.
    let app = daemon().await;

    let health = reqwest::Client::new()
        .get(format!("{}/healthz", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), reqwest::StatusCode::OK);

    let ping = reqwest::Client::new()
        .get(format!("{}/v1/ping", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(ping.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn turning_the_transport_off_removes_the_route_rather_than_gating_nothing() {
    let app = TestApp::spawn_with(&[("RECUERDOS_AI_SERVER__MCP__HTTP", "false")]).await;

    let response = post_mcp(&app, None).await;

    assert_eq!(
        response.status(),
        reqwest::StatusCode::NOT_FOUND,
        "with the transport off there is nothing behind the gate, so the gate goes too"
    );
}
