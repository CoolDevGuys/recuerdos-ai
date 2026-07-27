//! The MCP surface: tools and the profile resource, over two transports.
//!
//! See `memory_toolbox.rs` for why the tool definitions are written once
//! against a trait and only execution differs.
//!
//! Two transports: **stdio** (`recuerdos-ai mcp`, a per-client shim) and
//! **streamable HTTP** (`/mcp` on the daemon, see `http_service.rs`).
//! Both drive the same `MemoryMcpServer` over the `HttpMemoryToolbox`, so
//! the tool definitions live once and only how a request is authenticated
//! differs — see `memory_mcp_server::MemoryMcpServer`.

pub mod http_memory_toolbox;
pub mod http_service;
pub mod memory_mcp_server;
pub mod memory_toolbox;
pub mod stdio_shim;
pub mod tool_text;
