//! The per-caller request budget.
//!
//! A memory server whose writes run an LLM pipeline cannot leave either
//! endpoint unmetered: one agent in a retry loop is otherwise an unlimited
//! supply of argon2 verifications and model calls. These tests hold the two
//! properties that matter — a caller can only spend its own budget, and the
//! credential-less requests that never get as far as auth are bounded too.

mod common;

use common::TestApp;
use reqwest::header::{HeaderName, HeaderValue, RETRY_AFTER};

/// Not a registered constant in `http`: a name the daemon only reads because
/// proxies write it.
fn x_forwarded_for() -> HeaderName {
    HeaderName::from_static("x-forwarded-for")
}

/// A tight budget: 3 at once, refilling once every 10s. Slow on purpose — a
/// budget that refines itself between two assertions makes a flaky test.
const TIGHT: &[(&str, &str)] = &[
    ("RECUERDOS_AI_RATE_LIMIT__REQUESTS_PER_MINUTE", "6"),
    ("RECUERDOS_AI_RATE_LIMIT__BURST", "3"),
];

async fn daemon(env: &[(&str, &str)]) -> TestApp {
    TestApp::spawn_with(env).await
}

async fn ping(
    app: &TestApp,
    key: Option<&str>,
    headers: Vec<(HeaderName, HeaderValue)>,
) -> reqwest::Response {
    let mut request = reqwest::Client::new().get(format!("{}/v1/ping", app.base_url));
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    for (name, value) in headers {
        request = request.header(name, value);
    }
    request.send().await.expect("reaching the daemon")
}

#[tokio::test]
async fn bursting_past_the_budget_answers_429_with_a_retry_after() {
    let app = daemon(TIGHT).await;
    let key = app.create_user_with_key("alex", "read");

    for _ in 0..3 {
        let response = ping(&app, Some(&key), vec![]).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }

    let response = ping(&app, Some(&key), vec![]).await;

    assert_eq!(
        response.status(),
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "the fourth request in one burst must be refused"
    );
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    assert!(
        retry_after.is_some_and(|seconds| seconds >= 1),
        "a 429 without a usable Retry-After leaves the client guessing: {:?}",
        response.headers()
    );

    let body: serde_json::Value = response.json().await.expect("a JSON error body");
    assert_eq!(body["error"]["code"], "rate_limited", "{body}");
}

#[tokio::test]
async fn the_budget_is_per_caller_rather_than_global() {
    // The whole reason this is not a single global counter: one noisy client
    // must degrade its own throughput, not the daemon's.
    let app = daemon(TIGHT).await;
    let loud = app.create_user_with_key("loud", "read");
    let quiet = app.create_user_with_key("quiet", "read");

    for _ in 0..4 {
        ping(&app, Some(&loud), vec![]).await;
    }

    let response = ping(&app, Some(&quiet), vec![]).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "one caller's burst spent another's budget"
    );
}

#[tokio::test]
async fn credential_less_probing_is_bounded_too() {
    // Requests with no token have no caller to charge, so they share one
    // bucket per address. Without that, the cheapest way to spend the server's
    // CPU is an endless stream of keys that never verify.
    let app = daemon(TIGHT).await;

    let mut statuses = Vec::new();
    for _ in 0..4 {
        statuses.push(ping(&app, None, vec![]).await.status());
    }

    assert_eq!(statuses[0], reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        statuses[3],
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "unauthenticated attempts must stop being free: {statuses:?}"
    );
}

#[tokio::test]
async fn health_checks_do_not_consume_the_budget() {
    // A refused `/healthz` reads as an unhealthy service, and an orchestrator
    // restarts a daemon that was only being limited — the limiter would cause
    // the outage it exists to prevent.
    let app = daemon(TIGHT).await;
    let key = app.create_user_with_key("alex", "read");

    for _ in 0..20 {
        let health = reqwest::Client::new()
            .get(format!("{}/healthz", app.base_url))
            .send()
            .await
            .unwrap();
        assert_eq!(health.status(), reqwest::StatusCode::OK);
    }

    let response = ping(&app, Some(&key), vec![]).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "probing the health endpoint spent a real caller's budget"
    );
}

#[tokio::test]
async fn a_forged_forwarded_header_does_not_open_a_second_budget() {
    // The default: a directly reachable daemon ignores a client-supplied
    // identity. Otherwise every request picks a fresh bucket and the limiter
    // becomes a tool for evading itself.
    let app = daemon(TIGHT).await;

    let forged = |value: &str| {
        vec![(
            x_forwarded_for(),
            HeaderValue::from_str(value).expect("a header value"),
        )]
    };

    let mut statuses = Vec::new();
    for address in ["10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4"] {
        statuses.push(ping(&app, None, forged(address)).await.status());
    }

    assert_eq!(
        statuses[3],
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "each spoofed address got its own budget: {statuses:?}"
    );
}

#[tokio::test]
async fn a_trusted_proxy_still_distinguishes_callers() {
    // The other half: with the header believed, the clients behind one proxy
    // address must not share a bucket, or one busy client behind a shared
    // reverse proxy locks out the rest.
    let mut env = TIGHT.to_vec();
    env.push(("RECUERDOS_AI_RATE_LIMIT__TRUST_PROXY", "true"));
    let app = daemon(&env).await;

    let forged = |value: &str| {
        vec![(
            x_forwarded_for(),
            HeaderValue::from_str(value).expect("a header value"),
        )]
    };

    // Two requests from one address: within the burst of 3.
    ping(&app, None, forged("10.0.0.1")).await;
    ping(&app, None, forged("10.0.0.1")).await;

    let response = ping(&app, None, forged("10.0.0.9")).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a distinct forwarded address should have its own budget"
    );
}

#[tokio::test]
async fn the_default_budget_leaves_a_normal_client_alone() {
    // The default has to be wide enough that nobody tunes it off in
    // irritation. 40 requests is a busy agent's minute, nowhere near a burst.
    let app = TestApp::spawn().await;
    let key = app.create_user_with_key("alex", "read,write");

    let mut statuses = Vec::new();
    for _ in 0..40 {
        statuses.push(ping(&app, Some(&key), vec![]).await.status());
    }

    assert!(
        statuses
            .iter()
            .all(|status| *status == reqwest::StatusCode::OK),
        "the default limit refused an ordinary burst: {:?}",
        statuses
            .iter()
            .filter(|status| **status != reqwest::StatusCode::OK)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn turning_the_limiter_off_refuses_nothing() {
    // The documented escape hatch has to actually work, or the escape hatch is
    // a config key that does not do what it says.
    let mut env = TIGHT.to_vec();
    env.push(("RECUERDOS_AI_RATE_LIMIT__ENABLED", "false"));
    let app = daemon(&env).await;
    let key = app.create_user_with_key("alex", "read");

    for _ in 0..10 {
        let response = ping(&app, Some(&key), vec![]).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }
}

#[tokio::test]
async fn the_mcp_surface_is_counted_like_every_other_route() {
    // `/mcp` is the expensive surface — a session, then an LLM ingest per tool
    // call — so it must not be the one hole in the budget.
    let mut env = TIGHT.to_vec();
    env.push(("RECUERDOS_AI_SERVER__MCP__HTTP", "true"));
    let app = daemon(&env).await;
    let key = app.create_user_with_key("alex", "read,write");

    let mut statuses = Vec::new();
    for _ in 0..5 {
        let response = reqwest::Client::new()
            .post(format!("{}/mcp", app.base_url))
            .bearer_auth(&key)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "t", "version": "1"}
                }
            }))
            .send()
            .await
            .expect("reaching /mcp");
        statuses.push(response.status());
    }

    assert_eq!(statuses[0], reqwest::StatusCode::OK);
    assert_eq!(
        statuses[4],
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "the most expensive surface was unmetered: {statuses:?}"
    );
}
