//! Black-box tests for the MCP surface.
//!
//! These spawn the real `recordagent mcp` process against a real daemon
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
    app: TestApp,
    service: RunningService<RoleClient, ()>,
}

impl McpClient {
    async fn connect() -> Self {
        let app = TestApp::spawn().await;
        let key = app.create_user_with_key("alex", "read,write");

        let mut command = Command::new(env!("CARGO_BIN_EXE_recordagent"));
        command
            .arg("mcp")
            .env("RECORDAGENT_API_KEY", &key)
            .env("RECORDAGENT_URL", &app.base_url)
            .env_remove("RECORDAGENT_LOG");

        let service =
            ().serve(TokioChildProcess::new(command).expect("spawn the mcp shim"))
                .await
                .expect("mcp handshake");

        Self { app, service }
    }

    async fn call(&self, tool: &str, arguments: Value) -> String {
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

        result
            .content
            .iter()
            .filter_map(|block| block.as_text().map(|text| text.text.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }
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
                "category": "preference.coding",
                "tags": ["typescript"]
            }),
        )
        .await;
    assert!(saved.contains("Saved as [preference.coding]"), "{saved}");

    let recalled = client
        .call(
            "memory_recall",
            json!({"query": "how should I structure my typescript imports?"}),
        )
        .await;

    assert!(recalled.contains("barrel files"), "{recalled}");
    assert!(
        recalled.starts_with("1. [preference.coding]"),
        "results should be numbered and category-first: {recalled}"
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
        .call("memory_save", json!({"content": "a memory about pnpm"}))
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
            json!({"content": "User prefers pnpm", "category": "preference.coding"}),
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
    assert_eq!(info.server_info.name, "recordagent");
}

#[tokio::test]
async fn memories_saved_over_mcp_are_visible_over_rest() {
    // One store, two front doors. If these diverged, an agent and a
    // script would disagree about what the user believes.
    let client = McpClient::connect().await;
    client
        .call("memory_save", json!({"content": "saved through mcp"}))
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

    let mut command = Command::new(env!("CARGO_BIN_EXE_recordagent"));
    command
        .arg("mcp")
        .env(
            "RECORDAGENT_API_KEY",
            "ra_live_deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        )
        .env("RECORDAGENT_URL", &app.base_url)
        .env_remove("RECORDAGENT_LOG");

    let result = ().serve(TokioChildProcess::new(command).expect("spawn")).await;

    assert!(result.is_err(), "a bad key should not complete a handshake");
}
