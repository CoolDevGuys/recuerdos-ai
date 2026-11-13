//! Projects over REST (ai-specs/memory-structure-plan.md §6, example E15):
//! listing, renaming and merging them against the real daemon and store,
//! with the local MCP server's project headers set by hand where a request
//! comes from inside a repository.

mod common;

use common::TestApp;
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Store {
    app: TestApp,
    key: String,
    http: reqwest::Client,
}

/// Where a request comes from: a repository's name and its remote, as the
/// stdio shim sends them.
type Folder<'a> = Option<(&'a str, &'a str)>;

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

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        folder: Folder<'_>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.app.base_url))
            .bearer_auth(&self.key);
        if let Some((name, remote)) = folder {
            request = request
                .header("x-recuerdos-project", name)
                .header("x-recuerdos-project-remote", remote);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// A rule of the folder's project; `new_project` confirms a new one.
    async fn save_rule(
        &self,
        folder: Folder<'_>,
        content: &str,
        new_project: bool,
    ) -> (StatusCode, Value) {
        self.send(
            reqwest::Method::POST,
            "/v1/memories:direct",
            folder,
            Some(json!({
                "content": content,
                "kind": "rule",
                "topic": "coding",
                "applies_to": {"target": "project", "new_project": new_project},
                "origin": "user_said",
            })),
        )
        .await
    }

    async fn projects(&self) -> Vec<Value> {
        let (status, body) = self
            .send(reqwest::Method::GET, "/v1/projects", None, None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["projects"].as_array().unwrap().clone()
    }

    async fn memory(&self, id: &str) -> Value {
        self.send(
            reqwest::Method::GET,
            &format!("/v1/memories/{id}"),
            None,
            None,
        )
        .await
        .1
    }

    async fn rename(&self, project: &str, name: &str, folder: Folder<'_>) -> (StatusCode, Value) {
        self.send(
            reqwest::Method::PATCH,
            &format!("/v1/projects/{project}"),
            folder,
            Some(json!({"name": name})),
        )
        .await
    }

    async fn merge(&self, project: &str, into: &str) -> (StatusCode, Value) {
        self.send(
            reqwest::Method::POST,
            &format!("/v1/projects/{project}/merge"),
            None,
            Some(json!({"into": into})),
        )
        .await
    }
}

const QUILL: Folder = Some(("quill", "github.com/acme/quill"));
const QUILL_APP: Folder = Some(("quill-app", "github.com/acme/quill-app"));
const FORK: Folder = Some(("quill", "github.com/me/quill"));

#[tokio::test]
async fn e15_a_renamed_repository_is_offered_as_a_rename_and_the_rename_fixes_it() {
    let store = Store::new().await;
    let (status, saved) = store
        .save_rule(QUILL, "Migrations are forward-only", true)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let old_memory = saved["id"].as_str().unwrap().to_string();

    // The repository was renamed on its host: its remote is one no project
    // knows.
    let (status, refused) = store
        .save_rule(QUILL_APP, "Handlers are named after their command", false)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "unknown_project");
    let message = refused["error"]["message"].as_str().unwrap();
    assert!(message.contains("Is it a rename of quill"), "{message}");
    assert_eq!(refused["error"]["details"]["rename_of"], json!(["quill"]));

    // The user says yes; the agent renames from inside the folder.
    let (status, renamed) = store.rename("quill", "quill-app", QUILL_APP).await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["project"]["name"], "quill-app");
    assert_eq!(renamed["previous"], "quill");
    assert_eq!(renamed["learned"], json!(["github.com/acme/quill-app"]));
    assert!(
        renamed["message"]
            .as_str()
            .unwrap()
            .starts_with("Renamed quill to quill-app"),
        "{renamed}"
    );

    // Its memories are found under the new name…
    assert_eq!(
        store.memory(&old_memory).await["applies_to"]["project_name"],
        "quill-app"
    );
    // …the new remote resolves…
    let (status, saved) = store
        .save_rule(QUILL_APP, "Handlers are named after their command", false)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["applies_to"]["project_name"], "quill-app");
    // …and so does the old one, from a checkout that was never updated.
    let (status, saved) = store
        .save_rule(QUILL, "Every endpoint has a contract test", false)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["applies_to"]["project_name"], "quill-app");

    let projects = store.projects().await;
    assert_eq!(projects.len(), 1, "{projects:?}");
    assert_eq!(projects[0]["memories"], 3);
    let aliases = projects[0]["aliases"].as_array().unwrap();
    for alias in [
        "quill",
        "github.com/acme/quill",
        "github.com/acme/quill-app",
    ] {
        assert!(aliases.contains(&json!(alias)), "{alias} in {aliases:?}");
    }
}

#[tokio::test]
async fn a_fork_with_the_same_name_becomes_its_own_project_and_can_be_merged_back() {
    let store = Store::new().await;
    store
        .save_rule(QUILL, "Migrations are forward-only", true)
        .await;

    let (status, refused) = store
        .save_rule(FORK, "The fork releases from its own main", false)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["details"]["collides_with"], "quill");

    let (status, saved) = store
        .save_rule(FORK, "The fork releases from its own main", true)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["applies_to"]["project_name"], "me-quill");
    assert!(
        saved["message"]
            .as_str()
            .unwrap()
            .contains("Created the project me-quill"),
        "{saved}"
    );
    let fork_memory = saved["id"].as_str().unwrap().to_string();
    assert_eq!(store.projects().await.len(), 2);

    // The user says the fork is the same project after all.
    let (status, merged) = store.merge("me-quill", "quill").await;
    assert_eq!(status, StatusCode::OK, "{merged}");
    assert_eq!(merged["moved"], 1);
    assert_eq!(merged["project"]["memories"], 2);
    assert!(
        merged["message"]
            .as_str()
            .unwrap()
            .starts_with("Merged me-quill into quill"),
        "{merged}"
    );

    let projects = store.projects().await;
    assert_eq!(projects.len(), 1);
    assert_eq!(
        store.memory(&fork_memory).await["applies_to"]["project_name"],
        "quill"
    );
    // The fork's remote now finds quill, with nothing to confirm.
    let (status, saved) = store
        .save_rule(FORK, "The fork tracks upstream weekly", false)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["applies_to"]["project_name"], "quill");

    // The move is in the audit trail.
    let (_, audit) = store
        .send(reqwest::Method::GET, "/v1/audit", None, None)
        .await;
    assert!(
        audit["entries"].as_array().unwrap().iter().any(|entry| {
            entry["memory_id"] == fork_memory.as_str()
                && entry["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("merged into quill"))
        }),
        "{audit}"
    );
}

#[tokio::test]
async fn a_project_is_found_by_id_name_or_remote() {
    let store = Store::new().await;
    store
        .save_rule(QUILL, "Migrations are forward-only", true)
        .await;
    let id = store.projects().await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    for identifier in [id.as_str(), "quill", "github.com%2Facme%2Fquill"] {
        let (status, project) = store
            .send(
                reqwest::Method::GET,
                &format!("/v1/projects/{identifier}"),
                None,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{identifier}: {project}");
        assert_eq!(project["name"], "quill", "{identifier}");
    }

    let (status, missing) = store
        .send(reqwest::Method::GET, "/v1/projects/quil", None, None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{missing}");
    assert_eq!(missing["error"]["code"], "unknown_project");
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap()
            .contains("known: quill"),
        "{missing}"
    );
}

#[tokio::test]
async fn a_rename_onto_another_projects_name_or_an_invalid_one_is_refused() {
    let store = Store::new().await;
    store
        .save_rule(QUILL, "Migrations are forward-only", true)
        .await;
    store
        .save_rule(Some(("fern", "github.com/acme/fern")), "Fern uses Go", true)
        .await;

    let (status, taken) = store.rename("quill", "fern", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{taken}");
    assert!(
        taken["error"]["message"]
            .as_str()
            .unwrap()
            .contains("merge"),
        "{taken}"
    );

    let (status, invalid) = store.rename("quill", "Quill App", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{invalid}");
    assert_eq!(invalid["error"]["code"], "invalid_value");

    let (status, itself) = store.merge("quill", "quill").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{itself}");
}

#[tokio::test]
async fn a_read_only_key_can_list_projects_but_not_rename_them() {
    let store = Store::new().await;
    store
        .save_rule(QUILL, "Migrations are forward-only", true)
        .await;
    let reader = store.app.create_user_with_key("sam", "read");

    let response = store
        .http
        .patch(format!("{}/v1/projects/quill", store.app.base_url))
        .bearer_auth(&reader)
        .json(&json!({"name": "quill-app"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Another user's projects are not theirs to see.
    let listed: Value = store
        .http
        .get(format!("{}/v1/projects", store.app.base_url))
        .bearer_auth(&reader)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["projects"], json!([]));
}
