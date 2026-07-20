//! End-to-end tests for the consolidation context.
//!
//! Same approach as `understanding_pipeline.rs`, and for the same reason:
//! the unit tests substitute a `ChatModel` trait object and so prove
//! nothing about the wiring. Distillation in particular is wired to a
//! *second* pipeline built with a different prompt, and "both pipelines
//! were built from the same extractor" is exactly the kind of mistake
//! that compiles, passes every unit test, and silently files a
//! transcript's task chatter as durable memories.

mod common;

use common::TestApp;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Instance {
    app: TestApp,
    key: String,
    model: MockServer,
    http: reqwest::Client,
}

impl Instance {
    async fn with_model(replies: Vec<Value>) -> Self {
        let model = MockServer::start().await;

        for (index, reply) in replies.into_iter().enumerate() {
            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{
                        "finish_reason": "stop",
                        "message": {"role": "assistant", "content": reply.to_string()},
                    }]
                })))
                .up_to_n_times(1)
                .with_priority(index as u8 + 1)
                .mount(&model)
                .await;
        }

        let app = TestApp::spawn_with(&[
            ("RECORDAGENT_UNDERSTANDING__PROVIDER", "openai-compat"),
            ("RECORDAGENT_UNDERSTANDING__MODEL", "mock"),
            ("RECORDAGENT_UNDERSTANDING__API_KEY_ENV", "MOCK_MODEL_KEY"),
            ("RECORDAGENT_UNDERSTANDING__BASE_URL", &model.uri()),
            ("MOCK_MODEL_KEY", "not-a-real-key"),
        ])
        .await;

        let key = app.create_user_with_key("alex", "read,write");
        Self {
            app,
            key,
            model,
            http: reqwest::Client::new(),
        }
    }

    /// A daemon with no provider — the default configuration.
    async fn degraded() -> Self {
        let model = MockServer::start().await;
        let app = TestApp::spawn().await;
        let key = app.create_user_with_key("alex", "read,write");
        Self {
            app,
            key,
            model,
            http: reqwest::Client::new(),
        }
    }

    async fn post(&self, path: &str, body: Value) -> (reqwest::StatusCode, Value) {
        let response = self
            .http
            .post(format!("{}{path}", self.app.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .expect("request failed");

        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> (reqwest::StatusCode, Value) {
        let response = self
            .http
            .get(format!("{}{path}", self.app.base_url))
            .bearer_auth(&self.key)
            .send()
            .await
            .expect("request failed");

        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn distill(&self, content: &str) -> (reqwest::StatusCode, Value) {
        self.post("/v1/sessions/distill", json!({"content": content}))
            .await
    }

    async fn search(&self, query: &str) -> Vec<String> {
        let (_, results) = self
            .post("/v1/memories/search", json!({"query": query, "limit": 20}))
            .await;

        results["results"]
            .as_array()
            .expect("results array")
            .iter()
            .map(|hit| hit["content"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The system prompt of the nth call the daemon made to the model.
    async fn system_prompt(&self, index: usize) -> String {
        let requests = self
            .model
            .received_requests()
            .await
            .expect("the mock recorded requests");
        let body: Value =
            serde_json::from_slice(&requests[index].body).expect("a JSON request body");

        body["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .filter(|message| message["role"] == "system")
            .map(|message| message["content"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Runs `recordagent consolidate` against a spawned app's data dir.
///
/// The nightly job has no HTTP surface — it is a timer and a CLI command
/// — so this is the only way to drive it end to end. It runs as a
/// separate process against the same SQLite file, which is exactly how an
/// operator would run it.
fn consolidate(instance: &Instance, model_url: &str, args: &[&str]) -> std::process::Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_recordagent"));
    command
        .arg("consolidate")
        .args(args)
        .env("RECORDAGENT_STORAGE__PATH", instance.app.data_dir())
        .env("RECORDAGENT_UNDERSTANDING__PROVIDER", "openai-compat")
        .env("RECORDAGENT_UNDERSTANDING__MODEL", "mock")
        .env("RECORDAGENT_UNDERSTANDING__API_KEY_ENV", "MOCK_MODEL_KEY")
        .env("RECORDAGENT_UNDERSTANDING__BASE_URL", model_url)
        .env("MOCK_MODEL_KEY", "not-a-real-key")
        .env_remove("RECORDAGENT_LOG");

    command
        .output()
        .expect("failed to run recordagent consolidate")
}

#[tokio::test]
async fn five_phrasings_of_one_preference_become_one_memory() {
    // The DoD scenario, end to end and out of process: seed duplicates
    // over REST, run the CLI job, then ask REST what recall returns.
    let instance = Instance::with_model(vec![json!({
        "merge": true,
        "content": "User uses pnpm as the package manager; never npm or yarn",
        "category": "preference.coding",
        "tags": ["tooling"],
        "reason": "five phrasings of one package-manager preference",
    })])
    .await;

    // Identical content, so the deterministic local embedder puts them in
    // one cluster regardless of the configured threshold.
    for _ in 0..5 {
        instance
            .post(
                "/v1/memories:direct",
                json!({"content": "User prefers pnpm", "category": "preference.coding"}),
            )
            .await;
    }
    assert_eq!(instance.search("pnpm").await.len(), 5);

    let output = consolidate(&instance, &instance.model.uri(), &[]);
    assert!(
        output.status.success(),
        "consolidate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("5 memories retired"), "{stdout}");

    assert_eq!(
        instance.search("package manager").await,
        ["User uses pnpm as the package manager; never npm or yarn"],
        "recall should return exactly one memory after consolidation"
    );

    // Superseded, not deleted — the originals are still in the trail.
    let (status, audit) = instance.get("/v1/audit").await;
    assert_eq!(status, 200, "{audit}");
    let merges = audit["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("no audit entries in {audit}"))
        .iter()
        .filter(|entry| entry["operation"] == "merge")
        .count();
    assert_eq!(merges, 5, "{audit}");
}

#[tokio::test]
async fn a_dry_run_reports_what_it_would_merge_and_changes_nothing() {
    let instance = Instance::with_model(vec![]).await;

    for _ in 0..3 {
        instance
            .post(
                "/v1/memories:direct",
                json!({"content": "User prefers pnpm", "category": "preference.coding"}),
            )
            .await;
    }

    let output = consolidate(&instance, &instance.model.uri(), &["--dry-run"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("nothing was changed"), "{stdout}");
    assert!(stdout.contains("would merge"), "{stdout}");
    assert!(stdout.contains("User prefers pnpm"), "{stdout}");

    assert_eq!(
        instance.search("pnpm").await.len(),
        3,
        "a dry run modified the store"
    );
    assert!(
        instance
            .model
            .received_requests()
            .await
            .expect("recorded requests")
            .is_empty(),
        "a dry run called the model"
    );
}

#[tokio::test]
async fn consolidation_without_a_provider_refuses_rather_than_guessing() {
    let instance = Instance::degraded().await;

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_recordagent"))
        .arg("consolidate")
        .env("RECORDAGENT_STORAGE__PATH", instance.app.data_dir())
        .env_remove("RECORDAGENT_LOG")
        .output()
        .expect("failed to run recordagent consolidate");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("[understanding].provider"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn a_session_is_distilled_into_the_memories_that_outlive_it() {
    let instance = Instance::with_model(vec![
        json!({"candidates": [
            {"content": "Session tokens expired early because the refresh timer used \
                         local time instead of UTC",
             "category": "experience", "tags": ["auth"]},
            {"content": "The team writes table-driven tests for new Go packages",
             "category": "preference.coding", "tags": ["go"]}
        ]}),
        json!({"decisions": [{"action": "ADD", "reason": "a convention, not the bug"}]}),
    ])
    .await;

    let (status, body) = instance
        .distill(
            "long session: chased a token expiry bug for two hours, found the timer \
             bug, fixed it, then agreed to use table-driven tests going forward. \
             Tests are green now and the branch is pushed.",
        )
        .await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["distilled"], 2, "{body}");

    // The point of the whole feature: what survived is recallable in a
    // later session, and the session's own status ("tests are green")
    // is not among it.
    let recalled = instance.search("why did tokens expire").await;
    assert!(
        recalled
            .iter()
            .any(|content| content.contains("refresh timer")),
        "{recalled:?}"
    );
    assert!(
        !recalled.iter().any(|content| content.contains("green")),
        "session status was stored as a memory: {recalled:?}"
    );
}

#[tokio::test]
async fn distillation_asks_the_session_question_not_the_submission_one() {
    // The wiring test. Both pipelines are structurally identical and
    // differ only in the prompt the extractor was built with, so nothing
    // but this would notice them being wired to the same one.
    let instance = Instance::with_model(vec![json!({"candidates": []})]).await;

    instance.distill("a session that yielded nothing").await;

    let prompt = instance.system_prompt(0).await;
    assert!(
        prompt.contains("still true after this session ends"),
        "the distillation endpoint used the submission prompt: {prompt}"
    );
}

#[tokio::test]
async fn ordinary_ingestion_still_uses_the_submission_prompt() {
    // The other half of the same wiring risk, in the other direction.
    let instance = Instance::with_model(vec![json!({"candidates": []})]).await;

    instance
        .post(
            "/v1/memories",
            json!({"content": "I prefer pnpm", "wait": true}),
        )
        .await;

    let prompt = instance.system_prompt(0).await;
    assert!(
        prompt.contains("You extract durable memories from raw text"),
        "ordinary ingestion picked up the distillation prompt: {prompt}"
    );
}

#[tokio::test]
async fn a_session_with_nothing_durable_in_it_succeeds_with_nothing() {
    // Runs at the end of every session a user has. If "nothing worth
    // keeping" read as a failure, a hook would report a broken memory
    // service daily.
    let instance = Instance::with_model(vec![json!({"candidates": []})]).await;

    let (status, body) = instance
        .distill("thanks, that worked. ok running the tests. all green, pushing now")
        .await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["distilled"], 0, "{body}");
    assert_eq!(body["memory_ids"], json!([]));
    assert!(instance.search("tests").await.is_empty());
}

#[tokio::test]
async fn without_a_provider_distillation_refuses_rather_than_storing_the_transcript() {
    // The one surface that does not degrade to verbatim. Storing a whole
    // transcript as a single memory would be worse than storing nothing:
    // unrecallable, and spent from a context window on every match.
    let instance = Instance::degraded().await;

    let (status, body) = instance.distill("a long transcript of a session").await;

    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("[understanding].provider"),
        "the error must name the setting to change: {body}"
    );
    assert!(
        instance.search("transcript").await.is_empty(),
        "the transcript was stored anyway"
    );
}

#[tokio::test]
async fn an_empty_session_is_rejected_before_any_model_call() {
    let instance = Instance::with_model(vec![]).await;

    let (status, body) = instance.distill("   ").await;

    assert_eq!(status, 400, "{body}");
    assert!(
        instance
            .model
            .received_requests()
            .await
            .expect("recorded requests")
            .is_empty(),
        "an empty session reached the provider"
    );
}

#[tokio::test]
async fn distilling_requires_the_write_scope() {
    let instance = Instance::with_model(vec![]).await;
    let read_only = instance.app.create_user_with_key("sam", "read");

    let response = instance
        .http
        .post(format!("{}/v1/sessions/distill", instance.app.base_url))
        .bearer_auth(&read_only)
        .json(&json!({"content": "a session"}))
        .send()
        .await
        .expect("request failed");

    assert_eq!(response.status(), 403);
}
