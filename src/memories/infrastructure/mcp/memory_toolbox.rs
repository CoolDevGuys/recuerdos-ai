//! What the MCP tools need, independent of where the work happens.
//!
//! There are two transports and they execute differently:
//!
//! - **Streamable HTTP** (`/mcp` on the daemon) calls the use cases
//!   in-process.
//! - **stdio** (`recuerdos-ai mcp`) is a shim: a per-client process that
//!   forwards to the daemon over localhost HTTP.
//!
//! The tool *definitions* — names, descriptions, argument schemas, output
//! rendering — must be byte-identical across both, or an agent would see
//! a different memory service depending on how it connected. So they are
//! written once against this trait, and only execution differs.
//!
//! # Why stdio is a shim rather than a second engine
//!
//! Running the engine in the stdio process would mean every editor
//! session loading its own copy of the 130 MB ONNX model, and several
//! processes writing the same SQLite file. One daemon, many thin clients,
//! is the only shape that stays correct under an agent that opens four
//! windows.

use crate::shared::error::Result;
use chrono::{DateTime, Utc};

/// A memory as the tools need to render it. Deliberately not the domain
/// `Memory`: the shim receives JSON from the daemon and has no business
/// reconstructing an aggregate it cannot validate.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolMemory {
    pub id: String,
    pub content: String,
    pub category: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub score: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct SaveRequest {
    pub content: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    /// Which client stored it, for the audit trail.
    pub client: Option<String>,
}

/// What a save actually did.
///
/// Not a single `ToolMemory`, because with understanding enabled one
/// submission can become several memories, replace an existing one, or —
/// when the store already knows it — produce none at all. An agent that
/// was told "saved" after a NOOP would report something untrue to the
/// user.
#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub memories: Vec<ToolMemory>,
    /// Whether a language model extracted and reconciled, or the content
    /// was stored as sent.
    pub understanding: bool,
}

#[derive(Debug, Clone)]
pub struct RecallRequest {
    pub query: String,
    pub categories: Vec<String>,
    pub limit: Option<usize>,
}

/// A finished session, handed over to be reduced to what outlives it.
#[derive(Debug, Clone)]
pub struct DistillRequest {
    /// The transcript, or a summary of it.
    pub content: String,
    /// The client's own id for the session.
    pub session_id: Option<String>,
    pub tags: Vec<String>,
}

/// Executes what the MCP tools ask for.
///
/// Async because one implementation is an HTTP client. The in-process one
/// wraps blocking calls in `spawn_blocking`, exactly as the REST handlers
/// do.
#[async_trait::async_trait]
pub trait MemoryToolbox: Send + Sync {
    async fn save(&self, request: SaveRequest) -> Result<SaveOutcome>;

    async fn recall(&self, request: RecallRequest) -> Result<Vec<ToolMemory>>;

    /// Distils a finished session. Returns what survived it — commonly
    /// nothing, which is a correct answer and not an error.
    async fn distill(&self, request: DistillRequest) -> Result<Vec<ToolMemory>>;

    /// Finds deletion candidates. Never deletes — `memory_forget` shows
    /// these and requires a second, explicit call.
    async fn find_candidates(&self, query: &str, limit: usize) -> Result<Vec<ToolMemory>>;

    /// Deletes by id. Ids not belonging to the caller are reported as not
    /// found, never silently skipped.
    async fn forget(&self, ids: &[String]) -> Result<usize>;

    async fn profile(&self) -> Result<String>;
}
