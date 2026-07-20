//! The MCP surface: tools and the profile resource, over two transports.
//!
//! See `memory_toolbox.rs` for why the tool definitions are written once
//! against a trait and only execution differs.
//!
//! Phase 3 ships one transport: **stdio**, via the `HttpMemoryToolbox`
//! shim. The streamable-HTTP transport (`/mcp` on the daemon) and its
//! in-process toolbox are deferred — see docs/mcp.md for the reason. The
//! `MemoryToolbox` trait is what makes adding it a matter of one new
//! implementation rather than a second copy of the tools.

pub mod http_memory_toolbox;
pub mod memory_mcp_server;
pub mod memory_toolbox;
pub mod stdio_shim;
pub mod tool_text;
