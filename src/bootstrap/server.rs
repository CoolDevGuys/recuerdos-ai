//! axum router, graceful shutdown, tracing init. The observability
//! baseline every later phase's routes build on.

use crate::bootstrap::state::{AppState, AuthMode};
use crate::consolidation::infrastructure::http as consolidation_http;
use crate::identity::infrastructure::http::authenticated::Authenticated;
use crate::memories::infrastructure::http as memories_http;
use crate::understanding::infrastructure::http as understanding_http;
use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
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
/// anything else. `RECUERDOS_AI_LOG=json` switches to structured JSON logs
/// (for log aggregators); otherwise logs are human-readable. Standard
/// `RUST_LOG`-style filters apply via the env-filter (default: `info`).
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let json = std::env::var("RECUERDOS_AI_LOG").as_deref() == Ok("json");

    let registry = tracing_subscriber::registry().with(filter);
    if json {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        // Unauthenticated by design: a health check that needs a
        // credential is useless to a load balancer or `docker healthcheck`.
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .route("/v1/ping", get(ping))
        // Raw content in, understood memories out — asynchronously.
        .route("/v1/memories", post(understanding_http::handlers::ingest))
        .route("/v1/jobs/{id}", get(understanding_http::handlers::get_job))
        // The escape hatch: store exactly this, no pipeline. For a caller
        // that has already decided what to remember.
        .route(
            "/v1/memories:direct",
            post(memories_http::handlers::save_memory),
        )
        .route(
            "/v1/memories/search",
            post(memories_http::handlers::search_memories),
        )
        .route(
            "/v1/memories/export",
            get(memories_http::handlers::export_memories),
        )
        .route(
            "/v1/memories/{id}",
            get(memories_http::handlers::get_memory)
                .patch(memories_http::handlers::update_memory)
                .delete(memories_http::handlers::forget_memory),
        )
        // A finished session in, the few things that outlive it out.
        .route(
            "/v1/sessions/distill",
            post(consolidation_http::handlers::distill_session),
        )
        .route(
            "/v1/profile",
            get(consolidation_http::handlers::read_profile),
        )
        .route("/v1/audit", get(memories_http::handlers::read_audit))
        .with_state(state)
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

/// Temporary: proves authentication end-to-end until Phase 2 gives the
/// API real authenticated routes, at which point this is removed.
async fn ping(Authenticated(context): Authenticated) -> Json<Value> {
    Json(json!({
        "user": context.handle(),
        "scopes": context.scopes().iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    }))
}

async fn version() -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "git_sha": env!("RECUERDOS_AI_GIT_SHA"),
    }))
}

/// Binds and serves until a shutdown signal (SIGINT or, on Unix, SIGTERM)
/// arrives, then drains in-flight requests before returning — Docker sends
/// SIGTERM on `docker stop` and expects the process gone well inside its
/// default 10 s grace period.
pub async fn serve(host: &str, port: u16, state: AppState) -> std::io::Result<()> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|e| panic!("invalid [server].host/port {host}:{port}: {e}"));

    if state.auth_mode == AuthMode::None {
        // Loud on purpose: anyone who can reach this port is the `default`
        // user, so an operator must never discover this setting by
        // accident.
        tracing::warn!(
            "[auth].mode = \"none\": authentication is DISABLED and every \
             request runs as the built-in `default` user. Only do this on a \
             host where the listen address is not reachable by others."
        );
    }

    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, auth_mode = ?state.auth_mode, "listening");

    // Mounted here rather than in `router` so it sits *outside* the 30 s
    // request-timeout layer: an MCP session's connection is long-lived and
    // must not be cut at 30 s. It forwards to the daemon's own REST over
    // loopback, so it needs no state — only the port.
    let mut app = router(state.clone());
    if state.mcp_http {
        app = app.nest_service(
            "/mcp",
            crate::memories::infrastructure::mcp::http_service::http_service(
                format!("http://127.0.0.1:{port}"),
                state.mcp_allowed_hosts.clone(),
            ),
        );
        tracing::info!(
            allowed_hosts = ?state.mcp_allowed_hosts,
            "MCP over streamable HTTP mounted at /mcp"
        );
    }

    axum::serve(listener, app)
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
