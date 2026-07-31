//! The streamable-HTTP MCP transport, mounted at `/mcp` on the daemon.
//!
//! Unlike the stdio shim (a separate process per client), this is part of
//! the daemon itself — an MCP client connects straight to
//! `http://<daemon>/mcp` with its API key as a bearer token, no local
//! `recuerdos-ai` binary required. That is the difference that lets a
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

/// The loopback hosts the DNS-rebinding guard accepts out of the box (the
/// same set rmcp defaults to). Configured `allowed_hosts` are added to
/// these rather than replacing them, so the local shim and local testing
/// keep working even after a deployment opens up its own hostname.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// Builds the tower service to mount at `/mcp`.
///
/// `loopback_base` is the daemon's own address (e.g.
/// `http://127.0.0.1:7070`); each session's server forwards there.
///
/// `allowed_hosts` extends the `Host`-header allow-list of the MCP
/// DNS-rebinding guard beyond the loopback defaults
/// ([`LOOPBACK_HOSTS`]) — see [`mcp_config`]. Empty keeps it loopback-only
/// (a reverse proxy rewrites `Host`); a `"*"` entry disables the guard.
pub fn http_service(
    loopback_base: String,
    allowed_hosts: Vec<String>,
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
        mcp_config(&allowed_hosts),
    )
}

/// Turns the configured `allowed_hosts` into the streamable-HTTP server
/// config, applying the DNS-rebinding-guard policy:
///
/// - empty      → loopback-only (rmcp's default), the safe baseline;
/// - `["*"]`    → guard disabled (accept any `Host`) — trusted networks only;
/// - otherwise  → loopback defaults **plus** the configured hosts.
///
/// No SSE keep-alive pings either way: tool calls are request/response, so
/// a keep-alive would hold a connection open with nothing to say.
fn mcp_config(allowed_hosts: &[String]) -> StreamableHttpServerConfig {
    let config = StreamableHttpServerConfig::default().with_sse_keep_alive(None);

    if allowed_hosts.iter().any(|host| host == "*") {
        // rmcp treats an empty allow-list as "accept any host".
        config.disable_allowed_hosts()
    } else if allowed_hosts.is_empty() {
        config
    } else {
        let hosts = LOOPBACK_HOSTS
            .iter()
            .map(|host| host.to_string())
            .chain(allowed_hosts.iter().cloned());
        config.with_allowed_hosts(hosts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_loopback_only() {
        // The default: exactly the three loopback hosts, nothing added.
        let config = mcp_config(&[]);
        assert_eq!(config.allowed_hosts, LOOPBACK_HOSTS.map(String::from));
    }

    #[test]
    fn configured_hosts_are_added_to_the_loopback_defaults() {
        let config = mcp_config(&["memory.example.com".to_string()]);
        // Loopback still works (local shim/testing) and the new host is allowed.
        assert!(config.allowed_hosts.contains(&"localhost".to_string()));
        assert!(
            config
                .allowed_hosts
                .contains(&"memory.example.com".to_string())
        );
    }

    #[test]
    fn a_star_disables_the_guard() {
        // rmcp reads an empty allow-list as "accept any Host".
        let config = mcp_config(&["*".to_string()]);
        assert!(config.allowed_hosts.is_empty());
    }
}
