//! Who "the user" is, end to end: the names a store's owner goes by
//! (`/v1/me`), the save rule that files a memory about them as theirs (R13),
//! the review queue for what was filed as someone else's, and the profile
//! line that tells an agent "the user" and the name are one person.

mod common;

use common::TestApp;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

struct Store {
    app: TestApp,
    key: String,
    http: reqwest::Client,
}

impl Store {
    async fn new() -> Self {
        let app = TestApp::spawn().await;
        let key = app.create_user_with_key("alex", "read,write");
        Self {
            app,
            key,
            http: reqwest::Client::new(),
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

    async fn save(&self, content: &str, target: &str) -> (StatusCode, Value) {
        self.send(
            Method::POST,
            "/v1/memories:direct",
            Some(json!({"content": content, "kind": "fact", "topic": "work",
                        "applies_to": {"target": target}, "origin": "user_said"})),
        )
        .await
    }

    /// A memory as v0.6.0's upgrade left one: a fact about "a person",
    /// general, not flagged.
    async fn legacy_person(&self, content: &str) -> String {
        let (status, body) = self
            .send(
                Method::POST,
                "/v1/memories",
                Some(json!({"content": content, "category": "fact.person", "wait": true})),
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

    async fn profile(&self) -> String {
        self.http
            .get(format!("{}/v1/profile", self.app.base_url))
            .bearer_auth(&self.key)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn names_start_empty_and_are_saved_trimmed() {
    let store = Store::new().await;

    let (status, empty) = store.send(Method::GET, "/v1/me", None).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["names"], json!([]));
    assert!(empty["sentence"].is_null());

    let (status, saved) = store
        .send(
            Method::PUT,
            "/v1/me",
            Some(json!({"names": ["  Alejandro Romero ", "Alex", "alex"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["names"], json!(["Alejandro Romero", "Alex"]));
    assert_eq!(
        saved["sentence"],
        "The user is Alejandro Romero (also called Alex)."
    );

    let (status, refused) = store
        .send(Method::PUT, "/v1/me", Some(json!({"names": ["a\u{7}"]})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
}

#[tokio::test]
async fn a_fact_about_the_user_filed_as_general_is_refused_with_how_to_file_it() {
    let store = Store::new().await;

    let (status, refused) = store
        .save(
            "The user is an expert in PHP and also proficient in Python",
            "general",
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "about_the_user");

    let (status, saved) = store
        .save(
            "The user is an expert in PHP and also proficient in Python",
            "me",
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");

    // Someone in the user's world is still general.
    let (status, sister) = store
        .save("The user's sister Ana is a nurse in Madrid", "general")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sister}");
}

#[tokio::test]
async fn saving_the_names_sends_what_they_reveal_as_the_users_to_review() {
    let store = Store::new().await;
    let career = store
        .legacy_person("Alejandro Romero is a Senior Backend Software Engineer in Berlin")
        .await;
    let sister = store
        .legacy_person("The user's sister Ana is a nurse in Madrid")
        .await;
    assert_eq!(store.memory(&career).await["needs_review"], false);

    let (status, saved) = store
        .send(
            Method::PUT,
            "/v1/me",
            Some(json!({"names": ["Alejandro Romero"]})),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["flagged"], 1, "{saved}");
    assert_eq!(store.memory(&career).await["needs_review"], true);
    assert_eq!(store.memory(&sister).await["needs_review"], false);

    // Now a save naming them is refused as general too.
    let (status, refused) = store
        .save("Alejandro Romero speaks Spanish and English", "general")
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "about_the_user");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Alejandro Romero, is the user"),
        "{refused}"
    );
}

#[tokio::test]
async fn the_profile_says_who_the_user_is_and_lists_their_personal_information() {
    let store = Store::new().await;
    store
        .send(
            Method::PUT,
            "/v1/me",
            Some(json!({"names": ["Alejandro Romero", "Alex"]})),
        )
        .await;
    store.save("The user is an expert in PHP", "me").await;

    let profile = store.profile().await;

    let title_end = profile.find('\n').unwrap();
    let sentence = profile
        .find("The user is Alejandro Romero (also called Alex).")
        .unwrap_or_else(|| panic!("{profile}"));
    assert!(sentence > title_end, "{profile}");
    assert!(profile.contains("### Personal information"), "{profile}");
}
