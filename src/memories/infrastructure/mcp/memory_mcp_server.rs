//! The MCP server: tools and the `memory://profile` resource.
//!
//! A thin adapter. It parses arguments, calls one [`MemoryToolbox`]
//! method, and renders the result — the same shape as the REST handlers,
//! over a different protocol. No memory rules live here.

use super::memory_toolbox::{DistillRequest, MemoryToolbox, RecallRequest, SaveRequest};
use super::tool_text;
use crate::shared::error::RaError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ListResourcesResult, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use std::sync::Arc;

pub const PROFILE_URI: &str = "memory://profile";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveParams {
    /// The memory, written as a standalone sentence.
    pub content: String,
    /// One of: preference.coding, preference.personal, decision,
    /// fact.project, fact.person, experience, skill, reference.
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecallParams {
    /// The question you actually have.
    pub query: String,
    /// Optional category filter.
    #[serde(default)]
    pub categories: Vec<String>,
    /// How many to return. Defaults to the server's configured limit.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DistillParams {
    /// The session: a summary of what happened, or the transcript itself.
    pub content: String,
    /// Your own id for this session, if you have one.
    pub session_id: Option<String>,
    /// Tags applied to everything the session yields.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ForgetParams {
    /// What to look for. Returns candidates and deletes nothing.
    pub query: Option<String>,
    /// Ids to delete, from a previous `query` call.
    #[serde(default)]
    pub ids: Vec<String>,
    /// Must be true to actually delete.
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Clone)]
pub struct MemoryMcpServer {
    toolbox: Arc<dyn MemoryToolbox>,
    /// Recorded as the memory's source so the audit trail can tell an
    /// agent's writes from a curl.
    client_name: String,
}

#[tool_router]
impl MemoryMcpServer {
    pub fn new(toolbox: Arc<dyn MemoryToolbox>, client_name: &str) -> Self {
        Self {
            toolbox,
            client_name: client_name.to_string(),
        }
    }

    /// Store a durable fact, preference, or decision the user has stated, so it is
    /// available in future sessions.
    ///
    /// Call this when the user says something that stays true after this conversation
    /// ends:
    /// - a preference: "I prefer pnpm", "never use default exports", "always write
    ///   table-driven tests"
    /// - a decision and its reason: "we chose SQLite over Postgres because installer
    ///   size matters more"
    /// - a durable fact: "the backend deploys on Hetzner", "I'm vegetarian"
    /// - something learned the hard way: "the migration failed because the index was
    ///   built concurrently"
    ///
    /// Do NOT call this for transient task detail — what file you are editing, what a
    /// test just printed, or what the user asked you to do next. Those are true for ten
    /// minutes, and storing them buries the things that matter.
    ///
    /// Write the memory as a standalone sentence that will still make sense in six
    /// months with no surrounding conversation. Prefer the user's own wording for
    /// preferences.
    #[tool(name = "memory_save")]
    async fn memory_save(
        &self,
        Parameters(params): Parameters<SaveParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let outcome = self
            .toolbox
            .save(SaveRequest {
                content: params.content,
                category: params.category,
                tags: params.tags,
                client: Some(self.client_name.clone()),
            })
            .await
            .map_err(to_mcp_error)?;

        Ok(CallToolResult::success(vec![ContentBlock::text(
            tool_text::render_saved(&outcome),
        )]))
    }

    /// Search the user's stored memories by meaning and by keyword.
    ///
    /// Call this before assuming anything about how the user works: their tooling,
    /// conventions, architecture decisions, or personal preferences. It is cheap, and
    /// being wrong about a preference the user already stated is expensive.
    ///
    /// Good queries are the question you actually have — "which package manager does
    /// the user prefer", "why did we choose this database", "how should imports be
    /// structured". Exact identifiers work too (`useQuery`, a ticket id), since the
    /// search matches literal tokens as well as meaning.
    ///
    /// Returns the best matches, most relevant first. An empty result means nothing has
    /// been stored on the subject, not that the user has no opinion.
    #[tool(name = "memory_recall")]
    async fn memory_recall(
        &self,
        Parameters(params): Parameters<RecallParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let memories = self
            .toolbox
            .recall(RecallRequest {
                query: params.query,
                categories: params.categories,
                limit: params.limit,
            })
            .await
            .map_err(to_mcp_error)?;

        Ok(CallToolResult::success(vec![ContentBlock::text(
            tool_text::render_recall(&memories),
        )]))
    }

    /// Reduce a finished session to the few things worth carrying into the next one.
    ///
    /// Call this at the end of a working session, or when the conversation is about to
    /// be compacted — pass a summary of what happened. It extracts only what stays true
    /// after this session ends: conventions that were established, decisions and their
    /// reasons, durable facts about the system, and root causes worth not rediscovering.
    ///
    /// Everything about the task itself is deliberately discarded — what was being
    /// built, what got done, what is still failing. Do not pre-filter the summary to
    /// "important" parts; pass what actually happened and let the extraction decide.
    ///
    /// Returning nothing is the normal outcome. Most sessions produce no durable
    /// memories, and that is a success, not a failure to report to the user.
    ///
    /// This is not a replacement for memory_save. Save a preference the moment the user
    /// states it — do not wait for the end of the session to batch it up.
    #[tool(name = "session_distill")]
    async fn session_distill(
        &self,
        Parameters(params): Parameters<DistillParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let memories = self
            .toolbox
            .distill(DistillRequest {
                content: params.content,
                session_id: params.session_id,
                tags: params.tags,
            })
            .await
            .map_err(to_mcp_error)?;

        Ok(CallToolResult::success(vec![ContentBlock::text(
            tool_text::render_distilled(&memories),
        )]))
    }

    /// Delete memories the user no longer wants stored.
    ///
    /// This is a two-step tool, on purpose. Call it first with `query` to see what
    /// matches; it returns candidates and deletes nothing. Then call it again with the
    /// `ids` you want removed and `confirm: true`.
    ///
    /// Only call the second step when the user has actually asked for something to be
    /// forgotten. Do not use it to tidy up, deduplicate, or make room — the user cannot
    /// see what you deleted, and a memory removed by mistake is gone from every future
    /// session.
    ///
    /// If a memory is merely out of date, save the correct version instead; superseding
    /// is safer than deleting.
    #[tool(name = "memory_forget")]
    async fn memory_forget(
        &self,
        Parameters(params): Parameters<ForgetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Deleting is the one irreversible thing an agent can do here, so
        // the two-step is enforced by the server rather than trusted to
        // the model: ids alone are not enough without `confirm`.
        if !params.ids.is_empty() {
            if !params.confirm {
                return Ok(CallToolResult::success(vec![ContentBlock::text(
                    "Nothing deleted. Pass confirm: true along with the ids to \
                     actually delete, and only if the user asked for it."
                        .to_string(),
                )]));
            }

            let deleted = self
                .toolbox
                .forget(&params.ids)
                .await
                .map_err(to_mcp_error)?;
            return Ok(CallToolResult::success(vec![ContentBlock::text(
                tool_text::render_forgotten(deleted),
            )]));
        }

        let Some(query) = params.query else {
            return Err(ErrorData::invalid_params(
                "provide `query` to find candidates, or `ids` with `confirm: true` to delete",
                None,
            ));
        };

        let candidates = self
            .toolbox
            .find_candidates(&query, 10)
            .await
            .map_err(to_mcp_error)?;

        Ok(CallToolResult::success(vec![ContentBlock::text(
            tool_text::render_forget_candidates(&candidates),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for MemoryMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::V_2025_06_18;
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        info.server_info = Implementation::new("recordagent", env!("CARGO_PKG_VERSION"))
            .with_title("RecordAgent memory");
        // Server instructions are the client's framing for the whole
        // server, separate from any one tool's description: this is where
        // "read the profile first" belongs.
        info.instructions = Some(
            "Long-term memory for this user. Read memory://profile at the start of a \
             session for their standing preferences and decisions, call memory_recall \
             before assuming how they work, and call memory_save when they state \
             something that stays true after this conversation."
                .to_string(),
        );
        info
    }

    async fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new(PROFILE_URI, "profile")
                .with_title("Memory profile")
                .with_description(tool_text::PROFILE_DESCRIPTION)
                .with_mime_type("text/markdown"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if request.uri != PROFILE_URI {
            return Err(ErrorData::resource_not_found(
                format!("unknown resource {}", request.uri),
                None,
            ));
        }

        let profile = self.toolbox.profile().await.map_err(to_mcp_error)?;

        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            profile,
            PROFILE_URI,
        )]))
    }
}

/// Maps a domain error to an MCP error.
///
/// Internal detail is dropped for the same reason the REST layer drops it:
/// it names paths and SQL, and here it would land in a model's context
/// window rather than a log.
fn to_mcp_error(error: RaError) -> ErrorData {
    match error {
        RaError::Validation(message) => ErrorData::invalid_params(message, None),
        RaError::NotFound(message) => ErrorData::invalid_params(message, None),
        RaError::Unauthorized(_) | RaError::Forbidden(_) => {
            ErrorData::invalid_request("this RecordAgent API key is not permitted to do that", None)
        }
        RaError::Conflict(message) => ErrorData::invalid_request(message, None),
        RaError::Internal(detail) => {
            tracing::error!(error = %detail, "mcp tool failed");
            ErrorData::internal_error("internal error", None)
        }
    }
}
