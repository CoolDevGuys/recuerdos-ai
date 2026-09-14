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
    /// How relevant this is to the query, `0.0..=1.0`, from the daemon.
    ///
    /// `None` for a memory that was not the answer to a search — a save
    /// echo, a distillation.
    ///
    /// This replaced the raw fusion score. That number was built from rank
    /// position, so it landed between `0.01` and `0.03` no matter how well a
    /// memory matched, which taught an agent to distrust every score and
    /// still left it unable to tell a good match from the top of a list of
    /// noise. Relevance is measured from the matches themselves, so a low
    /// one means "nothing here is about your question".
    pub relevance: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct SaveRequest {
    pub content: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    /// Which client stored it, for the audit trail.
    pub client: Option<String>,
}

/// What a save did, in the daemon's own words.
///
/// A local copy rather than the pipeline's enum, for the same reason
/// [`ToolMemory`] is: this layer parses JSON from a process that may be a
/// different version of itself, and an adapter that fails to parse a value
/// it has never seen turns a future improvement into a broken client. An
/// unrecognised status degrades to [`SaveStatus::Unknown`], which renders as
/// the honest "something happened, details above" rather than an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveStatus {
    /// At least one memory was written.
    Stored,
    /// Nothing in the content was durable — small talk, an acknowledgement, a
    /// step in the task.
    NothingDurable,
    /// The store already knew it, so nothing new was written.
    AlreadyKnown,
    /// The store changed without a memory being written — a retraction
    /// deleted, a candidate superseded something. Not a no-op.
    ChangedWithoutStoring,
    /// The daemon did not say, or said something this client does not know.
    Unknown,
}

impl SaveStatus {
    /// Parses the daemon's `outcome` field. Anything unrecognised — including
    /// its absence, from a daemon predating the field — is
    /// [`SaveStatus::Unknown`] rather than a guess.
    pub fn from_wire(value: &serde_json::Value) -> Self {
        match value.as_str() {
            Some("stored") => Self::Stored,
            Some("nothing_durable") => Self::NothingDurable,
            Some("already_known") => Self::AlreadyKnown,
            Some("changed_without_storing") => Self::ChangedWithoutStoring,
            _ => Self::Unknown,
        }
    }
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
    /// Which kind of outcome this was, including which kind of nothing.
    ///
    /// The empty case used to be one indistinguishable blob, and the client
    /// was left inferring — inferring "saved", because that is what it was
    /// hoping for.
    pub status: SaveStatus,
}

/// One item's result inside a batch save.
#[derive(Debug, Clone)]
pub struct BatchItemOutcome {
    /// What this item produced. Empty for a NOOP or a failure — the two are
    /// told apart by `error`.
    pub memories: Vec<ToolMemory>,
    /// Set when the item failed, carrying why. `None` for a success, even
    /// one that stored nothing.
    pub error: Option<String>,
    /// What the item did, or `None` when it failed and `error` explains.
    pub status: Option<SaveStatus>,
}

/// What a batch save did, one entry per submitted item, in order.
#[derive(Debug, Clone)]
pub struct BatchSaveOutcome {
    pub items: Vec<BatchItemOutcome>,
    /// Whether a language model extracted and reconciled, or the content
    /// was stored as sent — a server-wide property, so it sits here rather
    /// than on every item.
    pub understanding: bool,
}

#[derive(Debug, Clone)]
pub struct RecallRequest {
    pub query: String,
    pub categories: Vec<String>,
    /// Signed to match the domain: a caller passing `-5` should meet the
    /// explanation of the allowed range, not a deserializer's.
    pub limit: Option<i64>,
    /// Overrides the daemon's relevance floor for this query. `None` uses
    /// the server default; a surface with a cost to being wrong supplies a
    /// higher one.
    pub min_relevance: Option<f32>,
    /// Read the graph hop as of this instant in valid time (Task 7.3.4).
    /// `None` is "now"; a value asks what was true then. Only the graph
    /// leg is affected.
    pub as_of: Option<DateTime<Utc>>,
}

impl RecallRequest {
    /// The floor `memory_forget` holds its candidates to.
    ///
    /// Somewhat above the `[retrieval].min_relevance` default, because the
    /// two surfaces do not fail alike. Returning a weak result from
    /// `memory_recall` costs a paragraph of the agent's context. Returning one
    /// from `memory_forget` costs a memory the user did not mean to delete,
    /// and deletion is the one operation here that cannot be argued with after
    /// the fact.
    ///
    /// Measured against the default model, an unrelated query tops out at
    /// `0.13` and the weakest genuine answer scores `0.36`, so this sits at
    /// more than twice the noise and just under the real matches: it removes
    /// the garbage without hiding the memory the user was actually asking
    /// about, which a stricter bar would do.
    ///
    /// The cost of the asymmetry is a forget that finds nothing, which is
    /// recoverable in a way that a wrong deletion is not: the tool says so,
    /// and a more specific description is the fix.
    pub const FORGET_MIN_RELEVANCE: f32 = 0.3;
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

    /// Saves several submissions in one call, returning an outcome per
    /// input in order — the NOOPs and the failures included, because an
    /// agent that reported "all saved" over a batch where one item failed
    /// would be telling the user something untrue.
    async fn save_batch(&self, requests: Vec<SaveRequest>) -> Result<BatchSaveOutcome>;

    async fn recall(&self, request: RecallRequest) -> Result<Vec<ToolMemory>>;

    /// Distils a finished session. Returns what survived it — commonly
    /// nothing, which is a correct answer and not an error.
    async fn distill(&self, request: DistillRequest) -> Result<Vec<ToolMemory>>;

    /// Finds deletion candidates. Never deletes — `memory_forget` shows
    /// these and requires a second, explicit call.
    ///
    /// Held to [`RecallRequest::FORGET_MIN_RELEVANCE`], which is stricter
    /// than a plain recall: a candidate here is an invitation to destroy
    /// something.
    async fn find_candidates(&self, query: &str, limit: i64) -> Result<Vec<ToolMemory>>;

    /// Deletes by id. Ids not belonging to the caller are reported as not
    /// found, never silently skipped.
    async fn forget(&self, ids: &[String]) -> Result<usize>;

    async fn profile(&self) -> Result<String>;
}
