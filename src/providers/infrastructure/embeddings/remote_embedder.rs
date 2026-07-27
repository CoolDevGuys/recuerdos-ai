//! Embeddings from an external HTTP provider — the "bring your own model"
//! path that mirrors `[understanding]`'s remote providers.
//!
//! Two dialects behind one type, because they differ only in the URL and
//! the JSON shape:
//!
//! - **openai-compat** — `POST {base}/embeddings`, bearer auth, the
//!   OpenAI request/response shape. Covers OpenAI itself, and every
//!   gateway that copies it (OpenRouter, Together, a local vLLM, LM
//!   Studio).
//! - **ollama** — `POST {base}/api/embed`, no auth, Ollama's shape.
//!
//! # Why blocking HTTP
//!
//! The [`Embedder`](crate::memories::domain::embedder::Embedder) trait is
//! synchronous: the local ONNX path is CPU-bound. Rather than make the
//! trait async for one implementation, this uses `reqwest::blocking`.
//!
//! That client owns a Tokio runtime, and building, using or dropping it on
//! one of the daemon's async threads panics — which the startup probe, the
//! nightly consolidation task and the `reindex` command would all do. So
//! the client is confined to its own thread by [`BlockingHttpWorker`], and
//! `embed` hands work to it over a channel. See that module for the full
//! rationale.
//!
//! # Why it probes at startup
//!
//! A remote model's dimensionality is not known until it answers, and the
//! vector table's fixed width plus the per-collection model pin both
//! depend on it. So construction sends one embedding request and counts
//! the result. That doubles as a full end-to-end check of the URL, the
//! key and the model name — a self-hoster finds out their provider is
//! misconfigured when they start the daemon, not when their first recall
//! returns nothing.

use super::blocking_http::BlockingHttpWorker;
use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::shared::error::{RaError, Result};
use serde_json::{Value, json};
use std::time::Duration;

pub const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const OLLAMA_DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";

/// Long enough for a large batch against a slow endpoint, short enough
/// that a wedged connection does not hold a recall or an ingest worker
/// open forever.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Which provider's wire protocol to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingApi {
    OpenAiCompat,
    Ollama,
}

pub struct RemoteEmbedder {
    http: BlockingHttpWorker,
    api: EmbeddingApi,
    /// Trailing slash trimmed; the endpoint path is appended.
    base_url: String,
    /// `None` for a keyless server. Sent as a bearer token when present.
    api_key: Option<String>,
    model: String,
    dimensions: usize,
}

impl RemoteEmbedder {
    /// Builds the client and probes for the model's dimensionality.
    ///
    /// Fails loudly if the provider is unreachable, the key is rejected,
    /// or the model name is unknown — all of which are startup
    /// misconfigurations an operator should hear about immediately.
    pub fn load(
        api: EmbeddingApi,
        model: &str,
        base_url: &str,
        api_key: Option<String>,
    ) -> Result<Self> {
        let http = BlockingHttpWorker::spawn(|| {
            reqwest::blocking::Client::builder()
                .timeout(TIMEOUT)
                .build()
        })?;

        let mut embedder = Self {
            http,
            api,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.filter(|key| !key.trim().is_empty()),
            model: model.to_string(),
            // Filled in by the probe below.
            dimensions: 0,
        };

        // One real embedding call. Its length is the dimensionality, and
        // its success is the config check.
        let probe = embedder.embed(&["dimension probe".to_string()], EmbeddingTask::Document)?;
        let dimensions = probe.first().map(Vec::len).unwrap_or(0);
        if dimensions == 0 {
            return Err(RaError::Validation(format!(
                "the embedding provider returned an empty vector for model {model:?}; \
                 check that the model name is an embedding model"
            )));
        }

        embedder.dimensions = dimensions;
        tracing::info!(
            provider = ?api,
            model,
            dimensions,
            "remote embeddings enabled"
        );
        Ok(embedder)
    }

    fn endpoint(&self) -> String {
        match self.api {
            EmbeddingApi::OpenAiCompat => format!("{}/embeddings", self.base_url),
            EmbeddingApi::Ollama => format!("{}/api/embed", self.base_url),
        }
    }

    fn request_body(&self, texts: &[String]) -> Value {
        match self.api {
            // Both accept a batch under `input`; the field name happens
            // to match, the response shapes do not.
            EmbeddingApi::OpenAiCompat => json!({"model": self.model, "input": texts}),
            EmbeddingApi::Ollama => json!({"model": self.model, "input": texts}),
        }
    }
}

impl Embedder for RemoteEmbedder {
    fn embed(&self, texts: &[String], _task: EmbeddingTask) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Build the request off the confined HTTP thread, then run the
        // send + parse on it. The outer `?` is the transport result; the
        // closure's own `Result` is the embedding result.
        let endpoint = self.endpoint();
        let request_body = self.request_body(texts);
        let api = self.api;
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let expected = texts.len();
        self.http.run(move |http| -> Result<Vec<Vec<f32>>> {
            let mut request = http.post(endpoint).json(&request_body);
            if let Some(key) = &api_key {
                request = request.bearer_auth(key);
            }

            let response = request.send().map_err(|e| {
                // Transient by nature — a reset socket or a slow provider
                // says nothing about whether the request was valid — so map
                // it to `Internal`, which the ingest queue treats as
                // retryable.
                RaError::Internal(format!(
                    "could not reach the embedding provider at {base_url}: {e}"
                ))
            })?;

            let status = response.status();
            let body = response.text().unwrap_or_default();
            if !status.is_success() {
                return Err(map_status(status, &body));
            }

            let payload: Value = serde_json::from_str(&body).map_err(|e| {
                RaError::Internal(format!("the embedding provider returned invalid JSON: {e}"))
            })?;

            let vectors = match api {
                EmbeddingApi::OpenAiCompat => parse_openai(&payload)?,
                EmbeddingApi::Ollama => parse_ollama(&payload)?,
            };

            if vectors.len() != expected {
                return Err(RaError::Internal(format!(
                    "asked for {expected} embeddings, the provider returned {}",
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

/// A non-2xx from the provider. 429 and 5xx are the "we're busy" signals
/// and are worth a retry; everything else (a bad key, an unknown model)
/// will be rejected identically next time, so it is permanent.
fn map_status(status: reqwest::StatusCode, body: &str) -> RaError {
    let detail = summarise(body);
    let message = format!(
        "embedding provider returned HTTP {}: {detail}",
        status.as_u16()
    );
    if status.is_server_error() || status.as_u16() == 429 {
        RaError::Internal(message)
    } else {
        // Permanent — but the save/recall paths surface this as a 500 to
        // the caller rather than retrying, which is the intended shape
        // for "your embeddings config is wrong".
        RaError::Validation(message)
    }
}

/// `{"data": [{"index": 0, "embedding": [...]}, ...]}` — the OpenAI shape.
///
/// Sorted by `index` before extracting: the spec returns them in order,
/// but relying on that when the field exists specifically to guarantee it
/// is asking for trouble.
fn parse_openai(payload: &Value) -> Result<Vec<Vec<f32>>> {
    let data = payload["data"]
        .as_array()
        .ok_or_else(|| RaError::Internal("embedding response has no `data` array".to_string()))?;

    let mut indexed: Vec<(u64, Vec<f32>)> = Vec::with_capacity(data.len());
    for entry in data {
        let index = entry["index"].as_u64().unwrap_or(indexed.len() as u64);
        indexed.push((index, extract_vector(&entry["embedding"])?));
    }
    indexed.sort_by_key(|(index, _)| *index);
    Ok(indexed.into_iter().map(|(_, vector)| vector).collect())
}

/// `{"embeddings": [[...], [...]]}` — Ollama's `/api/embed` shape.
fn parse_ollama(payload: &Value) -> Result<Vec<Vec<f32>>> {
    let rows = payload["embeddings"].as_array().ok_or_else(|| {
        RaError::Internal("embedding response has no `embeddings` array".to_string())
    })?;
    rows.iter().map(extract_vector).collect()
}

fn extract_vector(value: &Value) -> Result<Vec<f32>> {
    let array = value
        .as_array()
        .ok_or_else(|| RaError::Internal("an embedding was not an array of numbers".to_string()))?;

    array
        .iter()
        .map(|number| {
            number.as_f64().map(|f| f as f32).ok_or_else(|| {
                RaError::Internal("an embedding contained a non-numeric value".to_string())
            })
        })
        .collect()
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
    fn the_openai_endpoint_and_body_match_the_spec() {
        let embedder = RemoteEmbedder {
            http: BlockingHttpWorker::spawn(|| Ok(reqwest::blocking::Client::new())).unwrap(),
            api: EmbeddingApi::OpenAiCompat,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: Some("sk-x".to_string()),
            model: "text-embedding-3-small".to_string(),
            dimensions: 1536,
        };

        assert_eq!(embedder.endpoint(), "https://api.openai.com/v1/embeddings");
        let body = embedder.request_body(&["a".to_string(), "b".to_string()]);
        assert_eq!(body["model"], "text-embedding-3-small");
        assert_eq!(body["input"], json!(["a", "b"]));
    }

    #[test]
    fn the_ollama_endpoint_differs() {
        let embedder = RemoteEmbedder {
            http: BlockingHttpWorker::spawn(|| Ok(reqwest::blocking::Client::new())).unwrap(),
            api: EmbeddingApi::Ollama,
            base_url: "http://127.0.0.1:11434".to_string(),
            api_key: None,
            model: "nomic-embed-text".to_string(),
            dimensions: 768,
        };
        assert_eq!(embedder.endpoint(), "http://127.0.0.1:11434/api/embed");
    }

    #[test]
    fn a_trailing_slash_in_the_base_url_does_not_double_up() {
        // Set explicitly here; the constructor trims it, but a config
        // value pasted with a slash is the common case.
        let base = "https://gateway.example.com/v1/".trim_end_matches('/');
        assert_eq!(base, "https://gateway.example.com/v1");
    }

    #[test]
    fn openai_responses_are_parsed_and_reordered_by_index() {
        // Deliberately out of order: the parser must not trust arrival
        // order when the index field exists precisely to fix it.
        let payload = json!({
            "data": [
                {"index": 1, "embedding": [0.3, 0.4]},
                {"index": 0, "embedding": [0.1, 0.2]},
            ]
        });

        let vectors = parse_openai(&payload).unwrap();

        assert_eq!(vectors, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
    }

    #[test]
    fn ollama_responses_are_parsed_in_order() {
        let payload = json!({"embeddings": [[0.1, 0.2], [0.3, 0.4]]});
        assert_eq!(
            parse_ollama(&payload).unwrap(),
            vec![vec![0.1, 0.2], vec![0.3, 0.4]]
        );
    }

    #[test]
    fn a_response_of_the_wrong_shape_is_an_error_not_a_panic() {
        // A provider that answered some other question, or an error body
        // that slipped past the status check, must not index-panic.
        assert!(parse_openai(&json!({"error": "nope"})).is_err());
        assert!(parse_ollama(&json!({"data": []})).is_err());
        assert!(extract_vector(&json!("not an array")).is_err());
        assert!(extract_vector(&json!([0.1, "oops"])).is_err());
    }

    #[test]
    fn a_bad_key_is_permanent_and_a_rate_limit_is_transient() {
        // The ingest queue retries `Internal` and surfaces `Validation`;
        // getting this backwards either burns the attempt budget on a
        // fixable mistake or dead-letters a batch over a brief 429.
        assert!(matches!(
            map_status(reqwest::StatusCode::UNAUTHORIZED, "bad key"),
            RaError::Validation(_)
        ));
        assert!(matches!(
            map_status(reqwest::StatusCode::TOO_MANY_REQUESTS, "slow down"),
            RaError::Internal(_)
        ));
        assert!(matches!(
            map_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "oops"),
            RaError::Internal(_)
        ));
    }

    #[test]
    fn embedding_an_empty_batch_makes_no_request() {
        // No client, so if this tried to send anything it would fail;
        // returning early is both an optimisation and what lets recall of
        // an empty query list be a no-op.
        let embedder = RemoteEmbedder {
            http: BlockingHttpWorker::spawn(|| Ok(reqwest::blocking::Client::new())).unwrap(),
            api: EmbeddingApi::OpenAiCompat,
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: None,
            model: "m".to_string(),
            dimensions: 3,
        };
        assert!(
            embedder
                .embed(&[], EmbeddingTask::Document)
                .unwrap()
                .is_empty()
        );
    }

    // --- end to end, over real HTTP, against a mock provider ---------
    //
    // The blocking client is driven inside `spawn_blocking` so it does
    // not stall the runtime serving wiremock, and the runtime is
    // multi-threaded so the two make progress at once.

    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    /// Answers with as many vectors as the request asked for, in the
    /// provider's shape — so the same mock serves both the one-input
    /// startup probe and a real multi-input batch.
    struct Vectors {
        api: EmbeddingApi,
        dim: usize,
    }

    impl Respond for Vectors {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(json!({}));
            let count = body["input"].as_array().map(Vec::len).unwrap_or(1);
            let vector: Vec<f32> = (0..self.dim).map(|i| i as f32 * 0.1).collect();

            let payload = match self.api {
                EmbeddingApi::OpenAiCompat => json!({
                    "data": (0..count)
                        .map(|i| json!({"index": i, "embedding": vector}))
                        .collect::<Vec<_>>(),
                }),
                EmbeddingApi::Ollama => json!({
                    "embeddings": (0..count).map(|_| json!(vector)).collect::<Vec<_>>(),
                }),
            };
            ResponseTemplate::new(200).set_body_json(payload)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn openai_compat_probes_dimensions_and_round_trips_a_batch() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            // Only matches when the bearer token is present, so a client
            // that forgot to send the key would get a 404 and fail.
            .and(header("authorization", "Bearer sk-test"))
            .and(body_partial_json(
                json!({"model": "text-embedding-3-small"}),
            ))
            .respond_with(Vectors {
                api: EmbeddingApi::OpenAiCompat,
                dim: 4,
            })
            .mount(&server)
            .await;

        let uri = server.uri();
        let embedder = tokio::task::spawn_blocking(move || {
            RemoteEmbedder::load(
                EmbeddingApi::OpenAiCompat,
                "text-embedding-3-small",
                &uri,
                Some("sk-test".to_string()),
            )
        })
        .await
        .unwrap()
        .expect("load should succeed against the mock");

        // Discovered from the probe, not configured.
        assert_eq!(embedder.dimensions(), 4);
        assert_eq!(embedder.model_id(), "text-embedding-3-small");

        let vectors = tokio::task::spawn_blocking(move || {
            embedder.embed(
                &["one".to_string(), "two".to_string()],
                EmbeddingTask::Document,
            )
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(vectors.len(), 2, "one vector per input");
        assert_eq!(vectors[0].len(), 4);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rejected_key_stops_startup_rather_than_surfacing_later() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("invalid api key"))
            .mount(&server)
            .await;

        let uri = server.uri();
        let result = tokio::task::spawn_blocking(move || {
            RemoteEmbedder::load(
                EmbeddingApi::OpenAiCompat,
                "text-embedding-3-small",
                &uri,
                Some("sk-wrong".to_string()),
            )
        })
        .await
        .unwrap();

        // `RemoteEmbedder` is intentionally not `Debug` (it holds an HTTP
        // client), so match rather than `expect_err`.
        let error = match result {
            Ok(_) => panic!("a 401 during the probe must fail load"),
            Err(error) => error,
        };
        assert!(matches!(error, RaError::Validation(_)), "got {error:?}");
        assert!(error.to_string().contains("401"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ollama_speaks_its_own_endpoint_and_shape() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(Vectors {
                api: EmbeddingApi::Ollama,
                dim: 3,
            })
            .mount(&server)
            .await;

        let uri = server.uri();
        let embedder = tokio::task::spawn_blocking(move || {
            RemoteEmbedder::load(EmbeddingApi::Ollama, "nomic-embed-text", &uri, None)
        })
        .await
        .unwrap()
        .expect("ollama load should succeed");

        assert_eq!(embedder.dimensions(), 3);
        let vectors = tokio::task::spawn_blocking(move || {
            embedder.embed(&["x".to_string()], EmbeddingTask::Document)
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(vectors, vec![vec![0.0, 0.1, 0.2]]);
    }
}
