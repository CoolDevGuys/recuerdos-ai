//! Task 0.4 DoD: spawn the app on a random port, assert both endpoints;
//! and prove graceful shutdown on SIGTERM completes well inside Docker's
//! grace period.

mod common;

use common::TestApp;
use std::time::Duration;

#[tokio::test]
async fn healthz_and_version_respond() {
    let app = TestApp::spawn().await;
    let client = reqwest::Client::new();

    let health = client
        .get(format!("{}/healthz", app.base_url))
        .send()
        .await
        .expect("GET /healthz");
    assert_eq!(health.status(), 200);
    let health_body: serde_json::Value = health.json().await.expect("json body");
    assert_eq!(health_body["status"], "ok");

    let version = client
        .get(format!("{}/version", app.base_url))
        .send()
        .await
        .expect("GET /version");
    assert_eq!(version.status(), 200);
    let version_body: serde_json::Value = version.json().await.expect("json body");
    assert_eq!(version_body["version"], env!("CARGO_PKG_VERSION"));
    assert!(version_body["git_sha"].is_string());
}

#[cfg(unix)]
#[tokio::test]
async fn graceful_shutdown_on_sigterm() {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let mut app = TestApp::spawn().await;

    kill(Pid::from_raw(app.pid() as i32), Signal::SIGTERM).expect("send SIGTERM");

    let start = std::time::Instant::now();
    let exited = app.wait_for_exit(Duration::from_secs(3)).await;
    assert!(exited, "process did not exit within 3s of SIGTERM");
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "shutdown took {:?}, expected < 3s",
        start.elapsed()
    );
}
