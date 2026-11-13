//! Contradictions, amending and replacing (ai-specs/memory-structure-plan.md
//! §5, examples E6, E7, E20 and E22), end to end: the real daemon, its real
//! store and embedder, and — where a judgement is needed — a mock model
//! served over HTTP, answering through the daemon's own provider client.

mod common;

use common::TestApp;
use reqwest::StatusCode;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Store {
    app: TestApp,
    key: String,
    http: reqwest::Client,
    /// Kept alive for the daemon's lifetime.
    _model: Option<MockServer>,
}

impl Store {
    /// A daemon with no model: the default.
    async fn without_a_model() -> Self {
        let app = TestApp::spawn().await;
        let key = app.create_user_with_key("alex", "read,write");
        Self {
            app,
            key,
            http: reqwest::Client::new(),
            _model: None,
        }
    }

    /// A daemon whose model answers each call with the next of `replies`.
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
            ("RECUERDOS_AI_UNDERSTANDING__PROVIDER", "openai-compat"),
            ("RECUERDOS_AI_UNDERSTANDING__MODEL", "mock"),
            ("RECUERDOS_AI_UNDERSTANDING__API_KEY_ENV", "MOCK_MODEL_KEY"),
            ("RECUERDOS_AI_UNDERSTANDING__BASE_URL", &model.uri()),
            ("MOCK_MODEL_KEY", "not-a-real-key"),
        ])
        .await;
        let key = app.create_user_with_key("alex", "read,write");
        Self {
            app,
            key,
            http: reqwest::Client::new(),
            _model: Some(model),
        }
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Value) -> (StatusCode, Value) {
        let response = self
            .http
            .request(method, format!("{}{path}", self.app.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn save(&self, body: Value) -> (StatusCode, Value) {
        self.send(reqwest::Method::POST, "/v1/memories:direct", body)
            .await
    }

    async fn update(&self, id: &str, body: Value) -> (StatusCode, Value) {
        self.send(reqwest::Method::PATCH, &format!("/v1/memories/{id}"), body)
            .await
    }

    async fn get(&self, id: &str) -> Value {
        let response = self
            .http
            .get(format!("{}/v1/memories/{id}", self.app.base_url))
            .bearer_auth(&self.key)
            .send()
            .await
            .unwrap();
        response.json().await.unwrap()
    }

    async fn recall(&self, query: &str) -> Vec<String> {
        let (_, body) = self
            .send(
                reqwest::Method::POST,
                "/v1/memories/search",
                json!({"query": query, "min_relevance": 0.0}),
            )
            .await;
        body["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| hit["content"].as_str().unwrap().to_string())
            .collect()
    }
}

fn mine(content: &str) -> Value {
    json!({
        "content": content,
        "kind": "preference",
        "topic": "coding",
        "applies_to": {"target": "me"},
        "origin": "user_said",
    })
}

/// The judge's answer: nothing contradicts, and the text fits its scope.
fn fits() -> Value {
    json!({"neighbours": [], "scope": {"fits": true}})
}

#[tokio::test]
async fn e6_a_change_of_mind_is_refused_as_a_conflict_then_replaces_the_old_memory() {
    let store = Store::with_model(vec![
        fits(),
        json!({
            "neighbours": [{"number": 1, "verdict": "contradicts",
                            "reason": "mocks are preferred in one and avoided in the other"}],
            "scope": {"fits": true},
        }),
        fits(),
    ])
    .await;

    let (status, old) = store
        .save(mine(
            "The user prefers mocks over real dependencies in unit tests",
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{old}");
    let old_id = old["id"].as_str().unwrap();

    let (status, refused) = store
        .save(mine("The user now avoids mocks in unit tests"))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "conflict");
    let message = refused["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("prefers mocks over real dependencies"),
        "{message}"
    );
    assert!(
        message.contains("mocks are preferred"),
        "the model's reason: {message}"
    );
    assert_eq!(refused["error"]["details"]["judged"], true);
    assert_eq!(refused["error"]["details"]["conflicts"][0]["id"], old_id);

    // The user changed their mind.
    let mut replacing = mine("The user now avoids mocks in unit tests");
    replacing["replaces"] = json!(old_id);
    let (status, saved) = store.save(replacing).await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["replaced"], json!([old_id]));
    assert!(
        saved["message"]
            .as_str()
            .unwrap()
            .contains("which is kept in history"),
        "{saved}"
    );

    // Superseded, so out of recall, and kept.
    assert_eq!(store.get(old_id).await["superseded_by"], saved["id"]);
    assert_eq!(
        store.recall("does the user use mocks in unit tests").await,
        ["The user now avoids mocks in unit tests"]
    );
}

#[tokio::test]
async fn without_a_model_a_near_repeat_is_stopped_and_can_be_kept_on_purpose() {
    let store = Store::without_a_model().await;
    let (_, old) = store
        .save(mine("The user prefers tabs for indentation"))
        .await;
    let old_id = old["id"].as_str().unwrap();

    let (status, refused) = store
        .save(mine("The user prefers spaces for indentation"))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "conflict");
    assert_eq!(refused["error"]["details"]["judged"], false);
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Without a language model"),
        "{refused}"
    );

    // Two things that can both hold are not stopped.
    let (status, body) = store.save(mine("The user prefers Vitest")).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // The user says both hold.
    let mut both = mine("The user prefers spaces for indentation");
    both["coexists_with"] = json!([old_id]);
    let (status, body) = store.save(both).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn saying_the_same_thing_twice_stores_it_once() {
    let store = Store::without_a_model().await;
    let (_, first) = store.save(mine("The user prefers pnpm")).await;

    let (status, again) = store.save(mine("The user prefers pnpm.")).await;

    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["status"], "already_known");
    assert_eq!(again["id"], first["id"]);
    assert!(
        again["message"]
            .as_str()
            .unwrap()
            .starts_with("Already known"),
        "{again}"
    );
}

#[tokio::test]
async fn e7_a_typo_is_amended_in_place() {
    let store = Store::without_a_model().await;
    let (_, saved) = store
        .save(mine("The user prefers mokcs in unit tests"))
        .await;
    let id = saved["id"].as_str().unwrap();

    let (status, amended) = store
        .update(
            id,
            json!({"content": "The user prefers mocks in unit tests"}),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{amended}");
    assert_eq!(amended["id"], id);
    assert_eq!(amended["content"], "The user prefers mocks in unit tests");
    assert_eq!(amended["kind"], "preference");
    assert!(amended["message"].as_str().unwrap().starts_with("Updated"));

    // The trail keeps the change.
    let (_, audit) = store
        .send(reqwest::Method::GET, "/v1/audit", Value::Null)
        .await;
    assert!(
        audit["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["memory_id"] == id && entry["operation"] == "update"),
        "{audit}"
    );
}

#[tokio::test]
async fn e20_only_the_user_closes_a_goal_and_a_closed_goal_leaves_recall() {
    let store = Store::without_a_model().await;
    let (status, goal) = store
        .save(json!({
            "content": "The user's goal is to run a half marathon in April",
            "kind": "goal",
            "topic": "health",
            "applies_to": {"target": "me"},
            "origin": "user_said",
        }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    assert_eq!(goal["goal_status"], "active");
    let id = goal["id"].as_str().unwrap();

    let (status, refused) = store
        .update(id, json!({"goal_status": "achieved", "origin": "inferred"}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "needs_confirmation");

    let (status, closed) = store
        .update(
            id,
            json!({"goal_status": "achieved", "origin": "user_confirmed"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{closed}");
    assert_eq!(closed["goal_status"], "achieved");

    assert!(
        !store
            .recall("half marathon")
            .await
            .iter()
            .any(|content| content.contains("half marathon")),
        "a closed goal is still recalled"
    );
    assert_eq!(
        store.get(id).await["goal_status"],
        "achieved",
        "kept in history"
    );
}

#[tokio::test]
async fn e22_the_model_catches_a_paraphrase_the_phrase_list_misses() {
    let store = Store::with_model(vec![json!({
        "neighbours": [],
        "scope": {"fits": false, "reason": "\"here the convention is\" describes one codebase"},
    })])
    .await;

    let (status, refused) = store
        .save(json!({
            "content": "Here the convention is to name handlers after the command they run",
            "kind": "rule",
            "topic": "coding",
            "applies_to": {"target": "me"},
            "origin": "user_said",
        }))
        .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "scope_contradiction");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("describes one codebase"),
        "{refused}"
    );
}
