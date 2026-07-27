//! The streamable-HTTP MCP transport, mounted at `/mcp` on the daemon.
//!
//! Unlike the stdio shim (a separate process per client), this is part of
//! the daemon itself — an MCP client connects straight to
//! `http://<daemon>/mcp` with its API key as a bearer token, no local
//! `recordagent` binary required. That is the difference that lets a
//! containerised daemon be used from an editor without a `docker exec`
//! wrapper.
//!
//! # How a request is authenticated
//!
//! rmcp's [`StreamableHttpService`] stashes the HTTP request's
//! [`Parts`](http::request::Parts) in each tool call's request context.
//! `MemoryMcpServer::loopback` reads the `Authorization` header from there
//! and forwards to the daemon's own REST API — so the MCP surface is
//! authenticated by exactly the same middleware as every other request,
//! with no second auth path to keep in sync.

use super::memory_mcp_server::MemoryMcpServer;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig;
use std::sync::Arc;

/// Recorded as the source of memories saved over this transport. It
/// cannot tell one HTTP client from another (the connection carries no
/// client name), so all of them share this label — distinct from the
/// stdio shim, which is told its client with `--client`.
const HTTP_CLIENT: &str = "mcp-http";

/// Builds the tower service to mount at `/mcp`.
///
/// `loopback_base` is the daemon's own address (e.g.
/// `http://127.0.0.1:7070`); each session's server forwards there.
pub fn http_service(
    loopback_base: String,
) -> StreamableHttpService<MemoryMcpServer, LocalSessionManager> {
    StreamableHttpService::new(
        // Called once per MCP session. Auth is not known here — it is read
        // per request inside the tool — so every session gets the same
        // loopback-backed server.
        move || {
            Ok(MemoryMcpServer::loopback(
                loopback_base.as_str(),
                HTTP_CLIENT,
            ))
        },
        Arc::new(LocalSessionManager::default()),
        // No SSE keep-alive pings: tool calls are request/response, and a
        // keep-alive would hold a connection open with nothing to say.
        // The default host allow-list (localhost, 127.0.0.1, ::1) is the
        // DNS-rebinding guard the MCP spec asks for and covers the local
        // case; a non-loopback deployment sits behind the operator's own
        // proxy and auth.
        StreamableHttpServerConfig::default().with_sse_keep_alive(None),
    )
}
