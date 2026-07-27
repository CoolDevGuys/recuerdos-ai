//! `recuerdos-ai mcp` — the stdio transport.
//!
//! MCP clients spawn a server process per session and talk to it over
//! stdin/stdout. That process is a *shim*: it forwards to the daemon over
//! HTTP rather than opening the database itself.
//!
//! # Why a shim
//!
//! An editor with four windows open means four of these processes. If
//! each ran the engine, that would be four ONNX models resident (~130 MB
//! each) and four writers on one SQLite file. One daemon, many thin
//! clients, is the only arrangement that stays correct and affordable.
//!
//! # Nothing may be written to stdout
//!
//! stdout *is* the protocol channel. A stray `println!` corrupts the JSON
//! stream and the client disconnects with a parse error that points
//! nowhere near the cause. Diagnostics therefore go to stderr, which MCP
//! clients surface as server logs.

use super::http_memory_toolbox::HttpMemoryToolbox;
use super::memory_mcp_server::MemoryMcpServer;
use super::memory_toolbox::MemoryToolbox;
use crate::shared::error::{RaError, Result};
use rmcp::ServiceExt;
use std::sync::Arc;

pub const API_KEY_ENV: &str = "RECUERDOS_AI_API_KEY";
pub const BASE_URL_ENV: &str = "RECUERDOS_AI_URL";
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:7070";

/// Serves MCP over stdio until the client disconnects.
pub async fn serve_stdio(client_name: &str) -> Result<()> {
    let api_key = std::env::var(API_KEY_ENV).map_err(|_| {
        RaError::Validation(format!(
            "{API_KEY_ENV} is not set. Issue a key with `recuerdos-ai key issue \
             --user <handle>` and put it in your MCP client's config for this server."
        ))
    })?;
    let base_url = std::env::var(BASE_URL_ENV).unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());

    let toolbox: Arc<dyn MemoryToolbox> = Arc::new(HttpMemoryToolbox::new(&base_url, &api_key));

    // Fail here rather than on the client's first tool call: an MCP
    // client shows a server that failed to start much more clearly than
    // one that starts and then errors on every request.
    toolbox.profile().await.map_err(|error| {
        RaError::Validation(format!(
            "could not reach the Recuerdos AI daemon at {base_url}: {error}"
        ))
    })?;

    eprintln!("recuerdos-ai mcp: connected to {base_url}");

    let server = MemoryMcpServer::new(toolbox, client_name);
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| RaError::Internal(format!("failed to start the MCP server: {e}")))?;

    running
        .waiting()
        .await
        .map_err(|e| RaError::Internal(format!("MCP server stopped: {e}")))?;

    Ok(())
}
