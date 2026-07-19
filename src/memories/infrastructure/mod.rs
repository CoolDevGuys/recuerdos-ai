//! Adapters: SQLite + sqlite-vec storage, tantivy full-text index, REST
//! handlers, MCP tools — everything that touches the outside world.

pub mod http;
pub mod mcp;
pub mod sqlite_memory_repository;
pub mod sqlite_vector_index;
pub mod tantivy_text_index;

#[cfg(test)]
mod storage_tests;
