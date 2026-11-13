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
    /// What it is and who it is for, as one label: `preference · coding ·
    /// me`, `convention · coding · quill` (memory-structure-plan.md §9).
    /// `None` from a daemon that predates structured memories.
    pub scope: Option<String>,
}

impl ToolMemory {
    /// What a rendered line leads with: the scope label, else the category.
    pub fn label(&self) -> &str {
        self.scope.as_deref().unwrap_or(&self.category)
    }
}

/// A project as the project tools show it (plan §6).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolProject {
    pub id: String,
    pub name: String,
    /// Git remotes, folder names and former names that also find it.
    pub aliases: Vec<String>,
    /// Active memories filed under it.
    pub memories: usize,
}

/// What renaming or merging a project did, in the daemon's words.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectChange {
    pub project: ToolProject,
    pub message: String,
}

/// Who or what a memory is true for, as the agent stated it.
#[derive(Debug, Clone, Default)]
pub struct SaveAppliesTo {
    pub target: String,
    pub project_name: Option<String>,
    pub new_project: bool,
}

/// A structured save (ai-specs/memory-structure-plan.md §3.1), as the tool
/// received it. The daemon applies the memory rules; this layer only
/// carries the fields.
#[derive(Debug, Clone, Default)]
pub struct SaveRequest {
    pub content: String,
    pub kind: String,
    pub topic: String,
    pub applies_to: Option<SaveAppliesTo>,
    pub origin: String,
    pub subtopic: Option<String>,
    pub tags: Vec<String>,
    pub valid_until: Option<DateTime<Utc>>,
    /// The id of a stored memory this one replaces (the user changed their
    /// mind).
    pub replaces: Option<String>,
    /// Ids of stored memories the user said this does not replace.
    pub coexists_with: Vec<String>,
    /// Which client stored it, for the audit trail.
    pub client: Option<String>,
}

/// An amendment to a stored memory (`memory_update`). `None` leaves a field
/// alone; `Some(None)` on `subtopic` or `valid_until` clears it.
#[derive(Debug, Clone, Default)]
pub struct UpdateRequest {
    pub id: String,
    pub content: Option<String>,
    pub kind: Option<String>,
    pub topic: Option<String>,
    pub applies_to: Option<SaveAppliesTo>,
    pub subtopic: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    pub valid_until: Option<Option<DateTime<Utc>>>,
    pub goal_status: Option<String>,
    pub origin: Option<String>,
}

/// What an amendment did: the memory as it now is, and the daemon's
/// sentence about it.
#[derive(Debug, Clone)]
pub struct UpdateOutcome {
    pub memory: ToolMemory,
    pub message: Option<String>,
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
    /// The memory was written.
    Stored,
    /// A stored memory in the same scope already says this, so nothing new
    /// was written.
    AlreadyKnown,
    /// The daemon did not say, or said something this client does not know.
    Unknown,
}

impl SaveStatus {
    /// Parses the daemon's `status` field. Anything unrecognised — including
    /// its absence — is [`SaveStatus::Unknown`] rather than a guess.
    pub fn from_wire(value: &serde_json::Value) -> Self {
        match value.as_str() {
            Some("stored") => Self::Stored,
            Some("already_known") => Self::AlreadyKnown,
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
    /// The daemon's own sentence about how it was filed ("Saved as your
    /// personal coding preference …"), to repeat to the user.
    pub message: Option<String>,
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
    /// The daemon's sentence about how the item was filed.
    pub message: Option<String>,
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

#[derive(Debug, Clone, Default)]
pub struct RecallRequest {
    pub query: String,
    pub categories: Vec<String>,
    /// Structure filters, each OR-ed within itself.
    pub kinds: Vec<String>,
    pub topics: Vec<String>,
    pub targets: Vec<String>,
    pub projects: Vec<String>,
    /// The project the caller works in, when the transport has no folder to
    /// detect it from (HTTP MCP). The stdio shim sends its folder's instead.
    pub project: Option<String>,
    /// Rank every scope alike rather than demoting other projects: what a
    /// search for a memory to delete wants, since the user described it.
    pub all_scopes: bool,
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

    /// Amends a stored memory, under the same rules as a save.
    async fn update(&self, request: UpdateRequest) -> Result<UpdateOutcome>;

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

    /// The user's projects.
    async fn list_projects(&self) -> Result<Vec<ToolProject>>;

    /// Renames a project; asked from inside its folder, the folder's remote
    /// is learned too.
    async fn rename_project(&self, name: &str, new_name: &str) -> Result<ProjectChange>;

    /// Folds project `name` into `into`.
    async fn merge_project(&self, name: &str, into: &str) -> Result<ProjectChange>;
}
