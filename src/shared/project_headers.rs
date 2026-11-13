//! The project a local client says it is working in.
//!
//! The `recuerdos-ai mcp` shim runs inside the project folder, detects the
//! project from its git remote, and sends it with every request; any other
//! client may send the same headers. Shared because every context that turns
//! a request into memories needs it — the direct save, the pipeline, session
//! distillation — and the boundary rules keep them from borrowing it from
//! one another's adapters.

use axum::http::HeaderMap;

/// The project's name: `quill`.
pub const PROJECT_HEADER: &str = "x-recuerdos-project";
/// Its normalised remote: `github.com/acme/quill`.
pub const PROJECT_REMOTE_HEADER: &str = "x-recuerdos-project-remote";

/// The `(name, remote)` the request names, when it names a project.
pub fn project_from(headers: &HeaderMap) -> Option<(String, Option<String>)> {
    let read = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    read(PROJECT_HEADER).map(|name| (name, read(PROJECT_REMOTE_HEADER)))
}
