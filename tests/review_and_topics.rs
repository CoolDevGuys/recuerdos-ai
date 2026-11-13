//! The web UI's S5 surfaces over REST (ai-specs/memory-structure-plan.md §8,
//! §3.3): topics, the review queue's structure bulk edit and its Suggest,
//! against the real daemon — with a mock model where Suggest needs one.

mod common;

use common::TestApp;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Store {
    app: TestApp,
    key: String,
    http: reqwest::Client,
    _model: Option<MockServer>,
}

impl Store {
    async fn new() -> Self {
        let app = TestApp::spawn().await;
        let key = app.create_user_with_key("alex", "read,write");
        Self {
            app,
            key,
            http: reqwest::Client::new(),
            _model: None,
        }
    }

    async fn with_model(reply: Value) -> Self {
        let model = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"finish_reason": "stop",
                             "message": {"role": "assistant", "content": reply.to_string()}}]
            })))
            .mount(&model)
            .await;
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

    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.app.base_url))
            .bearer_auth(&self.key);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// An old-style memory, as the migration left it: filed by a guess.
    async fn legacy(&self, content: &str, category: &str) -> String {
        let (status, body) = self
            .send(
                Method::POST,
                "/v1/memories",
                Some(json!({"content": content, "category": category, "wait": true})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["memory_ids"][0].as_str().unwrap().to_string()
    }

    async fn memory(&self, id: &str) -> Value {
        self.send(Method::GET, &format!("/v1/memories/{id}"), None)
            .await
            .1
    }
}

#[tokio::test]
async fn topics_are_listed_with_counts_added_and_relabelled() {
    let store = Store::new().await;
    store
        .legacy("Prefers pnpm over npm", "preference.coding")
        .await;

    let (status, listed) = store.send(Method::GET, "/v1/topics", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let coding = &listed["topics"][0];
    assert_eq!(coding["id"], "coding");
    assert_eq!(coding["count"], 1);
    assert_eq!(coding["built_in"], true);

    let (status, created) = store
        .send(
            Method::POST,
            "/v1/topics",
            Some(json!({"id": "homelab", "label": "Home lab"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["label"], "Home lab");

    let (status, again) = store
        .send(Method::POST, "/v1/topics", Some(json!({"id": "homelab"})))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");

    let (status, relabelled) = store
        .send(
            Method::PATCH,
            "/v1/topics/coding",
            Some(json!({"label": "Software"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{relabelled}");
    assert_eq!(relabelled["label"], "Software");
    assert_eq!(relabelled["default_label"], "Coding");

    // The map shows the user's words for its topics.
    let (_, map) = store.send(Method::GET, "/v1/map", None).await;
    assert_eq!(map["topics"][0]["label"], "Software", "{map}");

    // A topic just added can be filed under.
    let (status, saved) = store
        .send(
            Method::POST,
            "/v1/memories:direct",
            Some(
                json!({"content": "The NAS has 40 TB", "kind": "fact", "topic": "homelab",
                        "applies_to": {"target": "general"}, "origin": "user_said"}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
}

#[tokio::test]
async fn the_review_queue_files_many_memories_at_once_under_the_save_rules() {
    let store = Store::new().await;
    let nas = store
        .legacy("The NAS backups are in /mnt/tank", "fact.project")
        .await;
    let router = store
        .legacy("The router is at 10.0.0.1", "fact.project")
        .await;
    assert_eq!(store.memory(&nas).await["needs_review"], true);

    let (status, edited) = store
        .send(
            Method::POST,
            "/v1/memories:bulk",
            Some(json!({"ids": [nas, router],
                        "set": {"kind": "reference", "topic": "tech",
                                "applies_to": {"target": "general"}}})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["changed"], 2);
    for memory in edited["memories"].as_array().unwrap() {
        assert_eq!(memory["kind"], "reference");
        assert_eq!(memory["topic"], "tech");
        assert_eq!(memory["needs_review"], false);
        assert_eq!(memory["origin"], "user_confirmed");
    }

    // A structure one memory cannot take changes none of them.
    let tied = store
        .legacy("In this repo we never mock the database", "fact.project")
        .await;
    let (status, refused) = store
        .send(
            Method::POST,
            "/v1/memories:bulk",
            Some(json!({"ids": [tied], "set": {"kind": "rule", "applies_to": {"target": "me"}}})),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "invalid_value");

    // Accepting a guess as it is clears the flag, but not for a guess the
    // rules refuse: a "this repo" memory cannot be confirmed as general.
    let (status, refused) = store
        .send(
            Method::POST,
            "/v1/memories:bulk",
            Some(json!({"ids": [tied], "set": {"reviewed": true}})),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let fine = store
        .legacy("The printer is a Brother HL-L2350", "fact.project")
        .await;
    let (status, reviewed) = store
        .send(
            Method::POST,
            "/v1/memories:bulk",
            Some(json!({"ids": [fine], "set": {"reviewed": true}})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reviewed}");
    assert_eq!(store.memory(&fine).await["needs_review"], false);

    // A project must already exist to file under it in bulk.
    let (status, unknown) = store
        .send(
            Method::POST,
            "/v1/memories:bulk",
            Some(json!({"ids": [nas],
                        "set": {"applies_to": {"target": "project", "project_name": "quill"}}})),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unknown}");
    assert_eq!(unknown["error"]["code"], "unknown_project");
}

#[tokio::test]
async fn suggest_needs_a_model_and_ping_says_whether_there_is_one() {
    let store = Store::new().await;
    let id = store.legacy("The NAS has 40 TB", "fact.project").await;

    let (_, ping) = store.send(Method::GET, "/v1/ping", None).await;
    assert_eq!(ping["understanding"], false);

    let (status, refused) = store
        .send(
            Method::POST,
            "/v1/memories:suggest",
            Some(json!({"ids": [id]})),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "no_model");
}

#[tokio::test]
async fn suggest_proposes_a_structure_and_writes_nothing() {
    let store = Store::with_model(json!({"suggestions": [
        {"number": 1, "kind": "reference", "topic": "tech", "target": "general",
         "reason": "a path on the home NAS"}
    ]}))
    .await;
    let (status, saved) = store
        .send(
            Method::POST,
            "/v1/memories:direct",
            Some(
                json!({"content": "The NAS backups are in /mnt/tank", "category": "fact.project",
                        "kind": "fact", "topic": "coding",
                        "applies_to": {"target": "general"}, "origin": "user_said"}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let id = saved["id"].as_str().unwrap();

    let (status, body) = store
        .send(
            Method::POST,
            "/v1/memories:suggest",
            Some(json!({"ids": [id]})),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let suggestion = &body["suggestions"][0];
    assert_eq!(suggestion["id"], id);
    assert_eq!(suggestion["kind"], "reference");
    assert_eq!(suggestion["applies_to"]["target"], "general");
    assert_eq!(suggestion["reason"], "a path on the home NAS");
    assert_eq!(
        store.memory(id).await["kind"],
        "fact",
        "nothing was written"
    );
}
