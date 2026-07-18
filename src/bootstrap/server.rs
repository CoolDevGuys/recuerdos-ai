//! axum router, graceful shutdown, tracing init. The observability
//! baseline every later phase's routes build on.

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initializes the global tracing subscriber. Call once, before doing
/// anything else. `RECORDAGENT_LOG=json` switches to structured JSON logs
/// (for log aggregators); otherwise logs are human-readable. Standard
/// `RUST_LOG`-style filters apply via the env-filter (default: `info`).
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let json = std::env::var("RECORDAGENT_LOG").as_deref() == Ok("json");

    let registry = tracing_subscriber::registry().with(filter);
    if json {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    }
}

pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn version() -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "git_sha": env!("RECORDAGENT_GIT_SHA"),
    }))
}

/// Binds and serves until a shutdown signal (SIGINT or, on Unix, SIGTERM)
/// arrives, then drains in-flight requests before returning — Docker sends
/// SIGTERM on `docker stop` and expects the process gone well inside its
/// default 10 s grace period.
pub async fn serve(host: &str, port: u16) -> std::io::Result<()> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|e| panic!("invalid [server].host/port {host}:{port}: {e}"));

    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");

    axum::serve(listener, router())
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install SIGINT handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received, draining in-flight requests");
}
