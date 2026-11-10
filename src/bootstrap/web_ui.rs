//! The web UI: a static single-page app compiled into the binary and served
//! at [`MOUNT`] on the API port (ai-specs/web-ui-plan.md §3).
//!
//! # What is served, and what is not
//!
//! Only static files: the built `web/dist` bundle. None of them carry data,
//! so none of them need a credential — the page asks for an API key and
//! then talks to `/v1/*` like any other client, under the same scopes. An
//! asset request therefore never touches identity, and the rate limiter lets
//! it through (see `server::rate_limit`): it costs a map lookup.
//!
//! # Why a missing bundle is a 503 and not a build failure
//!
//! The bundle is produced by `pnpm` (`just web-build`), and most of the
//! people and jobs that build this crate — the test suite, the eval, the
//! dev container — never open the UI. Failing the build without `web/dist`
//! would make Node a requirement for all of them. So the embed allows the
//! folder to be missing, and `/ui` then answers `503` with the command
//! that builds it. The release workflow builds the bundle first and checks
//! that it is present.
//!
//! # Client-side routes
//!
//! The app routes in the browser (`/ui/memories`, …). A reload on such a
//! path must return the app, not a 404, so any path under the mount that
//! does not look like a file (its last segment has no `.`) is answered with
//! `index.html`. A missing *file* (`/ui/assets/gone.js`) is still a 404 —
//! serving HTML in place of a script hides the real error behind a parse
//! failure.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use std::borrow::Cow;
use std::sync::Arc;

/// Where the UI is mounted. Fixed rather than configurable: the bundle is
/// built with this base path baked into its asset URLs (`web/vite.config.ts`).
pub const MOUNT: &str = "/ui";

/// Same-origin everything. `connect-src 'self'` is the line that matters
/// most: even if a memory's content ever escaped into markup, the page
/// could not send anything to another host. `'unsafe-inline'` is limited
/// to styles, which the graph renderer and Tailwind set inline; scripts
/// are never inline.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; \
     connect-src 'self'; worker-src 'self' blob:; object-src 'none'; \
     base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

const NOT_BUILT: &str = "The web UI is not included in this build.\n\n\
     Build the bundle with `just web-build` (needs Node and pnpm), then \
     rebuild the daemon.\n";

/// One file of the bundle.
pub struct Asset {
    pub bytes: Cow<'static, [u8]>,
    pub mime: String,
}

/// Where the bundle's files come from: the embedded `web/dist` in the
/// daemon, an in-memory map in tests.
pub trait AssetSource: Send + Sync + 'static {
    /// The file at `path`, relative to the bundle root (`index.html`,
    /// `assets/app-1a2b.js`).
    fn get(&self, path: &str) -> Option<Asset>;
}

/// The bundle compiled into this binary, or an empty source when the `ui`
/// feature is off.
pub fn built_in() -> Arc<dyn AssetSource> {
    #[cfg(feature = "ui")]
    {
        Arc::new(embedded::EmbeddedAssets)
    }
    #[cfg(not(feature = "ui"))]
    {
        Arc::new(NoAssets)
    }
}

/// Whether `path` is one of the UI's (asset) paths — for the rate limiter,
/// which lets these through uncounted.
pub fn is_ui_path(path: &str) -> bool {
    path == MOUNT
        || path
            .strip_prefix(MOUNT)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The UI's routes, ready to merge into the daemon's router.
pub fn routes(assets: Arc<dyn AssetSource>) -> Router {
    Router::new()
        // `/ui` → `/ui/`, so the document's relative URLs resolve under the
        // mount rather than beside it.
        .route(MOUNT, get(|| async { Redirect::permanent("/ui/") }))
        .route("/ui/", get(index))
        .route("/ui/{*path}", get(file))
        .with_state(assets)
}

async fn index(State(assets): State<Arc<dyn AssetSource>>) -> Response {
    serve_index(assets.as_ref())
}

async fn file(State(assets): State<Arc<dyn AssetSource>>, Path(path): Path<String>) -> Response {
    serve(assets.as_ref(), &path)
}

fn serve(assets: &dyn AssetSource, path: &str) -> Response {
    // The embedded source is a map, but in a debug build rust-embed reads
    // from disk: refuse anything that could step outside the bundle, rather
    // than trusting the lookup to.
    // A trailing slash on a client-side route (`/ui/memories/`) is fine; an
    // empty segment inside the path is not.
    let path = path.strip_suffix('/').unwrap_or(path);
    if path
        .split('/')
        .any(|segment| segment == ".." || segment.is_empty())
        || path.contains('\\')
    {
        return with_security_headers(StatusCode::NOT_FOUND.into_response());
    }

    if let Some(asset) = assets.get(path) {
        // Vite fingerprints everything under `assets/` (`app-1a2b3c.js`), so
        // those never change under the same name; anything else (`index.html`,
        // `favicon.svg`) must be revalidated or a deploy would be invisible.
        let cache = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return asset_response(asset, cache);
    }

    let last_segment = path.rsplit('/').next().unwrap_or(path);
    if last_segment.contains('.') {
        return with_security_headers(StatusCode::NOT_FOUND.into_response());
    }

    serve_index(assets)
}

fn serve_index(assets: &dyn AssetSource) -> Response {
    match assets.get("index.html") {
        Some(asset) => asset_response(asset, "no-cache"),
        None => with_security_headers(
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                NOT_BUILT,
            )
                .into_response(),
        ),
    }
}

fn asset_response(asset: Asset, cache_control: &'static str) -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(mime) = HeaderValue::from_str(&asset.mime) {
        headers.insert(header::CONTENT_TYPE, mime);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );

    with_security_headers((StatusCode::OK, headers, asset.bytes.into_owned()).into_response())
}

fn with_security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

#[cfg(feature = "ui")]
mod embedded {
    use super::{Asset, AssetSource};

    #[derive(rust_embed::RustEmbed)]
    #[folder = "web/dist/"]
    #[allow_missing = true]
    struct Bundle;

    pub(super) struct EmbeddedAssets;

    impl AssetSource for EmbeddedAssets {
        fn get(&self, path: &str) -> Option<Asset> {
            Bundle::get(path).map(|file| Asset {
                mime: file.metadata.mimetype().to_string(),
                bytes: file.data,
            })
        }
    }
}

#[cfg(not(feature = "ui"))]
struct NoAssets;

#[cfg(not(feature = "ui"))]
impl AssetSource for NoAssets {
    fn get(&self, _path: &str) -> Option<Asset> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::collections::HashMap;
    use tower::ServiceExt;

    /// An in-memory bundle: path → (mime, body).
    struct FakeAssets(HashMap<&'static str, (&'static str, &'static str)>);

    impl AssetSource for FakeAssets {
        fn get(&self, path: &str) -> Option<Asset> {
            self.0.get(path).map(|(mime, body)| Asset {
                bytes: Cow::Borrowed(body.as_bytes()),
                mime: (*mime).to_string(),
            })
        }
    }

    fn built_bundle() -> Arc<dyn AssetSource> {
        Arc::new(FakeAssets(HashMap::from([
            (
                "index.html",
                ("text/html", "<!doctype html><title>app</title>"),
            ),
            ("assets/app-1a2b.js", ("text/javascript", "console.log(1)")),
            ("favicon.svg", ("image/svg+xml", "<svg/>")),
        ])))
    }

    fn missing_bundle() -> Arc<dyn AssetSource> {
        Arc::new(FakeAssets(HashMap::new()))
    }

    async fn get(assets: Arc<dyn AssetSource>, uri: &str) -> Response {
        routes(assets)
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn header(response: &Response, name: header::HeaderName) -> &str {
        response.headers()[name].to_str().unwrap()
    }

    #[tokio::test]
    async fn the_bare_mount_redirects_to_its_trailing_slash() {
        let response = get(built_bundle(), "/ui").await;

        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(header(&response, header::LOCATION), "/ui/");
    }

    #[tokio::test]
    async fn the_mount_serves_the_app_uncached() {
        let response = get(built_bundle(), "/ui/").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header(&response, header::CONTENT_TYPE), "text/html");
        assert_eq!(header(&response, header::CACHE_CONTROL), "no-cache");
        assert!(body_text(response).await.contains("<title>app</title>"));
    }

    #[tokio::test]
    async fn fingerprinted_assets_are_cached_forever() {
        let response = get(built_bundle(), "/ui/assets/app-1a2b.js").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header(&response, header::CONTENT_TYPE), "text/javascript");
        assert_eq!(
            header(&response, header::CACHE_CONTROL),
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn unfingerprinted_files_are_revalidated() {
        let response = get(built_bundle(), "/ui/favicon.svg").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header(&response, header::CACHE_CONTROL), "no-cache");
    }

    #[tokio::test]
    async fn a_client_side_route_is_answered_with_the_app() {
        let response = get(built_bundle(), "/ui/memories/mem_01J").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.contains("<title>app</title>"));
    }

    #[tokio::test]
    async fn a_missing_file_is_a_404_not_the_app() {
        let response = get(built_bundle(), "/ui/assets/gone-9z.js").await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_path_that_climbs_out_of_the_bundle_is_refused() {
        for uri in [
            "/ui/../Cargo.toml",
            "/ui/assets/../../Cargo.toml",
            "/ui/a//b",
        ] {
            let response = get(built_bundle(), uri).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        }
    }

    #[tokio::test]
    async fn without_a_bundle_the_app_says_how_to_build_it() {
        let response = get(missing_bundle(), "/ui/").await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(body_text(response).await.contains("just web-build"));
    }

    #[tokio::test]
    async fn every_response_carries_the_security_headers() {
        for (assets, uri) in [
            (built_bundle(), "/ui/"),
            (built_bundle(), "/ui/assets/app-1a2b.js"),
            (built_bundle(), "/ui/assets/gone-9z.js"),
            (missing_bundle(), "/ui/"),
        ] {
            let response = get(assets, uri).await;
            let csp = header(&response, header::CONTENT_SECURITY_POLICY);
            assert!(csp.contains("connect-src 'self'"), "{uri}: {csp}");
            assert!(csp.contains("frame-ancestors 'none'"), "{uri}: {csp}");
            assert_eq!(header(&response, header::X_CONTENT_TYPE_OPTIONS), "nosniff");
            assert_eq!(header(&response, header::X_FRAME_OPTIONS), "DENY");
        }
    }

    #[test]
    fn only_paths_under_the_mount_are_ui_paths() {
        assert!(is_ui_path("/ui"));
        assert!(is_ui_path("/ui/"));
        assert!(is_ui_path("/ui/assets/app.js"));
        assert!(!is_ui_path("/uix"));
        assert!(!is_ui_path("/v1/ping"));
        assert!(!is_ui_path("/"));
    }
}
