//! Axum extractors that turn a bearer key into a `UserContext`.
//!
//! A handler that takes `Authenticated` cannot run unauthenticated, and a
//! handler that takes `WriteAccess` cannot run without the write scope —
//! the guarantee is in the signature, not in a line of code someone has
//! to remember to write at the top of the function body.
//!
//! ```ignore
//! async fn save(WriteAccess(ctx): WriteAccess) -> impl IntoResponse { ... }
//! ```

use crate::bootstrap::state::{AppState, AuthMode};
use crate::identity::domain::scope::Scope;
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::RaError;
use axum::extract::FromRequestParts;
use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Any valid credential.
pub struct Authenticated(pub UserContext);

/// A credential carrying the `read` scope.
///
/// Phase 1 ships no scoped route (`/v1/ping` accepts any valid key), so
/// only the tests below construct these today. Phase 2's memory routes
/// are their first production use: search takes `ReadAccess`, save takes
/// `WriteAccess`. They ship now, tested, so those routes cannot be
/// written without a scope check.
#[allow(dead_code)]
pub struct ReadAccess(pub UserContext);

/// A credential carrying the `write` scope. See [`ReadAccess`].
#[allow(dead_code)]
pub struct WriteAccess(pub UserContext);

impl FromRequestParts<AppState> for Authenticated {
    type Rejection = RaError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authenticate(parts, state).await.map(Authenticated)
    }
}

impl FromRequestParts<AppState> for ReadAccess {
    type Rejection = RaError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let context = authenticate(parts, state).await?;
        context.require(Scope::Read)?;
        Ok(ReadAccess(context))
    }
}

impl FromRequestParts<AppState> for WriteAccess {
    type Rejection = RaError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let context = authenticate(parts, state).await?;
        context.require(Scope::Write)?;
        Ok(WriteAccess(context))
    }
}

/// Authentication for a surface mounted as a *service* rather than as axum
/// handlers — today that is only `/mcp`.
///
/// The MCP transport authenticates lazily: it reads the bearer token out of
/// each tool call and forwards it to the REST API. Without this gate an
/// unauthenticated request to `/mcp` is therefore *accepted* — rmcp
/// allocates a session, replies to `initialize`, and only fails later,
/// inside a tool, where the answer is delivered as a JSON-RPC error even
/// though it is an HTTP fact. Worse, session state is allocated for callers
/// who have not proved who they are, which hands the sizing of that memory
/// to whoever finds the port.
///
/// So the credential is checked here, and the request never reaches rmcp if
/// it fails. The token is verified again by the tool's own loopback call:
/// two argon2 runs per tool call is the deliberate price of having exactly
/// one authentication path, rather than the MCP surface trusting a
/// credential some other layer vouches for.
pub async fn require_authentication(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();
    if let Err(error) = authenticate(&parts, &state).await {
        // `RaError` renders the 401 with its `WWW-Authenticate: Bearer`, so
        // this surface says how to authenticate exactly as the REST routes
        // do.
        return error.into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}

async fn authenticate(parts: &Parts, state: &AppState) -> Result<UserContext, RaError> {
    match state.auth_mode {
        AuthMode::None => {
            let resolver = state.identity.default_user_resolver.clone();
            spawn_blocking(move || resolver.execute()).await
        }
        AuthMode::ApiKey => {
            let raw_key = bearer_token(parts)?;
            let authenticator = state.identity.key_authenticator.clone();

            // argon2 is deliberately slow (tens of ms). Running it on a
            // runtime worker would block every other task on that thread.
            let context = spawn_blocking(move || authenticator.execute(&raw_key)).await?;

            touch_last_used(state, &context);
            Ok(context)
        }
    }
}

/// Records key usage without making the request wait for the write.
fn touch_last_used(state: &AppState, context: &UserContext) {
    let Some(key_id) = context.key_id() else {
        return;
    };
    let keys = state.identity.keys.clone();
    let now = state.identity.clock.now();

    tokio::task::spawn_blocking(move || {
        if let Err(error) = keys.touch_last_used(key_id, now) {
            // Bookkeeping: a failure here must never fail the request the
            // caller actually made.
            tracing::warn!(%error, "failed to record API key usage");
        }
    });
}

async fn spawn_blocking<T, F>(work: F) -> Result<T, RaError>
where
    F: FnOnce() -> Result<T, RaError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| RaError::Internal(format!("authentication task failed: {e}")))?
}

fn bearer_token(parts: &Parts) -> Result<String, RaError> {
    let header = parts
        .headers
        .get(AUTHORIZATION)
        .ok_or_else(missing_credentials)?
        .to_str()
        .map_err(|_| missing_credentials())?;

    // Scheme is case-insensitive per RFC 9110.
    let (scheme, token) = header.split_once(' ').ok_or_else(missing_credentials)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(missing_credentials());
    }

    let token = token.trim();
    if token.is_empty() {
        return Err(missing_credentials());
    }

    Ok(token.to_string())
}

fn missing_credentials() -> RaError {
    // Same wording as a bad key: whether the header was absent or wrong
    // is not information worth handing out.
    RaError::Unauthorized("invalid API key".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::wiring::Identity;
    use crate::shared::sqlite::SqliteDatabase;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::header::WWW_AUTHENTICATE;
    use axum::http::{Request, StatusCode};
    use axum::middleware;
    use axum::routing::get;
    use std::sync::Arc;
    use tower::{ServiceExt, service_fn};

    /// A router exposing one route per extractor, so each can be probed
    /// independently — including the scope rejection that Phase 1 has no
    /// production write route to exercise yet.
    fn app(auth_mode: AuthMode) -> (Router, Arc<Identity>) {
        let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
        let shared_database = Arc::clone(&database);
        let identity = Arc::new(Identity::from_database(Arc::clone(&database)).unwrap());
        // A tmp dir per test app, so the tantivy indexes don't collide.
        let index_dir = tempfile::tempdir().unwrap();
        let memories = Arc::new(
            crate::bootstrap::memories_wiring::Memories::for_test(
                Arc::clone(&database),
                Arc::new(crate::memories::application::fake_embedder::FakeEmbedder::default()),
                index_dir.keep(),
            )
            .unwrap(),
        );
        // Default config: no provider, so this builds the verbatim
        // pipeline and reaches nothing outside the process.
        let understanding = Arc::new(
            crate::bootstrap::understanding_wiring::Understanding::build(
                &crate::bootstrap::config::AppConfig::default(),
                database,
                &memories,
            )
            .unwrap(),
        );
        let consolidation = Arc::new(
            crate::bootstrap::consolidation_wiring::Consolidation::build(
                &crate::bootstrap::config::AppConfig::default(),
                &identity,
                &memories,
                &understanding,
                shared_database,
            )
            .unwrap(),
        );
        let state = AppState {
            identity: Arc::clone(&identity),
            memories,
            understanding,
            consolidation,
            auth_mode,
            rate_limit: crate::bootstrap::config::RateLimitConfig::default(),
            mcp_http: false,
            mcp_allowed_hosts: Vec::new(),
            ingest_timeout: std::time::Duration::from_secs(180),
        };

        let router = Router::new()
            .route(
                "/any",
                get(|Authenticated(ctx): Authenticated| async move { ctx.handle().to_string() }),
            )
            .route(
                "/read",
                get(|ReadAccess(ctx): ReadAccess| async move { ctx.handle().to_string() }),
            )
            .route(
                "/write",
                get(|WriteAccess(ctx): WriteAccess| async move { ctx.handle().to_string() }),
            )
            .merge(gated(state.clone()))
            .with_state(state);

        (router, identity)
    }

    /// A surface mounted as a *service* and gated by
    /// [`require_authentication`], which is how `/mcp` is mounted. It answers
    /// "served" so a test can tell a rejection apart from a pass-through.
    fn gated(state: AppState) -> Router<AppState> {
        Router::new()
            .nest_service(
                "/gated",
                service_fn(|_: axum::extract::Request| async {
                    Ok::<_, std::convert::Infallible>(Response::new(Body::from("served")))
                }),
            )
            .layer(middleware::from_fn_with_state(
                state.clone(),
                require_authentication,
            ))
    }

    fn issue(identity: &Identity, handle: &str, scopes: Vec<Scope>) -> String {
        identity.user_creator.execute(handle, None).unwrap();
        identity
            .api_key_issuer
            .execute(handle, scopes, "test")
            .unwrap()
            .token
            .render()
    }

    async fn get_with(router: &Router, path: &str, header: Option<&str>) -> StatusCode {
        let mut request = Request::builder().uri(path);
        if let Some(value) = header {
            request = request.header(AUTHORIZATION, value);
        }
        router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// Status, `WWW-Authenticate` value, and body of a gate response.
    async fn probe(
        router: &Router,
        path: &str,
        header: Option<&str>,
    ) -> (StatusCode, Option<String>, String) {
        let mut request = Request::builder().method("POST").uri(path);
        if let Some(value) = header {
            request = request.header(AUTHORIZATION, value);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();

        let status = response.status();
        let challenge = response
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();

        (status, challenge, body)
    }

    #[tokio::test]
    async fn the_service_gate_answers_a_missing_credential_with_a_challenge() {
        let (router, _) = app(AuthMode::ApiKey);

        let (status, challenge, body) = probe(&router, "/gated", None).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(challenge.as_deref(), Some("Bearer"), "RFC 9110");
        assert!(
            !body.contains("served"),
            "the gated service ran without a credential: {body}"
        );
    }

    #[tokio::test]
    async fn the_service_gate_answers_a_bad_key_the_same_way() {
        let (router, _) = app(AuthMode::ApiKey);

        let (status, _, body) = probe(&router, "/gated", Some("Bearer ra_live_nonsense")).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!body.contains("served"), "{body}");
    }

    #[tokio::test]
    async fn the_service_gate_lets_a_valid_key_through() {
        let (router, identity) = app(AuthMode::ApiKey);
        let key = issue(&identity, "alex", vec![Scope::Read]);

        let (status, _, body) = probe(&router, "/gated", Some(&format!("Bearer {key}"))).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "served");
    }

    #[tokio::test]
    async fn the_service_gate_passes_single_user_mode_through() {
        // No credential exists to present in `auth_mode = "none"`, so the
        // gate must not become the thing that makes a local `/mcp` unusable.
        let (router, _) = app(AuthMode::None);

        let (status, _, body) = probe(&router, "/gated", None).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "served");
    }

    #[tokio::test]
    async fn a_valid_key_is_accepted() {
        let (router, identity) = app(AuthMode::ApiKey);
        let key = issue(&identity, "alex", vec![Scope::Read, Scope::Write]);

        let status = get_with(&router, "/any", Some(&format!("Bearer {key}"))).await;

        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_missing_header_is_rejected() {
        let (router, _) = app(AuthMode::ApiKey);

        assert_eq!(
            get_with(&router, "/any", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn a_malformed_header_is_rejected() {
        let (router, identity) = app(AuthMode::ApiKey);
        let key = issue(&identity, "alex", vec![Scope::Read]);

        for header in [
            "".to_string(),
            "Bearer".to_string(),
            "Bearer ".to_string(),
            key.clone(),            // no scheme
            format!("Basic {key}"), // wrong scheme
            "Bearer not-a-key".to_string(),
        ] {
            assert_eq!(
                get_with(&router, "/any", Some(&header)).await,
                StatusCode::UNAUTHORIZED,
                "header {header:?} should be rejected"
            );
        }
    }

    #[tokio::test]
    async fn the_bearer_scheme_is_case_insensitive() {
        let (router, identity) = app(AuthMode::ApiKey);
        let key = issue(&identity, "alex", vec![Scope::Read]);

        assert_eq!(
            get_with(&router, "/any", Some(&format!("bearer {key}"))).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_revoked_key_is_rejected() {
        let (router, identity) = app(AuthMode::ApiKey);
        let key = issue(&identity, "alex", vec![Scope::Read]);
        let prefix = &key["ra_live_".len().."ra_live_".len() + 8];
        identity.api_key_revoker.execute(prefix).unwrap();

        assert_eq!(
            get_with(&router, "/any", Some(&format!("Bearer {key}"))).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn a_read_key_cannot_reach_a_write_route() {
        let (router, identity) = app(AuthMode::ApiKey);
        let key = issue(&identity, "alex", vec![Scope::Read]);
        let header = format!("Bearer {key}");

        assert_eq!(
            get_with(&router, "/read", Some(&header)).await,
            StatusCode::OK
        );
        assert_eq!(
            get_with(&router, "/write", Some(&header)).await,
            StatusCode::FORBIDDEN,
            "a read-only key reached a write route"
        );
    }

    #[tokio::test]
    async fn a_write_key_cannot_reach_a_read_route() {
        let (router, identity) = app(AuthMode::ApiKey);
        let key = issue(&identity, "alex", vec![Scope::Write]);
        let header = format!("Bearer {key}");

        assert_eq!(
            get_with(&router, "/write", Some(&header)).await,
            StatusCode::OK
        );
        assert_eq!(
            get_with(&router, "/read", Some(&header)).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn an_admin_key_reaches_every_route() {
        let (router, identity) = app(AuthMode::ApiKey);
        let key = issue(&identity, "alex", vec![Scope::Admin]);
        let header = format!("Bearer {key}");

        for path in ["/any", "/read", "/write"] {
            assert_eq!(
                get_with(&router, path, Some(&header)).await,
                StatusCode::OK,
                "admin was refused {path}"
            );
        }
    }

    #[tokio::test]
    async fn auth_mode_none_admits_requests_without_a_key() {
        let (router, _) = app(AuthMode::None);

        for path in ["/any", "/read", "/write"] {
            assert_eq!(get_with(&router, path, None).await, StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn using_a_key_records_last_used() {
        let (router, identity) = app(AuthMode::ApiKey);
        let key = issue(&identity, "alex", vec![Scope::Read]);
        let prefix = key["ra_live_".len().."ra_live_".len() + 8].to_string();

        assert!(
            identity
                .keys
                .find_by_prefix(&prefix)
                .unwrap()
                .unwrap()
                .last_used_at()
                .is_none()
        );

        get_with(&router, "/any", Some(&format!("Bearer {key}"))).await;

        // The write is deliberately off the request path, so poll for it
        // rather than assuming it landed before the response did.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let recorded = identity
                .keys
                .find_by_prefix(&prefix)
                .unwrap()
                .unwrap()
                .last_used_at()
                .is_some();
            if recorded {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "last_used_at was never recorded"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}
