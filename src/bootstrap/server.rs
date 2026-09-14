//! axum router, graceful shutdown, tracing init. The observability
//! baseline every later phase's routes build on.

use crate::bootstrap::config::RateLimitConfig;
use crate::bootstrap::state::{AppState, AuthMode};
use crate::consolidation::infrastructure::http as consolidation_http;
use crate::identity::infrastructure::http::authenticated::{self, Authenticated};
use crate::memories::infrastructure::http as memories_http;
use crate::shared::api_error::{ApiErrorBody, ApiErrorDetail};
use crate::shared::rate_limit::{Decision, RateLimiter};
use crate::understanding::infrastructure::http as understanding_http;
use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, RETRY_AFTER};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
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
    // The write routes run the LLM ingest pipeline — extraction plus a
    // reconciliation call per candidate — and a `wait: true` request runs
    // it inline; a batch runs it once per item. That is seconds to minutes
    // on a slow local model, well past what the read routes should ever
    // take, so they carry their own longer timeout. Built as a separate
    // router and merged in *after* the 30s layer below, so the short
    // timeout never reaches them.
    let write_routes = Router::new()
        .route("/v1/memories", post(understanding_http::handlers::ingest))
        .route(
            "/v1/memories/batch",
            post(understanding_http::handlers::ingest_batch),
        )
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            state.ingest_timeout,
        ));

    Router::new()
        // Unauthenticated by design: a health check that needs a
        // credential is useless to a load balancer or `docker healthcheck`.
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .route("/v1/ping", get(ping))
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
        // The 30s cap covers only the routes added above it. The write
        // routes are merged in afterwards so they keep their own longer
        // timeout rather than being clamped to this one.
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .merge(write_routes)
        .with_state(state)
        // Observability wraps everything, both timeout groups alike.
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
        // Gated rather than trusted to the transport's own lazy check: an
        // unauthenticated request must be answered with a 401 at the HTTP
        // layer, not with a session and a later JSON-RPC error. Merged in
        // rather than layered on `app`, which would gate every route.
        let mcp = Router::new()
            .nest_service(
                "/mcp",
                crate::memories::infrastructure::mcp::http_service::http_service(
                    format!("http://127.0.0.1:{port}"),
                    state.mcp_allowed_hosts.clone(),
                ),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                authenticated::require_authentication,
            ))
            .with_state(state.clone());
        app = app.merge(mcp);
        tracing::info!(
            allowed_hosts = ?state.mcp_allowed_hosts,
            "MCP over streamable HTTP mounted at /mcp"
        );
    }

    // One limiter for the process: buckets are per caller, so nothing is lost
    // by sharing it, and rebuilding it per router would reset everyone's spend.
    //
    // Wrapped around the whole app rather than `router`, so `/mcp` is counted
    // with everything else — it is the surface whose worst case (an LLM ingest
    // per call) costs the most. Being *outermost* is also the point: a request
    // refused for volume should not first pay for routing, tracing, or an
    // argon2 verification, which is the expensive thing an unauthenticated
    // caller can make this server do.
    let app = match Gate::new(&state.rate_limit) {
        Some(gate) => app.layer(middleware::from_fn_with_state(gate.clone(), rate_limit)),
        None => {
            tracing::info!("[rate_limit].enabled = false: no request budget is enforced");
            app
        }
    };

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
}

/// What the rate-limit gate needs: the process's buckets, and how much of
/// `X-Forwarded-For` to believe.
#[derive(Clone)]
struct Gate {
    limiter: Arc<RateLimiter>,
    trust_proxy: bool,
}

impl Gate {
    /// `None` when `[rate_limit].enabled` is false.
    ///
    /// The switch is honoured by not building the gate at all, in one place,
    /// rather than by a branch inside it that every future reader has to find
    /// and preserve.
    fn new(config: &RateLimitConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }

        tracing::info!(
            requests_per_minute = config.requests_per_minute,
            burst = config.burst,
            trust_proxy = config.trust_proxy,
            "rate limiting per caller"
        );
        Some(Self {
            limiter: Arc::new(RateLimiter::new(config.requests_per_minute, config.burst)),
            trust_proxy: config.trust_proxy,
        })
    }
}

/// Refuses a caller that has spent its budget, before anything cheaper or more
/// expensive than a HashMap lookup happens.
///
/// `/healthz` is exempt. A container orchestrator's probe is the one client
/// whose budget must never run out: a refused health check reads as an
/// unhealthy service, and a healthy daemon gets restarted — the limiter would
/// have caused the outage it was preventing.
async fn rate_limit(State(gate): State<Gate>, request: Request, next: Next) -> Response {
    if request.uri().path() == "/healthz" {
        return next.run(request).await;
    }

    let caller = gate.caller(&request);
    let retry_after = match gate.limiter.try_acquire(&caller, Instant::now()) {
        Decision::Allowed => return next.run(request).await,
        Decision::Denied { retry_after } => retry_after,
    };

    // Logged here because a refused request never reaches the tracing layer,
    // which sits inside the router. The caller label is a hash or an address —
    // never the credential itself.
    tracing::warn!(%caller, retry_after = %retry_after.as_secs(), "rate limited");

    let seconds = retry_after.as_secs().max(1);
    let body = Json(ApiErrorBody {
        error: ApiErrorDetail {
            code: "rate_limited",
            message: format!(
                "too many requests: this caller is allowed a burst and then a steady rate;                  retry in {seconds}s"
            ),
        },
    });

    (
        StatusCode::TOO_MANY_REQUESTS,
        [(RETRY_AFTER, seconds.to_string())],
        body,
    )
        .into_response()
}

impl Gate {
    /// Which budget a request spends.
    ///
    /// A bearer token is the primary key because it is what identifies the
    /// caller: hashing it keeps the bucket label from *being* the secret (these
    /// labels go into logs), and separate keys stay separate budgets, so
    /// revoking one does not hand the other its spend.
    ///
    /// With no token — the requests about to be refused with 401, and the
    /// unauthenticated probes — the peer address is all there is. Those share
    /// one bucket per address, which is the right shape for a crowd of
    /// credential-less attempts from one host, and the reason the address must
    /// never come from a header a client controls.
    fn caller(&self, request: &Request) -> String {
        match bearer(request) {
            Some(token) => format!("key:{}", &fingerprint(token)[..12]),
            None => format!("ip:{}", self.peer(request)),
        }
    }

    /// The address the connection came from.
    ///
    /// `X-Forwarded-For` is consulted only when `[rate_limit].trust_proxy` is
    /// on; a directly reachable daemon must not let a request name the bucket
    /// it is counted against, which is how a limiter becomes a way to evade
    /// itself.
    fn peer(&self, request: &Request) -> String {
        if self.trust_proxy {
            if let Some(first) = request
                .headers()
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
            {
                return first.trim().to_string();
            }
        }

        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

fn bearer(request: &Request) -> Option<&str> {
    let header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())?;
    let (scheme, token) = header.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|token| !token.is_empty())
}

fn fingerprint(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(&digest[..6])
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
