//! Native Google Gemini embeddings, using the provider's own API rather
//! than its OpenAI-compatibility shim.
//!
//! # Why native, and not just openai-compat pointed at Gemini
//!
//! Gemini's embedding models are **asymmetric**: they are trained to
//! embed a document and the query that should retrieve it into related
//! but distinct spaces, selected by a `taskType`. Told
//! `RETRIEVAL_DOCUMENT` when storing and `RETRIEVAL_QUERY` when searching,
//! recall is measurably better than treating both the same. The OpenAI
//! compatibility endpoint does not expose `taskType`, so it leaves that
//! quality on the table. This client exists to spend it.
//!
//! # The API
//!
//! - Batch: `POST {base}/models/{model}:batchEmbedContents`
//! - Body: `{"requests": [{"model": "models/…", "content": {"parts":
//!   [{"text": …}]}, "taskType": "RETRIEVAL_DOCUMENT"}, …]}`
//! - Auth: `x-goog-api-key: <key>` — a header, never a `?key=` query
//!   parameter, so the secret stays out of URLs and logs.
//! - Response: `{"embeddings": [{"values": [f32, …]}, …]}`
//!
//! Blocking HTTP and startup dimension-probing, for the same reasons as
//! `remote_embedder` — see that module. The blocking client is confined to
//! its own thread by [`BlockingHttpWorker`], so the synchronous `embed`
//! (including the startup probe) is safe to call from the async runtime.

use super::blocking_http::BlockingHttpWorker;
use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::shared::error::{RaError, Result};
use serde_json::{Value, json};
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

const TIMEOUT: Duration = Duration::from_secs(60);

pub struct GeminiEmbedder {
    http: BlockingHttpWorker,
    base_url: String,
    api_key: String,
    /// The bare model name (`text-embedding-004`), stored as the
    /// collection pin. The `models/` prefix the API wants is added when
    /// building requests.
    model: String,
    dimensions: usize,
}

impl GeminiEmbedder {
    pub fn load(model: &str, base_url: &str, api_key: String) -> Result<Self> {
        if api_key.trim().is_empty() {
            return Err(RaError::Validation(
                "the Gemini embeddings provider needs an API key; set \
                 [embeddings].api_key_env to the name of an environment variable \
                 holding a Google AI Studio key"
                    .to_string(),
            ));
        }

        let http = BlockingHttpWorker::spawn(|| {
            reqwest::blocking::Client::builder()
                .timeout(TIMEOUT)
                .build()
        })?;

        let mut embedder = Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            // Stored without the `models/` prefix — see the field doc.
            model: model.trim().trim_start_matches("models/").to_string(),
            dimensions: 0,
        };

        // One real call: discovers the dimensionality and validates the
        // key, URL and model name in one shot at startup.
        let probe = embedder.embed(&["dimension probe".to_string()], EmbeddingTask::Document)?;
        let dimensions = probe.first().map(Vec::len).unwrap_or(0);
        if dimensions == 0 {
            return Err(RaError::Validation(format!(
                "Gemini returned an empty vector for model {model:?}; check that it is \
                 an embedding model (e.g. text-embedding-004)"
            )));
        }

        embedder.dimensions = dimensions;
        tracing::info!(model = %embedder.model, dimensions, "native Gemini embeddings enabled");
        Ok(embedder)
    }

    fn qualified_model(&self) -> String {
        format!("models/{}", self.model)
    }
}

/// The taskType Gemini should embed for. Storage, updates and
/// consolidation are all documents; only a recall query is a query.
fn task_type(task: EmbeddingTask) -> &'static str {
    match task {
        EmbeddingTask::Document => "RETRIEVAL_DOCUMENT",
        EmbeddingTask::Query => "RETRIEVAL_QUERY",
    }
}

impl Embedder for GeminiEmbedder {
    fn embed(&self, texts: &[String], task: EmbeddingTask) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let qualified = self.qualified_model();
        let type_of_task = task_type(task);
        let requests: Vec<Value> = texts
            .iter()
            .map(|text| {
                json!({
                    "model": qualified,
                    "content": {"parts": [{"text": text}]},
                    "taskType": type_of_task,
                })
            })
            .collect();

        let url = format!("{}/{}:batchEmbedContents", self.base_url, qualified);
        let body = json!({"requests": requests});

        // Everything the request needs is moved into the closure so it can
        // run on the confined HTTP thread. The outer `?` is the transport
        // result; the closure's own `Result` is the embedding result.
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let expected = texts.len();
        self.http.run(move |http| -> Result<Vec<Vec<f32>>> {
            let response = http
                .post(url)
                // Header auth, not `?key=`: the key must not land in a URL.
                .header("x-goog-api-key", &api_key)
                .json(&body)
                .send()
                .map_err(|e| {
                    RaError::Internal(format!("could not reach Gemini at {base_url}: {e}"))
                })?;

            let status = response.status();
            let body = response.text().unwrap_or_default();
            if !status.is_success() {
                return Err(map_status(status, &body));
            }

            let payload: Value = serde_json::from_str(&body)
                .map_err(|e| RaError::Internal(format!("Gemini returned invalid JSON: {e}")))?;

            let vectors = parse_embeddings(&payload)?;
            if vectors.len() != expected {
                return Err(RaError::Internal(format!(
                    "asked Gemini for {expected} embeddings, got {}",
                    vectors.len()
                )));
            }
            Ok(vectors)
        })?
    }

    fn model_id(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

/// `{"embeddings": [{"values": [...]}, ...]}`.
fn parse_embeddings(payload: &Value) -> Result<Vec<Vec<f32>>> {
    let rows = payload["embeddings"].as_array().ok_or_else(|| {
        RaError::Internal("Gemini response had no `embeddings` array".to_string())
    })?;

    rows.iter()
        .map(|row| {
            let values = row["values"].as_array().ok_or_else(|| {
                RaError::Internal("a Gemini embedding had no `values` array".to_string())
            })?;
            values
                .iter()
                .map(|number| {
                    number.as_f64().map(|f| f as f32).ok_or_else(|| {
                        RaError::Internal("a Gemini embedding value was not numeric".to_string())
                    })
                })
                .collect()
        })
        .collect()
}

/// 429 and 5xx are retryable; a bad key or model is not. Gemini also uses
/// 400 for an invalid API key on some paths, which is still permanent.
fn map_status(status: reqwest::StatusCode, body: &str) -> RaError {
    let message = format!(
        "Gemini returned HTTP {}: {}",
        status.as_u16(),
        summarise(body)
    );
    if status.is_server_error() || status.as_u16() == 429 {
        RaError::Internal(message)
    } else {
        RaError::Validation(message)
    }
}

fn summarise(body: &str) -> String {
    const LIMIT: usize = 300;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "(empty body)".to_string();
    }
    if trimmed.chars().count() <= LIMIT {
        return trimmed.to_string();
    }
    trimmed.chars().take(LIMIT).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_and_query_map_to_the_right_task_types() {
        // The whole reason this client exists. Getting these swapped
        // would quietly halve retrieval quality with no error anywhere.
        assert_eq!(task_type(EmbeddingTask::Document), "RETRIEVAL_DOCUMENT");
        assert_eq!(task_type(EmbeddingTask::Query), "RETRIEVAL_QUERY");
    }

    #[test]
    fn the_model_pin_drops_the_api_prefix_but_requests_carry_it() {
        let embedder = GeminiEmbedder {
            http: BlockingHttpWorker::spawn(|| Ok(reqwest::blocking::Client::new())).unwrap(),
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: "k".to_string(),
            model: "text-embedding-004".to_string(),
            dimensions: 768,
        };
        // Stored pin is bare; the request-time name is qualified.
        assert_eq!(embedder.model_id(), "text-embedding-004");
        assert_eq!(embedder.qualified_model(), "models/text-embedding-004");
    }

    #[test]
    fn the_response_shape_is_parsed() {
        let payload = json!({
            "embeddings": [
                {"values": [0.1, 0.2, 0.3]},
                {"values": [0.4, 0.5, 0.6]},
            ]
        });
        assert_eq!(
            parse_embeddings(&payload).unwrap(),
            vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5, 0.6]]
        );
    }

    #[test]
    fn a_wrong_shape_is_an_error_not_a_panic() {
        assert!(parse_embeddings(&json!({"data": []})).is_err());
        assert!(parse_embeddings(&json!({"embeddings": [{"nope": 1}]})).is_err());
        assert!(parse_embeddings(&json!({"embeddings": [{"values": ["x"]}]})).is_err());
    }

    #[test]
    fn a_missing_key_is_refused_before_any_request() {
        // GeminiEmbedder is intentionally not Debug (it holds an HTTP
        // client and a key), so match rather than `expect_err`.
        let error =
            match GeminiEmbedder::load("text-embedding-004", DEFAULT_BASE_URL, "  ".to_string()) {
                Ok(_) => panic!("an empty key must be refused"),
                Err(error) => error,
            };
        assert!(matches!(error, RaError::Validation(_)), "{error:?}");
    }

    #[test]
    fn a_bad_key_is_permanent_and_a_rate_limit_is_transient() {
        assert!(matches!(
            map_status(reqwest::StatusCode::BAD_REQUEST, "API key not valid"),
            RaError::Validation(_)
        ));
        assert!(matches!(
            map_status(reqwest::StatusCode::TOO_MANY_REQUESTS, "quota"),
            RaError::Internal(_)
        ));
    }

    // --- end to end against a mock Gemini ----------------------------

    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    struct GeminiVectors {
        dim: usize,
    }

    impl Respond for GeminiVectors {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(json!({}));
            let count = body["requests"].as_array().map(Vec::len).unwrap_or(1);
            let vector: Vec<f32> = (0..self.dim).map(|i| i as f32 * 0.1).collect();
            ResponseTemplate::new(200).set_body_json(json!({
                "embeddings": (0..count)
                    .map(|_| json!({"values": vector}))
                    .collect::<Vec<_>>(),
            }))
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stores_documents_and_searches_with_the_right_task_types() {
        let server = MockServer::start().await;

        // The store path: matches only a RETRIEVAL_DOCUMENT request that
        // also carries the qualified model and the api-key header.
        Mock::given(method("POST"))
            // The base URL under test is the bare mock root, so there is
            // no /v1beta segment here — in production it comes from
            // DEFAULT_BASE_URL. What matters is the model and the verb.
            .and(path("/models/text-embedding-004:batchEmbedContents"))
            .and(header("x-goog-api-key", "gk-test"))
            .and(body_partial_json(json!({
                "requests": [{"model": "models/text-embedding-004", "taskType": "RETRIEVAL_DOCUMENT"}]
            })))
            .respond_with(GeminiVectors { dim: 5 })
            .mount(&server)
            .await;

        let uri = server.uri();
        let embedder = tokio::task::spawn_blocking(move || {
            GeminiEmbedder::load("text-embedding-004", &uri, "gk-test".to_string())
        })
        .await
        .unwrap()
        .expect("load should probe successfully");

        // The probe used Document, so dimensions came back.
        assert_eq!(embedder.dimensions(), 5);
        assert_eq!(embedder.model_id(), "text-embedding-004");

        let docs = tokio::task::spawn_blocking(move || {
            embedder.embed(&["a".to_string(), "b".to_string()], EmbeddingTask::Document)
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].len(), 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_query_is_sent_as_retrieval_query() {
        let server = MockServer::start().await;
        // Only a RETRIEVAL_QUERY request matches; if the recall path sent
        // DOCUMENT, this would 404 and the embed would fail.
        Mock::given(method("POST"))
            .and(body_partial_json(json!({
                "requests": [{"taskType": "RETRIEVAL_QUERY"}]
            })))
            .respond_with(GeminiVectors { dim: 3 })
            .mount(&server)
            .await;
        // A permissive probe mock (Document) so `load` succeeds first.
        Mock::given(method("POST"))
            .and(body_partial_json(json!({
                "requests": [{"taskType": "RETRIEVAL_DOCUMENT"}]
            })))
            .respond_with(GeminiVectors { dim: 3 })
            .mount(&server)
            .await;

        let uri = server.uri();
        let embedder = tokio::task::spawn_blocking(move || {
            GeminiEmbedder::load("text-embedding-004", &uri, "gk-test".to_string())
        })
        .await
        .unwrap()
        .unwrap();

        let query = tokio::task::spawn_blocking(move || {
            embedder.embed_one("where do we deploy?", EmbeddingTask::Query)
        })
        .await
        .unwrap()
        .expect("a query must embed via RETRIEVAL_QUERY");
        assert_eq!(query.len(), 3);
    }
}
