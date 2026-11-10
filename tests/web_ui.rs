//! The web UI's mount on a real daemon: on by default, gone when disabled,
//! outside the API's rate-limit budget, and never a way around auth.
//!
//! Whether the bundle is *in* this build depends on whether `web/dist` was
//! built before the binary (web_ui.rs), so these tests accept either a
//! served app (200) or the "not built" answer (503) for `/ui/` — what they
//! pin down is everything around that: the route exists, it carries the
//! security headers, and it changes nothing about `/v1`.

mod common;

use common::TestApp;
use reqwest::StatusCode;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn the_ui_is_mounted_by_default_with_its_security_headers() {
    let app = TestApp::spawn().await;

    let response = client()
        .get(format!("{}/ui/", app.base_url))
        .send()
        .await
        .unwrap();

    assert!(
        matches!(
            response.status(),
            StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE
        ),
        "unexpected status {}",
        response.status()
    );
    let csp = response.headers()["content-security-policy"]
        .to_str()
        .unwrap();
    assert!(csp.contains("connect-src 'self'"), "{csp}");
}

#[tokio::test]
async fn the_bare_mount_redirects_into_the_app() {
    let app = TestApp::spawn().await;

    let response = client()
        .get(format!("{}/ui", app.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(response.headers()["location"], "/ui/");
}

#[tokio::test]
async fn disabling_the_ui_removes_the_routes() {
    let app = TestApp::spawn_with(&[("RECUERDOS_AI_UI__ENABLED", "false")]).await;

    let ui = client()
        .get(format!("{}/ui/", app.base_url))
        .send()
        .await
        .unwrap();
    let unknown = client()
        .get(format!("{}/no-such-route/", app.base_url))
        .send()
        .await
        .unwrap();

    // Answered exactly like a path that never existed (the API asks for a
    // credential before it says whether a path exists), and with none of
    // the UI's headers.
    assert_eq!(ui.status(), unknown.status());
    assert!(!ui.headers().contains_key("content-security-policy"));
}

#[tokio::test]
async fn the_ui_mount_does_not_open_the_api_without_a_key() {
    let app = TestApp::spawn().await;

    let response = client()
        .get(format!("{}/v1/ping", app.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn loading_the_ui_does_not_spend_the_api_budget() {
    // A budget of two requests, refilling at one a minute: a dozen charged
    // requests from one address inside this test would see 429s.
    let app = TestApp::spawn_with(&[
        ("RECUERDOS_AI_RATE_LIMIT__REQUESTS_PER_MINUTE", "1"),
        ("RECUERDOS_AI_RATE_LIMIT__BURST", "2"),
    ])
    .await;
    for path in ["/ui/", "/ui/assets/app.js", "/ui/memories", "/ui"] {
        for _ in 0..3 {
            let response = client()
                .get(format!("{}{path}", app.base_url))
                .send()
                .await
                .unwrap();
            assert_ne!(
                response.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "{path} was charged"
            );
        }
    }
}
