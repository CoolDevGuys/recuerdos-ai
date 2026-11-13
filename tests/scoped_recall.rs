//! Recall that knows where the caller is (ai-specs/memory-structure-plan.md
//! §9, examples E16–E18), end to end: the real daemon, its real embedder and
//! store. A request from inside a project carries the local MCP server's
//! project headers, set by hand here.

mod common;

use common::TestApp;
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Store {
    app: TestApp,
    key: String,
    http: reqwest::Client,
}

type Folder<'a> = Option<(&'a str, &'a str)>;

const QUILL: Folder = Some(("quill", "github.com/acme/quill"));
const FERN: Folder = Some(("fern", "github.com/acme/fern"));
const HERMES: Folder = None;

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

    async fn post(&self, path: &str, folder: Folder<'_>, body: Value) -> (StatusCode, Value) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.app.base_url))
            .bearer_auth(&self.key)
            .json(&body);
        if let Some((name, remote)) = folder {
            request = request
                .header("x-recuerdos-project", name)
                .header("x-recuerdos-project-remote", remote);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn save(&self, folder: Folder<'_>, content: &str, kind: &str, target: &str) {
        let (status, body) = self
            .post(
                "/v1/memories:direct",
                folder,
                json!({
                    "content": content,
                    "kind": kind,
                    "topic": "coding",
                    "applies_to": {"target": target, "new_project": true},
                    "origin": "user_said",
                }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{content}: {body}");
    }

    /// A recall as an agent makes it: the server's floor, from `folder`.
    async fn recall(&self, folder: Folder<'_>, query: &str) -> Vec<Value> {
        self.search(folder, json!({"query": query})).await
    }

    async fn search(&self, folder: Folder<'_>, body: Value) -> Vec<Value> {
        let (status, body) = self.post("/v1/memories/search", folder, body).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["results"].as_array().unwrap().clone()
    }

    /// The store the examples are about: two of the user's own coding
    /// memories, two projects' conventions and one project fact.
    async fn seeded() -> Self {
        let store = Self::new().await;
        store
            .save(
                HERMES,
                "The user prefers Vitest over Jest for unit tests",
                "preference",
                "me",
            )
            .await;
        store
            .save(
                HERMES,
                "The user always writes table-driven tests",
                "rule",
                "me",
            )
            .await;
        store
            .save(
                QUILL,
                "Command handlers are named after the command they run, like CreateInvoiceHandler",
                "convention",
                "project",
            )
            .await;
        store
            .save(
                QUILL,
                "Use cases are named by intent, with no Service suffix",
                "convention",
                "project",
            )
            .await;
        store
            .save(
                QUILL,
                "The frontend is built with React and Vite",
                "fact",
                "project",
            )
            .await;
        store
            .save(
                FERN,
                "Errors are returned as Result values and never thrown",
                "convention",
                "project",
            )
            .await;
        store
    }
}

fn contents(hits: &[Value]) -> Vec<&str> {
    hits.iter()
        .map(|hit| hit["content"].as_str().unwrap())
        .collect()
}

fn project_of(hit: &Value) -> Option<&str> {
    hit["applies_to"]["project_name"].as_str()
}

#[tokio::test]
async fn e16_in_fern_my_coding_preferences_are_mine_then_ferns_and_never_quills() {
    let store = Store::seeded().await;

    let hits = store.recall(FERN, "What are my coding preferences?").await;

    assert!(
        contents(&hits).contains(&"The user prefers Vitest over Jest for unit tests"),
        "{hits:#?}"
    );
    assert!(
        contents(&hits).contains(&"The user always writes table-driven tests"),
        "{hits:#?}"
    );
    assert!(
        hits.iter().all(|hit| project_of(hit) != Some("quill")),
        "another project's conventions answered for the user: {hits:#?}"
    );
    // No order is asserted between the user's own memories and fern's: §9
    // ranks the user's, the general and the current project's alike and only
    // demotes other projects. Fern's convention and the user's rule score
    // within a hair of each other, so their order is a coin toss on recency.
}

#[tokio::test]
async fn e17_a_question_that_names_another_project_finds_it() {
    let store = Store::seeded().await;

    let hits = store
        .recall(FERN, "How did quill handle command handlers?")
        .await;

    assert_eq!(
        hits.first().map(|hit| hit["content"].as_str().unwrap()),
        Some("Command handlers are named after the command they run, like CreateInvoiceHandler"),
        "{hits:#?}"
    );
}

#[tokio::test]
async fn e18_outside_any_project_one_projects_stack_is_not_my_favourite() {
    let store = Store::seeded().await;

    let hits = store
        .recall(HERMES, "What's my favourite frontend framework?")
        .await;

    assert!(
        hits.iter().all(|hit| project_of(hit).is_none()),
        "a project's stack answered a question about the user: {hits:#?}"
    );
}

#[tokio::test]
async fn the_current_projects_memories_rank_with_the_users_own() {
    // Inside quill, quill's conventions are not demoted.
    let store = Store::seeded().await;

    let hits = store.recall(QUILL, "how are use cases named").await;

    assert_eq!(
        contents(&hits).first(),
        Some(&"Use cases are named by intent, with no Service suffix"),
        "{hits:#?}"
    );
}

#[tokio::test]
async fn structure_filters_narrow_by_kind_target_and_project() {
    let store = Store::seeded().await;

    let conventions = store
        .search(
            HERMES,
            json!({"query": "naming", "kinds": ["convention"], "projects": ["quill"]}),
        )
        .await;
    assert_eq!(conventions.len(), 2, "{conventions:#?}");
    assert!(
        conventions
            .iter()
            .all(|hit| hit["kind"] == "convention" && project_of(hit) == Some("quill"))
    );

    let mine = store
        .search(HERMES, json!({"query": "tests", "targets": ["me"]}))
        .await;
    assert_eq!(mine.len(), 2, "{mine:#?}");

    let coding = store
        .search(
            HERMES,
            json!({"query": "tests", "topics": ["coding"], "limit": 20}),
        )
        .await;
    assert_eq!(coding.len(), 6, "{coding:#?}");

    let (status, unknown) = store
        .post(
            "/v1/memories/search",
            HERMES,
            json!({"query": "x", "projects": ["quil"]}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unknown}");
    assert_eq!(unknown["error"]["code"], "unknown_project");

    let (status, bad) = store
        .post(
            "/v1/memories/search",
            HERMES,
            json!({"query": "x", "kinds": ["opinion"]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
}

#[tokio::test]
async fn scope_all_ranks_every_project_alike() {
    let store = Store::seeded().await;

    let scoped = store
        .search(
            FERN,
            json!({"query": "how are handlers named", "min_relevance": 0.0}),
        )
        .await;
    let all = store
        .search(
            FERN,
            json!({"query": "how are handlers named", "min_relevance": 0.0, "scope": "all"}),
        )
        .await;

    let relevance = |hits: &[Value]| {
        hits.iter()
            .find(|hit| {
                hit["content"]
                    .as_str()
                    .unwrap()
                    .starts_with("Command handlers")
            })
            .map(|hit| hit["relevance"].as_f64().unwrap())
            .unwrap()
    };
    assert!(
        relevance(&scoped) < relevance(&all),
        "scoped {} vs all {}",
        relevance(&scoped),
        relevance(&all)
    );
}

#[tokio::test]
async fn the_profile_inside_a_project_shows_it_and_leaves_the_others_out() {
    let store = Store::seeded().await;
    let profile = |folder: Folder<'static>| {
        let mut request = store
            .http
            .get(format!("{}/v1/profile", store.app.base_url))
            .bearer_auth(&store.key);
        if let Some((name, remote)) = folder {
            request = request
                .header("x-recuerdos-project", name)
                .header("x-recuerdos-project-remote", remote);
        }
        async move { request.send().await.unwrap().text().await.unwrap() }
    };

    let in_fern = profile(FERN).await;
    assert!(in_fern.contains("## This project: fern"), "{in_fern}");
    assert!(in_fern.contains("never thrown"), "{in_fern}");
    assert!(in_fern.contains("prefers Vitest"), "{in_fern}");
    assert!(!in_fern.contains("CreateInvoiceHandler"), "{in_fern}");

    let nowhere = profile(HERMES).await;
    assert!(!nowhere.contains("This project"), "{nowhere}");
    assert!(!nowhere.contains("never thrown"), "{nowhere}");
}

/// The control for E16 and E18: ranked without scope, the same store answers
/// both questions with quill's memories above the agent-facing floor
/// (measured 2026-11-12: 0.39 and 0.35 for "my coding preferences", 0.36 for
/// "my favourite frontend framework"). The demotion is what keeps them out,
/// not the wording of the examples.
#[tokio::test]
async fn without_scope_the_same_questions_would_be_answered_by_quill() {
    let store = Store::seeded().await;
    let floor = 0.25;
    for (folder, query) in [
        (FERN, "What are my coding preferences?"),
        (HERMES, "What's my favourite frontend framework?"),
    ] {
        let hits = store
            .search(
                folder,
                json!({"query": query, "min_relevance": floor, "scope": "all"}),
            )
            .await;
        assert!(
            hits.iter().any(|hit| project_of(hit) == Some("quill")),
            "{query}: {hits:#?}"
        );
    }
}
