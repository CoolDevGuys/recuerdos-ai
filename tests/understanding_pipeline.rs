//! End-to-end tests for the understanding pipeline.
//!
//! These run the real `recordagent serve` binary against a mock provider
//! served over real HTTP: the daemon's own OpenAI-compatible client, its
//! real job queue, its real workers, its real SQLite. Nothing is stubbed
//! except the model's answers.
//!
//! That matters because the unit tests all substitute a `ChatModel` trait
//! object, which means none of them exercise the wiring — whether
//! `[understanding]` actually reaches the client, whether the worker pool
//! is running, whether `wait: true` takes a different path. Every one of
//! those has failed at some point in a way no unit test could catch.

mod common;

use common::TestApp;
use serde_json::{Value, json};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A daemon wired to a mock model, plus a key.
struct Pipeline {
    app: TestApp,
    key: String,
    #[allow(dead_code)]
    model: MockServer,
    http: reqwest::Client,
}

impl Pipeline {
    /// Starts a daemon whose `[understanding]` points at a mock server
    /// that answers every completion with `replies` in order.
    async fn with_model(replies: Vec<Value>) -> Self {
        let model = MockServer::start().await;

        for (index, reply) in replies.into_iter().enumerate() {
            // `up_to_n_times(1)` on each, mounted in order, is how
            // wiremock expresses a script: each mock answers once, and
            // the next call falls through to the one after it.
            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{
                        "finish_reason": "stop",
                        "message": {"role": "assistant", "content": reply.to_string()},
                    }]
                })))
                .up_to_n_times(1)
                // Higher priority for earlier replies, so they are
                // consumed in the order given.
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

    /// Starts a daemon with no provider at all — the default.
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

    /// Polls a job until it reaches a terminal state.
    async fn await_job(&self, job_id: &str) -> Value {
        for _ in 0..300 {
            let (_, job) = self.get(&format!("/v1/jobs/{job_id}")).await;
            match job["status"].as_str() {
                Some("succeeded") | Some("failed") => return job,
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        panic!("job {job_id} never finished");
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
}

#[tokio::test]
async fn submitting_raw_content_returns_a_job_that_becomes_memories() {
    // The headline flow: one sentence in, two separately-recallable
    // memories out, without the caller waiting on a model.
    let pipeline = Pipeline::with_model(vec![
        json!({"candidates": [
            {"content": "The backend runs on Hetzner", "category": "fact.project"},
            {"content": "User requires table-driven tests in Go", "category": "preference.coding"}
        ]}),
        json!({"decisions": [{"action": "ADD", "reason": "unrelated"}]}),
    ])
    .await;

    let (status, accepted) = pipeline
        .post(
            "/v1/memories",
            json!({"content": "we moved to Hetzner; also always table-driven tests in Go"}),
        )
        .await;

    assert_eq!(status, 202, "{accepted}");
    assert_eq!(accepted["status"], "pending");
    let job_id = accepted["job_id"].as_str().expect("a job id").to_string();
    assert_eq!(accepted["poll"], format!("/v1/jobs/{job_id}"));

    let job = pipeline.await_job(&job_id).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["memory_ids"].as_array().unwrap().len(), 2, "{job}");

    let recalled = pipeline.search("where does the backend run").await;
    assert!(
        recalled.contains(&"The backend runs on Hetzner".to_string()),
        "{recalled:?}"
    );
}

#[tokio::test]
async fn a_contradiction_supersedes_the_memory_it_replaces() {
    // project-plan §12.3, through the whole stack. Afterwards, asking
    // where the backend deploys must not mention Fly.io.
    let pipeline = Pipeline::with_model(vec![
        json!({"candidates": [
            {"content": "Backend deploys on Fly.io", "category": "fact.project"}
        ]}),
        json!({"candidates": [
            {"content": "Backend deploys on Hetzner", "category": "fact.project"}
        ]}),
        // Only two scripted replies: the reconciliation reply cannot be
        // written yet, because it has to name a memory id that does not
        // exist until the first ingestion has run. It is mounted below,
        // once there is an id to name.
    ])
    .await;

    let (_, first) = pipeline
        .post(
            "/v1/memories",
            json!({"content": "we deploy on fly.io", "wait": true}),
        )
        .await;
    let old_id = first["memory_ids"][0].as_str().expect("an id").to_string();

    // Now that the id exists, script the UPDATE that names it.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": json!({"decisions": [{
                    "action": "UPDATE",
                    "memory_id": old_id,
                    "reason": "the deployment target changed to Hetzner",
                }]}).to_string()},
            }]
        })))
        // Lowest priority, so it only answers once the two scripted
        // extraction replies above have been consumed.
        .with_priority(200)
        .mount(&pipeline.model)
        .await;

    let (_, second) = pipeline
        .post(
            "/v1/memories",
            json!({"content": "we're switching to Hetzner, fly.io got too expensive", "wait": true}),
        )
        .await;
    assert_eq!(second["status"], "succeeded", "{second}");

    let recalled = pipeline.search("where does the backend deploy").await;
    assert!(
        !recalled.iter().any(|content| content.contains("Fly.io")),
        "the superseded memory is still being recalled: {recalled:?}"
    );
    assert!(
        recalled.iter().any(|content| content.contains("Hetzner")),
        "{recalled:?}"
    );

    // Supersede is not delete: the old memory is still there deliberately.
    let (_, audit) = pipeline.get("/v1/audit?limit=50").await;
    let entries = audit["entries"].as_array().expect("audit entries");
    let supersede = entries
        .iter()
        .find(|entry| entry["operation"] == "supersede")
        .unwrap_or_else(|| panic!("no supersede entry in {audit}"));

    assert!(
        supersede["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("Hetzner"),
        "the rationale should reach the audit trail: {supersede}"
    );
}

#[tokio::test]
async fn small_talk_produces_a_job_that_succeeds_with_nothing() {
    // Must not dead-letter. "Nothing here is worth remembering" is the
    // most common outcome there is.
    let pipeline = Pipeline::with_model(vec![json!({"candidates": []})]).await;

    let (status, accepted) = pipeline
        .post("/v1/memories", json!({"content": "thanks, that worked!"}))
        .await;
    assert_eq!(status, 202);

    let job = pipeline
        .await_job(accepted["job_id"].as_str().unwrap())
        .await;

    assert_eq!(job["status"], "succeeded", "{job}");
    assert!(job["memory_ids"].as_array().unwrap().is_empty(), "{job}");
}

#[tokio::test]
async fn a_provider_outage_retries_rather_than_losing_the_memory() {
    let model = MockServer::start().await;
    // Every call fails with a retryable status.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .mount(&model)
        .await;

    let app = TestApp::spawn_with(&[
        ("RECORDAGENT_UNDERSTANDING__PROVIDER", "openai-compat"),
        ("RECORDAGENT_UNDERSTANDING__MODEL", "mock"),
        ("RECORDAGENT_UNDERSTANDING__API_KEY_ENV", "MOCK_MODEL_KEY"),
        ("RECORDAGENT_UNDERSTANDING__BASE_URL", &model.uri()),
        ("MOCK_MODEL_KEY", "not-a-real-key"),
        // One attempt, so the test does not wait out a backoff.
        ("RECORDAGENT_UNDERSTANDING__MAX_ATTEMPTS", "1"),
    ])
    .await;
    let key = app.create_user_with_key("alex", "read,write");
    let http = reqwest::Client::new();

    let accepted: Value = http
        .post(format!("{}/v1/memories", app.base_url))
        .bearer_auth(&key)
        .json(&json!({"content": "I prefer pnpm"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let job_id = accepted["job_id"].as_str().unwrap();
    let mut job = Value::Null;
    for _ in 0..300 {
        job = http
            .get(format!("{}/v1/jobs/{job_id}", app.base_url))
            .bearer_auth(&key)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if job["status"] == "failed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert_eq!(job["status"], "failed", "{job}");
    assert!(
        job["error"].as_str().unwrap_or_default().contains("503"),
        "a dead-lettered job must say why: {job}"
    );
}

#[tokio::test]
async fn without_a_provider_content_is_stored_verbatim_through_the_same_endpoint() {
    // The degraded half of the mode matrix. Same endpoint, same job
    // record — so turning understanding on later changes nothing for a
    // client.
    let pipeline = Pipeline::degraded().await;

    let (status, accepted) = pipeline
        .post(
            "/v1/memories",
            json!({"content": "I prefer pnpm over npm for this repo"}),
        )
        .await;
    assert_eq!(status, 202, "{accepted}");

    let job = pipeline
        .await_job(accepted["job_id"].as_str().unwrap())
        .await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["memory_ids"].as_array().unwrap().len(), 1);

    let recalled = pipeline.search("package manager").await;
    assert_eq!(
        recalled,
        ["I prefer pnpm over npm for this repo"],
        "verbatim mode should store exactly what was sent"
    );
}

#[tokio::test]
async fn waiting_returns_the_result_instead_of_a_job_to_poll() {
    let pipeline = Pipeline::degraded().await;

    let (status, result) = pipeline
        .post(
            "/v1/memories",
            json!({"content": "The API is written in Rust", "wait": true}),
        )
        .await;

    assert_eq!(status, 201, "{result}");
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["memory_ids"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["understanding"], false,
        "a caller must be able to tell verbatim storage from extraction"
    );
}

#[tokio::test]
async fn a_synchronous_ingestion_still_leaves_a_job_record() {
    // So the audit story does not depend on which flag the caller used.
    let pipeline = Pipeline::degraded().await;

    let (_, result) = pipeline
        .post(
            "/v1/memories",
            json!({"content": "The API is written in Rust", "wait": true}),
        )
        .await;

    let (status, job) = pipeline
        .get(&format!("/v1/jobs/{}", result["job_id"].as_str().unwrap()))
        .await;

    assert_eq!(status, 200);
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["memory_ids"], result["memory_ids"]);
}

#[tokio::test]
async fn empty_content_is_rejected_before_a_job_is_created() {
    // Paying for a model call that can only answer "nothing" is waste.
    let pipeline = Pipeline::degraded().await;

    let (status, body) = pipeline
        .post("/v1/memories", json!({"content": "   "}))
        .await;

    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "validation_failed", "{body}");
}

#[tokio::test]
async fn one_users_job_is_invisible_to_another() {
    let pipeline = Pipeline::degraded().await;
    let (_, accepted) = pipeline
        .post("/v1/memories", json!({"content": "alex's secret"}))
        .await;

    let sam = pipeline.app.create_user_with_key("sam", "read,write");
    let response = pipeline
        .http
        .get(format!(
            "{}/v1/jobs/{}",
            pipeline.app.base_url,
            accepted["job_id"].as_str().unwrap()
        ))
        .bearer_auth(sam)
        .send()
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        404,
        "another user could see the job's status"
    );
}

#[tokio::test]
async fn ingesting_requires_the_write_scope() {
    let pipeline = Pipeline::degraded().await;
    let read_only = pipeline.app.create_user_with_key("reader", "read");

    let response = pipeline
        .http
        .post(format!("{}/v1/memories", pipeline.app.base_url))
        .bearer_auth(read_only)
        .json(&json!({"content": "should not be stored"}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn the_direct_endpoint_still_bypasses_the_pipeline_entirely() {
    // The escape hatch has to keep working: a caller that has already
    // decided what to remember should not pay for a model call.
    let pipeline = Pipeline::with_model(vec![]).await;

    let (status, memory) = pipeline
        .post(
            "/v1/memories:direct",
            json!({"content": "Stored exactly as written", "category": "fact.project"}),
        )
        .await;

    assert_eq!(status, 201, "{memory}");
    assert_eq!(memory["content"], "Stored exactly as written");
    // The mock model was given no scripted replies at all; reaching it
    // would have produced an error rather than this 201.
}
