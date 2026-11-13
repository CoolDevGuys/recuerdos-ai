//! The MCP server: tools and the `memory://profile` resource.
//!
//! A thin adapter. It parses arguments, calls one [`MemoryToolbox`]
//! method, and renders the result — the same shape as the REST handlers,
//! over a different protocol. No memory rules live here.

use super::http_memory_toolbox::HttpMemoryToolbox;
use super::memory_toolbox::{
    BatchSaveOutcome, DistillRequest, MemoryToolbox, RecallRequest, SaveAppliesTo, SaveOutcome,
    SaveRequest, SaveStatus, ToolMemory, UpdateRequest,
};
use super::tool_text;
use crate::memories::application::structured_memory_saver::DetectedProject;
use crate::shared::error::RaError;
use chrono::{DateTime, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ListResourcesResult, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub const PROFILE_URI: &str = "memory://profile";

/// One memory and its structure. The same shape for `memory_save` and each
/// item of `memory_save_batch` (ai-specs/memory-structure-plan.md §3).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveParams {
    /// The memory, as one standalone sentence that still makes sense months
    /// from now with no conversation around it. Write the subject out ("The
    /// user prefers…", "Quill's API…"), one fact per memory, and never a
    /// secret: store how to reach something (host, user, which key file), not
    /// the password or key itself.
    pub content: String,
    /// What sort of statement it is. Ask yourself:
    /// preference — what the user picks when free to choose (only ever about
    /// the user: target "me");
    /// rule — a hard line never to cross;
    /// convention — how one project agreed to do things (target "project");
    /// decision — a choice made at a point in time, for a reason;
    /// fact — a durable truth about something or someone;
    /// experience — something that happened;
    /// learning — the lesson drawn from it;
    /// procedure — steps to follow again;
    /// reference — a pointer: a URL, a file path, a ticket;
    /// goal — something the user is working towards (stays open until the
    /// user says it is done);
    /// plan — a dated thing ahead (needs valid_until).
    pub kind: String,
    /// What it is about: coding (writing software), tech (machines, servers,
    /// services, configuration outside a codebase), work, family (relatives,
    /// the household), people (everyone else), health, lifestyle (routines,
    /// hobbies), home, finance, travel, food — or one of the user's own topics.
    pub topic: String,
    /// Who or what it is true for. If you are not sure, ask the user before
    /// saving. Omitted, it defaults to the current project when one is known,
    /// else "general" — never "me" — and the reply asks you to confirm.
    pub applies_to: Option<AppliesToParams>,
    /// Where it came from:
    /// user_said — the user stated it in this conversation;
    /// user_confirmed — you proposed it and the user said yes;
    /// inferred — you noticed it yourself (refused for the user's own
    /// preferences: ask them first, then save as user_confirmed);
    /// document — read from a file such as CLAUDE.md or a README (filed under
    /// that file's project).
    pub origin: String,
    /// Optional finer label within the topic, for filtering: "testing",
    /// "sleep".
    pub subtopic: Option<String>,
    /// Optional lowercase keywords for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// When it stops being true: an RFC 3339 time, or a date (YYYY-MM-DD,
    /// meaning through the end of that day, UTC). Required for a plan; not
    /// allowed on a goal.
    pub valid_until: Option<String>,
    /// The id of a stored memory this one replaces, when the user changed
    /// their mind (often after a "conflict" refusal named it). It must apply
    /// to the same scope. The old memory leaves recall and is kept in history.
    pub replaces: Option<String>,
    /// Ids of stored memories the user said this one does not replace: both
    /// hold. Use only after asking the user about a "conflict" refusal.
    #[serde(default)]
    pub coexists_with: Vec<String>,
}

/// An amendment to one stored memory. Every field but `id` is optional, and
/// a field left out is left as it is.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateParams {
    /// The memory's id, from a recall or a save reply.
    pub id: String,
    /// The corrected sentence, when the stored wording was wrong. Same rules
    /// as a save: standalone, one fact, never a secret.
    pub content: Option<String>,
    /// A new kind, as memory_save takes it.
    pub kind: Option<String>,
    /// A new topic, as memory_save takes it.
    pub topic: Option<String>,
    /// Move it to another scope (me, project or general), as memory_save
    /// takes it.
    pub applies_to: Option<AppliesToParams>,
    /// A new subtopic; an empty string clears it.
    pub subtopic: Option<String>,
    /// Replaces the tags.
    pub tags: Option<Vec<String>>,
    /// A new end (RFC 3339 or YYYY-MM-DD); an empty string clears it.
    pub valid_until: Option<String>,
    /// For a goal: "achieved" or "abandoned" closes it, "active" reopens it.
    /// Only the user closes a goal: send origin "user_said" when they told
    /// you, or "user_confirmed" after you asked and they said yes.
    pub goal_status: Option<String>,
    /// Who asked for this change: user_said, user_confirmed, inferred or
    /// document. Required to close a goal.
    pub origin: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectRenameParams {
    /// The project's current name (or a former name, or its git remote).
    pub name: String,
    /// The new name: lowercase letters, digits, '.', '_' or '-', like "quill-app".
    pub new_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectMergeParams {
    /// The project that goes: its memories move, and its name and remotes find the
    /// other one from then on.
    pub name: String,
    /// The project that stays.
    pub into: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AppliesToParams {
    /// me — who the user is and how they like things, everywhere ("my", "I
    /// always", "personally", "in every project").
    /// project — true of one project ("here", "this repo", "we", anything read
    /// from a project's files).
    /// general — the user's world outside any project: machines, files,
    /// services, places, accounts, appointments.
    pub target: String,
    /// The project's name, only with target "project". Inside a project
    /// folder it is detected for you; give it when there is no folder (or to
    /// name another project). Lowercase, like "quill".
    pub project_name: Option<String>,
    /// Set to true only after the user confirmed that a project the server
    /// does not know yet is a new one.
    #[serde(default)]
    pub new_project: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveBatchParams {
    /// The memories to save, each with its own structure, exactly as
    /// `memory_save` takes one.
    pub items: Vec<SaveParams>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecallParams {
    /// The question you actually have.
    pub query: String,
    /// Optional category filter (the older labels; prefer the filters below).
    #[serde(default)]
    pub categories: Vec<String>,
    /// Only these kinds, e.g. ["preference", "rule"] for "how do I like things done".
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Only these topics, e.g. ["coding"].
    #[serde(default)]
    pub topics: Vec<String>,
    /// Only memories that apply to these: "me", "project", "general".
    #[serde(default)]
    pub targets: Vec<String>,
    /// Only these projects' memories, by name.
    #[serde(default)]
    pub projects: Vec<String>,
    /// The project you are working in, when the server cannot tell from your folder
    /// (over HTTP). Its memories rank with the user's own; other projects' rank lower
    /// unless the question names them.
    pub project: Option<String>,
    /// How many to return, at least 1. Defaults to the server's configured limit.
    pub limit: Option<i64>,
    /// Optional point in time (RFC 3339, e.g. "2026-06-01T00:00:00Z") to
    /// read connected memories as of — "what was true before the
    /// migration?". Omit for the current view. Only affects graph-connected
    /// results.
    pub as_of: Option<String>,
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

/// Where a request's toolbox comes from — the one thing that differs
/// between the two transports.
#[derive(Clone)]
enum Backend {
    /// stdio: one toolbox, fixed when the shim starts, already bound to
    /// the operator's key.
    Fixed(Arc<dyn MemoryToolbox>),
    /// Streamable HTTP on the daemon: there is no single user, so the
    /// toolbox is built per request from the bearer token on the request,
    /// forwarding to the daemon's own REST API over loopback. Reusing the
    /// HTTP toolbox means the two transports share one code path and
    /// cannot drift, and auth is whatever the REST layer already enforces.
    Loopback { base_url: Arc<str> },
}

#[derive(Clone)]
pub struct MemoryMcpServer {
    backend: Backend,
    /// Recorded as the memory's source so the audit trail can tell an
    /// agent's writes from a curl.
    client_name: String,
}

#[tool_router]
impl MemoryMcpServer {
    /// A server over a fixed toolbox — the stdio shim, bound to one key.
    pub fn new(toolbox: Arc<dyn MemoryToolbox>, client_name: &str) -> Self {
        Self {
            backend: Backend::Fixed(toolbox),
            client_name: client_name.to_string(),
        }
    }

    /// A server for the daemon's streamable-HTTP transport. Each request's
    /// toolbox is built from that request's bearer token and forwards to
    /// `base_url` (the daemon's own loopback address).
    pub fn loopback(base_url: impl Into<Arc<str>>, client_name: &str) -> Self {
        Self {
            backend: Backend::Loopback {
                base_url: base_url.into(),
            },
            client_name: client_name.to_string(),
        }
    }

    /// The toolbox for this request.
    ///
    /// Fixed transports ignore the context; the loopback transport reads
    /// the `Authorization` header out of the HTTP request parts that the
    /// streamable-HTTP service stashes in the request context. A missing
    /// header yields an empty key, which the REST layer then rejects (or
    /// accepts, under `[auth].mode = "none"`) — the same decision it makes
    /// for any other caller.
    fn toolbox(&self, context: &RequestContext<RoleServer>) -> Arc<dyn MemoryToolbox> {
        match &self.backend {
            Backend::Fixed(toolbox) => Arc::clone(toolbox),
            Backend::Loopback { base_url } => {
                let parts = context.extensions.get::<http::request::Parts>();
                let key = parts
                    .and_then(|parts| parts.headers.get(http::header::AUTHORIZATION))
                    .and_then(|value| value.to_str().ok())
                    .map(strip_bearer)
                    .unwrap_or_default();
                // There is no folder over HTTP, so no project is detected. A
                // client that knows its project says so with the same headers
                // the stdio shim sends (`X-Recuerdos-Project`, optionally
                // `-Remote`), set per project in its MCP config; they are passed
                // on to the REST calls exactly as the shim passes its own.
                let project = parts
                    .and_then(|parts| crate::shared::project_headers::project_from(&parts.headers))
                    .map(|(name, alias)| DetectedProject { name, alias });
                Arc::new(HttpMemoryToolbox::new(base_url, &key).with_project(project))
            }
        }
    }

    /// Store something durable the user has stated, with what it is and who it is true
    /// for, so it is available in future sessions.
    ///
    /// Call this when the user says something that stays true after this conversation
    /// ends: a preference ("I prefer pnpm"), a rule ("never push to main"), a project's
    /// convention, a decision and its reason, a durable fact, something learned the hard
    /// way, a procedure, a goal, or a dated plan.
    ///
    /// Do NOT call this for transient task detail — what file you are editing, what a
    /// test just printed, or what the user asked you to do next. Those are true for ten
    /// minutes, and storing them buries the things that matter.
    ///
    /// Every field is a decision, not a formality: "applies_to" is what keeps one
    /// project's rule out of the user's personal preferences, so choose it from what the
    /// user said, and ask when it is unclear. The reply says how the memory was filed —
    /// repeat it to the user so they can correct it. A refusal says why and what to
    /// change; follow it (often: ask the user) rather than rewording to get past it.
    ///
    /// A save that contradicts a stored memory in the same scope is refused as a
    /// "conflict", naming the stored one. Ask the user which holds: if they changed their
    /// mind, save again with `replaces` set to its id; if the stored one is wrong, fix it
    /// with `memory_update`; if both hold, save again with `coexists_with`. A save the
    /// store already knows is not stored twice, and the reply says so.
    #[tool(name = "memory_save")]
    async fn memory_save(
        &self,
        Parameters(params): Parameters<SaveParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = save_request(params, &self.client_name).map_err(to_mcp_error)?;
        match self.toolbox(&context).save(request).await {
            Ok(outcome) => {
                let text = tool_text::render_saved(&outcome);
                Ok(save_result(&text, &outcome))
            }
            Err(error) => rejected_result(error, "Not saved"),
        }
    }

    /// Save several memories in one call, when the user has stated multiple durable things
    /// at once — wrapping up a session, a message that lists several preferences, or the
    /// rules read from a project's file (origin "document").
    ///
    /// Each item is one standalone memory with its own structure, exactly as `memory_save`
    /// takes it — do not stuff several facts into one item. Items are saved one by one
    /// through the same rules, so one refused item does not stop the rest.
    ///
    /// It reports per item, including any that were refused and why — repeat that back
    /// accurately rather than telling the user all were saved, and fix the refused ones as
    /// their reason says. For a single memory, use `memory_save`.
    #[tool(name = "memory_save_batch")]
    async fn memory_save_batch(
        &self,
        Parameters(params): Parameters<SaveBatchParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let requests = params
            .items
            .into_iter()
            .map(|item| save_request(item, &self.client_name))
            .collect::<Result<Vec<_>, RaError>>()
            .map_err(to_mcp_error)?;

        let outcome = self
            .toolbox(&context)
            .save_batch(requests)
            .await
            .map_err(to_mcp_error)?;

        let text = tool_text::render_saved_batch(&outcome);
        Ok(save_batch_result(&text, &outcome))
    }

    /// Amend a stored memory: correct its wording, move it to another scope, or close a
    /// goal. The memory keeps its id; the audit trail keeps what it said before.
    ///
    /// Use it when the user says a stored memory is wrong ("no, it's mocks, not
    /// mokcs", "that's quill's rule, not mine"), and after a "conflict" refusal when the
    /// user says the stored one should be corrected rather than replaced. When the user
    /// has changed their mind, save the new memory with `replaces` instead, so the old
    /// one stays in history as what they used to think.
    ///
    /// Closing a goal is `goal_status: "achieved"` (or "abandoned") with origin
    /// "user_said" or "user_confirmed". Only the user closes a goal: if you only noticed
    /// it was reached, ask them first. A closed goal leaves recall and the profile.
    ///
    /// The same rules as a save apply to what the memory becomes, and a refusal says
    /// why and what to change.
    #[tool(name = "memory_update")]
    async fn memory_update(
        &self,
        Parameters(params): Parameters<UpdateParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = update_request(params).map_err(to_mcp_error)?;
        match self.toolbox(&context).update(request).await {
            Ok(outcome) => Ok(CallToolResult::success(vec![ContentBlock::text(
                tool_text::render_updated(&outcome),
            )])),
            Err(error) => rejected_result(error, "Not updated"),
        }
    }

    /// List the user's projects: each one's name, what else finds it (git remotes,
    /// folder names, former names) and how many memories it holds.
    ///
    /// Call this when a save is refused with "unknown_project" and you need the known
    /// names, or when the user asks about their projects.
    #[tool(name = "project_list")]
    async fn project_list(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let projects = self
            .toolbox(&context)
            .list_projects()
            .await
            .map_err(to_mcp_error)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(
            tool_text::render_projects(&projects),
        )]))
    }

    /// Rename a project. Its memories follow (they store the project, not its name),
    /// and the old name keeps finding it, so older checkouts still resolve.
    ///
    /// Use it when the user renames a project, or when a save is refused because this
    /// repository is unknown and the reply asks whether it is a rename of a known
    /// project (its remote changed): ask the user, then rename the old project to this
    /// folder's name. Called from inside the folder, its new remote is learned too.
    /// Only rename when the user agrees.
    #[tool(name = "project_rename")]
    async fn project_rename(
        &self,
        Parameters(params): Parameters<ProjectRenameParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .toolbox(&context)
            .rename_project(&params.name, &params.new_name)
            .await
        {
            Ok(change) => Ok(CallToolResult::success(vec![ContentBlock::text(
                tool_text::render_project_change(&change),
            )])),
            Err(error) => refused_result(error, "Not renamed"),
        }
    }

    /// Merge two projects that are one: every memory of `name` moves to `into`, and
    /// `name`, its remotes and folder names find `into` from then on.
    ///
    /// Use it when the user says two projects are the same (a fork created as its own
    /// project, a project saved under two names). It cannot be undone by another call,
    /// so only merge when the user asked for it.
    #[tool(name = "project_merge")]
    async fn project_merge(
        &self,
        Parameters(params): Parameters<ProjectMergeParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        match self
            .toolbox(&context)
            .merge_project(&params.name, &params.into)
            .await
        {
            Ok(change) => Ok(CallToolResult::success(vec![ContentBlock::text(
                tool_text::render_project_change(&change),
            )])),
            Err(error) => refused_result(error, "Not merged"),
        }
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let as_of = params
            .as_of
            .as_deref()
            .map(parse_as_of)
            .transpose()
            .map_err(to_mcp_error)?;

        let memories = self
            .toolbox(&context)
            .recall(RecallRequest {
                query: params.query,
                categories: params.categories,
                kinds: params.kinds,
                topics: params.topics,
                targets: params.targets,
                projects: params.projects,
                project: params.project,
                limit: params.limit,
                // The server's floor: a caller with an opinion about it has
                // the REST surface, and every MCP caller is an agent, which
                // is exactly the audience the floor is tuned for.
                min_relevance: None,
                as_of,
                all_scopes: false,
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let memories = self
            .toolbox(&context)
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let toolbox = self.toolbox(&context);
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

            let deleted = toolbox.forget(&params.ids).await.map_err(to_mcp_error)?;
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

        let candidates = toolbox
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
        info.server_info = Implementation::new("recuerdos-ai", env!("CARGO_PKG_VERSION"))
            .with_title("Recuerdos AI memory");
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
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if request.uri != PROFILE_URI {
            return Err(ErrorData::resource_not_found(
                format!("unknown resource {}", request.uri),
                None,
            ));
        }

        let profile = self
            .toolbox(&context)
            .profile()
            .await
            .map_err(to_mcp_error)?;

        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            profile,
            PROFILE_URI,
        )]))
    }
}

/// Strips a leading `Bearer ` (any case) from an Authorization header,
/// leaving the raw key the toolbox sends back as its own bearer.
fn strip_bearer(header: &str) -> String {
    let trimmed = header.trim();
    trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

/// Maps a domain error to an MCP error.
///
/// Internal detail is dropped for the same reason the REST layer drops it:
/// it names paths and SQL, and here it would land in a model's context
/// window rather than a log.
/// Parses the `as_of` tool argument. A model passes it as an RFC 3339
/// string; a malformed one is the caller's mistake, so it surfaces as an
/// invalid-params error rather than being silently ignored.
/// A save result carrying both halves of the answer.
///
/// The prose is for the model, which is what reads it; the structured content
/// is for the client, which should not have to parse English to learn whether
/// anything was written. The spec makes `outputSchema` optional, so declaring
/// none is legal — the shape is small and stable:
/// `{"status": ..., "understanding": ..., "stored": [...]}`.
fn save_result(text: &str, outcome: &SaveOutcome) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(text.to_string())]);
    result.structured_content = Some(json!({
        "status": outcome.status,
        "understanding": outcome.understanding,
        "stored": stored_unless_known(outcome.status, &outcome.memories),
    }));
    result
}

/// What a save wrote: nothing, when the store already knew it — the memory
/// the daemon echoes then is the one already stored.
fn stored_unless_known(status: SaveStatus, memories: &[ToolMemory]) -> Vec<Value> {
    if status == SaveStatus::AlreadyKnown {
        Vec::new()
    } else {
        stored_json(memories)
    }
}

/// The batch form: one entry per submitted item, in order, so a client can
/// tell a NOOP item from a failed one without reading the summary sentence.
fn save_batch_result(text: &str, outcome: &BatchSaveOutcome) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(text.to_string())]);
    result.structured_content = Some(json!({
        "understanding": outcome.understanding,
        "items": outcome.items.iter().map(|item| json!({
            "status": item.status,
            "error": item.error,
            "stored": item
                .status
                .map(|status| stored_unless_known(status, &item.memories))
                .unwrap_or_default(),
        })).collect::<Vec<_>>(),
    }));
    result
}

fn stored_json(memories: &[super::memory_toolbox::ToolMemory]) -> Vec<Value> {
    memories
        .iter()
        .map(|memory| {
            json!({
                "id": memory.id,
                "category": memory.category,
                "content": memory.content,
            })
        })
        .collect()
}

/// The tool arguments as the toolbox carries them. Only `valid_until` is
/// parsed here; every rule is the daemon's.
fn save_request(params: SaveParams, client: &str) -> std::result::Result<SaveRequest, RaError> {
    Ok(SaveRequest {
        content: params.content,
        kind: params.kind,
        topic: params.topic,
        applies_to: params.applies_to.map(|applies_to| SaveAppliesTo {
            target: applies_to.target,
            project_name: applies_to.project_name,
            new_project: applies_to.new_project,
        }),
        origin: params.origin,
        subtopic: params.subtopic,
        tags: params.tags,
        valid_until: params
            .valid_until
            .as_deref()
            .map(parse_valid_until)
            .transpose()?,
        replaces: params.replaces,
        coexists_with: params.coexists_with,
        client: Some(client.to_string()),
    })
}

fn update_request(params: UpdateParams) -> std::result::Result<UpdateRequest, RaError> {
    // An empty string is how a tool argument says "clear it".
    let cleared = |raw: Option<String>| raw.map(|raw| Some(raw).filter(|v| !v.trim().is_empty()));
    Ok(UpdateRequest {
        id: params.id,
        content: params.content,
        kind: params.kind,
        topic: params.topic,
        applies_to: params.applies_to.map(|applies_to| SaveAppliesTo {
            target: applies_to.target,
            project_name: applies_to.project_name,
            new_project: applies_to.new_project,
        }),
        subtopic: cleared(params.subtopic),
        tags: params.tags,
        valid_until: cleared(params.valid_until)
            .map(|value| value.as_deref().map(parse_valid_until).transpose())
            .transpose()?,
        goal_status: params.goal_status,
        origin: params.origin,
    })
}

/// An RFC 3339 time, or a date meaning "through the end of that day, UTC".
fn parse_valid_until(raw: &str) -> std::result::Result<DateTime<Utc>, RaError> {
    let raw = raw.trim();
    if let Ok(at) = DateTime::parse_from_rfc3339(raw) {
        return Ok(at.with_timezone(&Utc));
    }
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.succ_opt())
        .and_then(|next| next.and_hms_opt(0, 0, 0))
        .map(|at| at.and_utc())
        .ok_or_else(|| {
            RaError::rejected(
                "invalid_value",
                format!("valid_until {raw:?} is neither an RFC 3339 time nor a YYYY-MM-DD date"),
            )
        })
}

/// A refused save as a tool result the agent reads — "not saved", the code,
/// and how to fix it — rather than a protocol error it may not show. Any
/// other failure stays an error.
fn rejected_result(error: RaError, verdict: &str) -> Result<CallToolResult, ErrorData> {
    match error {
        RaError::Rejected {
            code,
            message,
            details,
        } => {
            let mut result = CallToolResult::error(vec![ContentBlock::text(format!(
                "{verdict} ({code}): {message}"
            ))]);
            let mut structured = json!({ "rejected": code, "message": message });
            if let Some(details) = details {
                structured["details"] = details;
            }
            result.structured_content = Some(structured);
            Ok(result)
        }
        other => Err(to_mcp_error(other)),
    }
}

/// A refused project change: a rejection, or a name another project has.
fn refused_result(error: RaError, verdict: &str) -> Result<CallToolResult, ErrorData> {
    match error {
        RaError::Conflict(message) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
            "{verdict}: {message}"
        ))])),
        other => rejected_result(other, verdict),
    }
}

fn parse_as_of(raw: &str) -> std::result::Result<DateTime<Utc>, RaError> {
    DateTime::parse_from_rfc3339(raw.trim())
        .map(|at| at.with_timezone(&Utc))
        .map_err(|e| RaError::Validation(format!("as_of is not an RFC 3339 timestamp: {e}")))
}

fn to_mcp_error(error: RaError) -> ErrorData {
    match error {
        RaError::Validation(message) => ErrorData::invalid_params(message, None),
        RaError::NotFound(message) => ErrorData::invalid_params(message, None),
        RaError::Unauthorized(_) | RaError::Forbidden(_) => ErrorData::invalid_request(
            "this Recuerdos AI API key is not permitted to do that",
            None,
        ),
        RaError::Conflict(message) | RaError::PreconditionFailed(message) => {
            ErrorData::invalid_request(message, None)
        }
        // The save tools turn a rejection into a tool result the agent reads
        // and acts on (`rejected_result`); any other tool reaching one reports
        // it the same way an invalid argument would be.
        RaError::Rejected { code, message, .. } => {
            ErrorData::invalid_params(format!("{message} ({code})"), None)
        }
        // A busy second stage is not a client's mistake and never reaches
        // here anyway — the recaller serves the pre-rerank order (invariant
        // I-2) — so it answers with the same opaque internal error as any
        // other server-side fault. Nothing about an existing variant's
        // mapping moved to make room for it.
        RaError::RerankBusy(_) => {
            tracing::warn!("mcp tool reached a busy rerank session");
            ErrorData::internal_error("internal error", None)
        }
        RaError::Internal(detail) => {
            tracing::error!(error = %detail, "mcp tool failed");
            ErrorData::internal_error("internal error", None)
        }
    }
}
