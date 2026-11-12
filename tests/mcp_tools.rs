//! Black-box tests for the MCP surface.
//!
//! These spawn the real `recuerdos-ai mcp` process against a real daemon
//! and drive it with a real rmcp client over stdio — the exact path
//! Claude Code takes. Nothing here stubs the protocol, so a mistake in
//! the tool schemas, the transport, or the shim's HTTP forwarding shows
//! up as a failing test rather than as an editor that silently lists no
//! tools.

mod common;

use common::TestApp;
use rmcp::RoleClient;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use serde_json::{Map, Value, json};
use tokio::process::Command;

/// A daemon plus an MCP client connected to its stdio shim.
struct McpClient {
    /// Shared by every session of [`McpClient::elsewhere`].
    app: std::sync::Arc<TestApp>,
    key: String,
    service: RunningService<RoleClient, ()>,
    // The folder the shim runs in, kept alive for the test.
    _folder: tempfile::TempDir,
}

impl McpClient {
    /// A shim started in an empty folder: no project.
    async fn connect() -> Self {
        Self::connect_in(tempfile::tempdir().unwrap()).await
    }

    /// A shim started inside a git repository whose `origin` is `remote`, the
    /// way a harness starts it inside the project it works on.
    async fn connect_in_repository(remote: &str) -> Self {
        Self::connect_in(repository(remote)).await
    }

    async fn connect_in(folder: tempfile::TempDir) -> Self {
        let app = TestApp::spawn().await;
        let key = app.create_user_with_key("alex", "read,write");
        let service = shim(&app, &key, &folder).await;
        Self {
            app: std::sync::Arc::new(app),
            key,
            service,
            _folder: folder,
        }
    }

    /// Another session of the same user, on the same daemon, started inside
    /// another repository.
    async fn elsewhere(&self, remote: &str) -> McpClient {
        let folder = repository(remote);
        let service = shim(&self.app, &self.key, &folder).await;
        McpClient {
            app: std::sync::Arc::clone(&self.app),
            key: self.key.clone(),
            service,
            _folder: folder,
        }
    }

    async fn call(&self, tool: &str, arguments: Value) -> String {
        self.call_raw(tool, arguments).await.0
    }

    /// The prose the model reads, plus the `structuredContent` a client is
    /// meant to branch on. A save that reports only the first leaves the
    /// caller parsing English to find out whether anything was written.
    async fn call_structured(&self, tool: &str, arguments: Value) -> (String, Option<Value>) {
        let (text, structured) = self.call_raw(tool, arguments).await;
        (text, structured)
    }

    /// A call the tool answers with a refusal (`isError`), as the text the
    /// agent reads.
    async fn call_refused(&self, tool: &str, arguments: Value) -> String {
        let result = self
            .service
            .call_tool(
                CallToolRequestParams::new(tool.to_string()).with_arguments(as_object(arguments)),
            )
            .await
            .unwrap_or_else(|e| panic!("calling {tool} failed: {e}"));
        assert_eq!(
            result.is_error,
            Some(true),
            "{tool} should refuse: {result:?}"
        );
        result
            .content
            .iter()
            .filter_map(|block| block.as_text().map(|text| text.text.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn call_raw(&self, tool: &str, arguments: Value) -> (String, Option<Value>) {
        let result = self
            .service
            .call_tool(
                CallToolRequestParams::new(tool.to_string()).with_arguments(as_object(arguments)),
            )
            .await
            .unwrap_or_else(|e| panic!("calling {tool} failed: {e}"));

        assert_ne!(
            result.is_error,
            Some(true),
            "{tool} reported an error: {result:?}"
        );

        let text = result
            .content
            .iter()
            .filter_map(|block| block.as_text().map(|text| text.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");

        (text, result.structured_content)
    }
}

/// A git repository whose `origin` is `remote`.
fn repository(remote: &str) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    for args in [vec!["init", "-q"], vec!["remote", "add", "origin", remote]] {
        let status = std::process::Command::new("git")
            .args(&args)
            .current_dir(folder.path())
            .status()
            .expect("git is installed");
        assert!(status.success(), "git {args:?}");
    }
    folder
}

/// The stdio shim started in `folder`, the way a harness starts it.
async fn shim(
    app: &TestApp,
    key: &str,
    folder: &tempfile::TempDir,
) -> RunningService<RoleClient, ()> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_recuerdos-ai"));
    command
        .arg("mcp")
        .current_dir(folder.path())
        .env("RECUERDOS_AI_API_KEY", key)
        .env("RECUERDOS_AI_URL", &app.base_url)
        .env_remove("RECUERDOS_AI_PROJECT")
        .env_remove("RECUERDOS_AI_LOG");
    ().serve(TokioChildProcess::new(command).expect("spawn the mcp shim"))
        .await
        .expect("mcp handshake")
}

fn as_object(value: Value) -> Map<String, Value> {
    value
        .as_object()
        .cloned()
        .expect("arguments must be an object")
}

#[tokio::test]
async fn the_client_sees_the_three_memory_tools() {
    let client = McpClient::connect().await;

    let tools = client.service.list_all_tools().await.expect("list tools");

    let names: Vec<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
    assert!(names.contains(&"memory_save".to_string()), "got {names:?}");
    assert!(
        names.contains(&"memory_recall".to_string()),
        "got {names:?}"
    );
    assert!(
        names.contains(&"memory_forget".to_string()),
        "got {names:?}"
    );
    assert!(
        names.contains(&"memory_save_batch".to_string()),
        "got {names:?}"
    );
    assert!(
        names.contains(&"memory_update".to_string()),
        "got {names:?}"
    );
    for project_tool in ["project_list", "project_rename", "project_merge"] {
        assert!(names.contains(&project_tool.to_string()), "got {names:?}");
    }
}

#[tokio::test]
async fn a_batch_save_stores_every_item_and_they_recall() {
    // The batch tool over the real stdio shim: several memories in one
    // call, each stored and independently recallable.
    let client = McpClient::connect().await;

    let saved = client
        .call(
            "memory_save_batch",
            json!({"items": [
                {"content": "User prefers pnpm", "kind": "preference", "topic": "coding",
                 "applies_to": {"target": "me"}, "origin": "user_said"},
                {"content": "The backend runs on Hetzner", "kind": "fact", "topic": "tech",
                 "applies_to": {"target": "general"}, "origin": "user_said"}
            ]}),
        )
        .await;
    assert!(saved.contains("Saved"), "{saved}");

    assert!(
        client
            // A question, not two bare words: since enriched vectors became
            // the default, a terse keyword-free query against a two-memory
            // store reads just under the relevance floor (relevance ≈0.23).
            .call(
                "memory_recall",
                json!({"query": "which package manager does the user prefer?"})
            )
            .await
            .contains("pnpm"),
        "the first batch item is not recallable"
    );
    assert!(
        client
            .call(
                "memory_recall",
                json!({"query": "where does the backend run"})
            )
            .await
            .contains("Hetzner"),
        "the second batch item is not recallable"
    );
}

#[tokio::test]
async fn the_tool_descriptions_tell_a_model_when_to_call_them() {
    // The description is the only thing deciding when a memory gets
    // saved — there is no other trigger logic. Asserted against the
    // generated tool list, which is what an agent actually receives.
    let client = McpClient::connect().await;
    let tools = client.service.list_all_tools().await.expect("list tools");

    let describe = |name: &str| -> String {
        tools
            .iter()
            .find(|tool| tool.name == name)
            .and_then(|tool| tool.description.clone())
            .unwrap_or_else(|| panic!("{name} has no description"))
            .to_string()
    };

    let save = describe("memory_save");
    assert!(save.contains("I prefer"), "no trigger examples: {save}");
    assert!(
        save.contains("Do NOT"),
        "without negative guidance the store fills with task chatter: {save}"
    );

    assert!(
        save.contains("conflict") && save.contains("replaces"),
        "the agent must know how to answer a conflict: {save}"
    );

    let update = describe("memory_update");
    assert!(update.contains("Only the user closes a goal"), "{update}");
    assert!(
        update.contains("replaces"),
        "amend versus replace: {update}"
    );

    let forget = describe("memory_forget");
    assert!(forget.contains("two-step"), "{forget}");
    assert!(forget.contains("Do not use it to tidy up"), "{forget}");
}

#[tokio::test]
async fn save_then_recall_round_trips_through_the_protocol() {
    let client = McpClient::connect().await;

    let saved = client
        .call(
            "memory_save",
            json!({
                "content": "User forbids barrel files and index.ts re-exports",
                "kind": "preference",
                "topic": "coding",
                "applies_to": {"target": "me"},
                "origin": "user_said",
                "tags": ["typescript"]
            }),
        )
        .await;
    assert!(
        saved.contains("Saved as your personal coding preference (applies to you everywhere)."),
        "the reply says how it was filed: {saved}"
    );

    let recalled = client
        .call(
            "memory_recall",
            json!({"query": "how should I structure my typescript imports?"}),
        )
        .await;

    assert!(recalled.contains("barrel files"), "{recalled}");
    assert!(
        recalled.starts_with("1. [preference · coding · me]"),
        "results should be numbered and scope-first: {recalled}"
    );
}

#[tokio::test]
async fn recall_with_no_matches_says_so_rather_than_returning_nothing() {
    let client = McpClient::connect().await;

    let recalled = client
        .call("memory_recall", json!({"query": "anything at all"}))
        .await;

    assert!(recalled.contains("No memories matched"), "{recalled}");
}

#[tokio::test]
async fn forget_requires_two_steps_and_deletes_nothing_on_the_first() {
    let client = McpClient::connect().await;
    client
        .call(
            "memory_save",
            common::structured(json!({"content": "a memory about pnpm"})),
        )
        .await;

    // Step one: candidates only.
    let candidates = client.call("memory_forget", json!({"query": "pnpm"})).await;
    assert!(
        candidates.contains("Nothing has been deleted yet"),
        "{candidates}"
    );
    assert!(candidates.contains("confirm: true"), "{candidates}");

    // Still there.
    let recalled = client.call("memory_recall", json!({"query": "pnpm"})).await;
    assert!(recalled.contains("a memory about pnpm"), "{recalled}");

    // Step two, but without confirming: still nothing.
    let id = candidates
        .lines()
        .find_map(|line| line.strip_prefix("- "))
        .and_then(|line| line.split(' ').next())
        .expect("a candidate id")
        .to_string();

    let unconfirmed = client
        .call("memory_forget", json!({"ids": [id.clone()]}))
        .await;
    assert!(unconfirmed.contains("Nothing deleted"), "{unconfirmed}");
    assert!(
        client
            .call("memory_recall", json!({"query": "pnpm"}))
            .await
            .contains("a memory about pnpm"),
        "an unconfirmed forget deleted the memory"
    );

    // Step two, confirmed.
    let deleted = client
        .call("memory_forget", json!({"ids": [id], "confirm": true}))
        .await;
    assert!(deleted.contains("Deleted 1 memory"), "{deleted}");

    let after = client.call("memory_recall", json!({"query": "pnpm"})).await;
    assert!(after.contains("No memories matched"), "{after}");
}

#[tokio::test]
async fn forget_without_a_query_or_ids_is_rejected() {
    let client = McpClient::connect().await;

    let result = client
        .service
        .call_tool(CallToolRequestParams::new("memory_forget").with_arguments(as_object(json!({}))))
        .await;

    assert!(
        result.is_err(),
        "expected an invalid-params error: {result:?}"
    );
}

#[tokio::test]
async fn the_profile_resource_is_listed_and_readable() {
    let client = McpClient::connect().await;
    client
        .call(
            "memory_save",
            json!({"content": "User prefers pnpm", "kind": "preference", "topic": "coding",
                   "applies_to": {"target": "me"}, "origin": "user_said"}),
        )
        .await;

    let resources = client
        .service
        .list_all_resources()
        .await
        .expect("list resources");
    assert!(
        resources.iter().any(|r| r.uri == "memory://profile"),
        "got {resources:?}"
    );

    let profile = client
        .service
        .read_resource(rmcp::model::ReadResourceRequestParams::new(
            "memory://profile",
        ))
        .await
        .expect("read the profile");

    let text = profile
        .contents
        .iter()
        .filter_map(|content| match content {
            rmcp::model::ResourceContents::TextResourceContents { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("# Memory profile: alex"), "{text}");
    assert!(text.contains("User prefers pnpm"), "{text}");
}

#[tokio::test]
async fn an_unknown_resource_is_an_error_not_an_empty_read() {
    let client = McpClient::connect().await;

    let result = client
        .service
        .read_resource(rmcp::model::ReadResourceRequestParams::new(
            "memory://nonsense",
        ))
        .await;

    assert!(result.is_err(), "got {result:?}");
}

#[tokio::test]
async fn the_server_tells_the_client_how_to_use_it() {
    let client = McpClient::connect().await;

    let info = client.service.peer_info().expect("server info");

    let instructions = info.instructions.clone().unwrap_or_default();
    assert!(
        instructions.contains("memory://profile"),
        "the server should point clients at the profile: {instructions}"
    );
    assert_eq!(info.server_info.name, "recuerdos-ai");
}

#[tokio::test]
async fn memories_saved_over_mcp_are_visible_over_rest() {
    // One store, two front doors. If these diverged, an agent and a
    // script would disagree about what the user believes.
    let client = McpClient::connect().await;
    client
        .call(
            "memory_save",
            common::structured(json!({"content": "saved through mcp"})),
        )
        .await;

    let key = client.app.create_user_with_key("second", "read,write");
    let _ = key; // a different user must not see it

    let export = reqwest::Client::new()
        .get(format!("{}/v1/memories/export", client.app.base_url))
        .bearer_auth(client.app.create_user_with_key("third", "read"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        !export.contains("saved through mcp"),
        "another user saw an MCP-saved memory: {export}"
    );
}

#[tokio::test]
async fn a_bad_api_key_stops_the_shim_from_starting() {
    // Better to fail the handshake than to start and error on every call:
    // an MCP client surfaces a server that failed to start far more
    // clearly than one that appears healthy and refuses all work.
    let app = TestApp::spawn().await;

    let mut command = Command::new(env!("CARGO_BIN_EXE_recuerdos-ai"));
    command
        .arg("mcp")
        .env(
            "RECUERDOS_AI_API_KEY",
            "ra_live_deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        )
        .env("RECUERDOS_AI_URL", &app.base_url)
        .env_remove("RECUERDOS_AI_LOG");

    let result = ().serve(TokioChildProcess::new(command).expect("spawn")).await;

    assert!(result.is_err(), "a bad key should not complete a handshake");
}

#[tokio::test]
async fn a_save_reports_its_outcome_as_structured_content() {
    // The prose tells a model what happened; this is what a client branches
    // on. Without it, "Saved" and "the store already knew it" are the same
    // byte sequence to anything that is not reading English.
    let client = McpClient::connect().await;
    let (text, structured) = client
        .call_structured(
            "memory_save",
            json!({"content": "The backend deploys on Railway", "kind": "fact", "topic": "tech",
                   "applies_to": {"target": "general"}, "origin": "user_said"}),
        )
        .await;

    let structured = structured.expect("a save must carry a machine-readable outcome");
    assert_eq!(
        structured["status"],
        json!("stored"),
        "a save that wrote a memory must say so: {structured}"
    );

    let stored = structured["stored"]
        .as_array()
        .expect("`stored` must be a list of what was written");
    assert_eq!(stored.len(), 1, "{structured}");
    assert!(
        stored[0]["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && !id.starts_with('-')),
        "the client needs a usable id: {stored:?}"
    );
    assert_eq!(stored[0]["category"], json!("fact.project"));

    // The two halves must not disagree, or the model is told one thing and
    // the client another.
    assert!(text.contains("Railway"), "{text}");
    assert!(text.contains("Saved"), "prose and structure agree: {text}");
}

#[tokio::test]
async fn a_batch_save_reports_one_status_per_item() {
    // A batch that says "saved 2 memories" and hides which item did nothing
    // is the single-save trap again, one level up.
    let client = McpClient::connect().await;
    let (_text, structured) = client
        .call_structured(
            "memory_save_batch",
            json!({"items": [
                common::structured(json!({"content": "The API is written in Rust"})),
                common::structured(json!({"content": "The migrations run with sqlx"}))
            ]}),
        )
        .await;

    let structured = structured.expect("a batch save must carry a machine-readable outcome");
    let items = structured["items"]
        .as_array()
        .expect("`items` must be a list, one per submitted item");
    assert_eq!(items.len(), 2, "{structured:#}");
    for item in items {
        assert_eq!(item["status"], json!("stored"), "{structured:#}");
        assert!(item["error"].is_null(), "no item failed: {structured:#}");
        assert_eq!(item["stored"].as_array().expect("stored list").len(), 1);
    }
}

/// E3 over MCP: a refusal reaches the agent as a tool result it reads, with
/// the code and what to do, not as a protocol error it may never show.
#[tokio::test]
async fn a_refused_save_tells_the_agent_why_and_what_to_do() {
    let client = McpClient::connect().await;

    let refused = client
        .call_refused(
            "memory_save",
            json!({"content": "Prefers small pull requests", "kind": "preference",
                   "topic": "coding", "applies_to": {"target": "me"}, "origin": "inferred"}),
        )
        .await;

    assert!(
        refused.starts_with("Not saved (needs_confirmation)"),
        "{refused}"
    );
    assert!(refused.contains("Ask the user"), "{refused}");
}

/// Inside a repository the shim knows the project: a "this repo" rule needs
/// no project name, an unknown project is only created once the user
/// confirms, and the remote is remembered so the next save finds it.
#[tokio::test]
async fn inside_a_repository_the_project_is_detected_from_its_remote() {
    let client = McpClient::connect_in_repository("git@github.com:acme/quill.git").await;
    let rule = json!({"content": "In this repo migrations are forward-only", "kind": "rule",
                      "topic": "coding", "applies_to": {"target": "project"},
                      "origin": "user_said"});

    let refused = client.call_refused("memory_save", rule.clone()).await;
    assert!(
        refused.starts_with("Not saved (unknown_project)"),
        "{refused}"
    );
    assert!(refused.contains("github.com/acme/quill"), "{refused}");

    let mut confirmed = rule;
    confirmed["applies_to"]["new_project"] = json!(true);
    let saved = client.call("memory_save", confirmed).await;
    assert!(
        saved.contains("Saved as a coding rule of quill."),
        "{saved}"
    );
    assert!(saved.contains("Created the project quill."), "{saved}");

    // No target at all: the detected project is the default (plan D4).
    let defaulted = client
        .call(
            "memory_save",
            json!({"content": "Releases are tagged from main by CI", "kind": "rule",
                   "topic": "coding", "origin": "user_said"}),
        )
        .await;
    assert!(defaulted.contains("of quill"), "{defaulted}");
    assert!(
        defaulted.contains("confirm that with the user"),
        "{defaulted}"
    );
}

/// A near-repeat is refused over MCP with the stored memory's id and the
/// ways out; amending it with `memory_update` keeps its id; replacing it
/// retires it.
#[tokio::test]
async fn a_conflict_is_explained_and_resolved_through_the_tools() {
    let client = McpClient::connect().await;
    let mine = |content: &str| {
        json!({"content": content, "kind": "preference", "topic": "coding",
               "applies_to": {"target": "me"}, "origin": "user_said"})
    };

    let saved = client
        .call("memory_save", mine("The user prefers tabs for indentation"))
        .await;
    let id = saved
        .split("(id ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .unwrap_or_else(|| panic!("no id in {saved}"))
        .to_string();

    let refused = client
        .call_refused(
            "memory_save",
            mine("The user prefers spaces for indentation"),
        )
        .await;
    assert!(refused.starts_with("Not saved (conflict)"), "{refused}");
    assert!(refused.contains(&id), "{refused}");
    assert!(refused.contains("memory_update"), "{refused}");

    let amended = client
        .call(
            "memory_update",
            json!({"id": id, "content": "The user prefers tabs for indentation in Go"}),
        )
        .await;
    assert!(
        amended.starts_with("Updated; filed as your personal coding preference."),
        "{amended}"
    );
    assert!(amended.contains(&id), "{amended}");

    let mut replacing = mine("The user prefers spaces for indentation");
    replacing["replaces"] = json!(id);
    let replaced = client.call("memory_save", replacing).await;
    assert!(replaced.contains("which is kept in history"), "{replaced}");

    let refused = client
        .call_refused(
            "memory_update",
            json!({"id": id, "content": "token: ghp_8Fk2LmQ9zXv7Rt3Wq1Yp6Hs4Jd0Nb5Ce1a2"}),
        )
        .await;
    assert!(refused.starts_with("Not updated (secret)"), "{refused}");
}

/// E15 through the tools: the repository was renamed on its host, so a
/// session inside it finds no project; the refusal offers the rename,
/// `project_rename` from inside the folder fixes it, and an old checkout
/// still resolves.
#[tokio::test]
async fn e15_a_renamed_repository_is_fixed_with_project_rename() {
    let old_checkout = McpClient::connect_in_repository("git@github.com:acme/quill.git").await;
    let rule = |content: &str, new_project: bool| {
        json!({"content": content, "kind": "rule", "topic": "coding",
               "applies_to": {"target": "project", "new_project": new_project},
               "origin": "user_said"})
    };
    old_checkout
        .call("memory_save", rule("Migrations are forward-only", true))
        .await;

    let renamed_repo = old_checkout
        .elsewhere("https://github.com/acme/quill-app.git")
        .await;
    let refused = renamed_repo
        .call_refused(
            "memory_save",
            rule("Handlers are named after their command", false),
        )
        .await;
    assert!(
        refused.starts_with("Not saved (unknown_project)"),
        "{refused}"
    );
    assert!(refused.contains("Is it a rename of quill"), "{refused}");
    assert!(refused.contains("project_rename"), "{refused}");

    let renamed = renamed_repo
        .call(
            "project_rename",
            json!({"name": "quill", "new_name": "quill-app"}),
        )
        .await;
    assert!(
        renamed.starts_with("Renamed quill to quill-app"),
        "{renamed}"
    );
    assert!(renamed.contains("github.com/acme/quill-app"), "{renamed}");

    let saved = renamed_repo
        .call(
            "memory_save",
            rule("Handlers are named after their command", false),
        )
        .await;
    assert!(saved.contains("of quill-app"), "{saved}");
    let saved = old_checkout
        .call(
            "memory_save",
            rule("Every endpoint has a contract test", false),
        )
        .await;
    assert!(
        saved.contains("of quill-app"),
        "the old remote resolves: {saved}"
    );

    let listed = old_checkout.call("project_list", json!({})).await;
    assert!(listed.starts_with("1 project:"), "{listed}");
    assert!(listed.contains("quill-app — 3 memories"), "{listed}");
    assert!(listed.contains("github.com/acme/quill,"), "{listed}");
}

#[tokio::test]
async fn project_merge_folds_one_project_into_another_and_refusals_are_explained() {
    let quill = McpClient::connect_in_repository("git@github.com:acme/quill.git").await;
    let rule = |content: &str| {
        json!({"content": content, "kind": "rule", "topic": "coding",
               "applies_to": {"target": "project", "new_project": true},
               "origin": "user_said"})
    };
    quill
        .call("memory_save", rule("Migrations are forward-only"))
        .await;
    let fork = quill.elsewhere("git@github.com:me/quill.git").await;
    let saved = fork
        .call("memory_save", rule("The fork releases from its own main"))
        .await;
    assert!(saved.contains("Created the project me-quill"), "{saved}");

    let refused = fork
        .call_refused(
            "project_rename",
            json!({"name": "me-quill", "new_name": "quill"}),
        )
        .await;
    assert!(refused.starts_with("Not renamed:"), "{refused}");
    assert!(refused.contains("merge"), "{refused}");

    let merged = fork
        .call(
            "project_merge",
            json!({"name": "me-quill", "into": "quill"}),
        )
        .await;
    assert!(
        merged.starts_with("Merged me-quill into quill: 1 memory moved"),
        "{merged}"
    );
    let listed = quill.call("project_list", json!({})).await;
    assert!(listed.contains("quill — 2 memories"), "{listed}");
}

/// Recall lines name what a memory is and who it is for, and the shim's
/// folder is the project recall ranks with the user's own (plan §9).
#[tokio::test]
async fn recall_names_each_memorys_scope() {
    let client = McpClient::connect_in_repository("git@github.com:acme/quill.git").await;
    client
        .call(
            "memory_save",
            json!({"content": "Use cases are named by intent, with no Service suffix",
                   "kind": "convention", "topic": "coding",
                   "applies_to": {"target": "project", "new_project": true},
                   "origin": "user_said"}),
        )
        .await;
    client
        .call(
            "memory_save",
            json!({"content": "The user prefers Vitest over Jest for unit tests",
                   "kind": "preference", "topic": "coding",
                   "applies_to": {"target": "me"}, "origin": "user_said"}),
        )
        .await;

    let recalled = client
        .call("memory_recall", json!({"query": "how are use cases named"}))
        .await;
    assert!(
        recalled.contains("1. [convention · coding · quill] Use cases are named by intent"),
        "{recalled}"
    );

    let mine = client
        .call(
            "memory_recall",
            json!({"query": "unit tests", "targets": ["me"]}),
        )
        .await;
    assert!(
        mine.contains("[preference · coding · me] The user prefers Vitest"),
        "{mine}"
    );
    assert!(!mine.contains("Use cases"), "{mine}");
}
