//! Local ONNX embeddings — the zero-configuration default.
//!
//! This is what makes "works with no API key, no network, no account" a
//! real claim rather than a roadmap item: the service embeds on CPU,
//! in-process, with nothing configured.
//!
//! # Model cache
//!
//! fastembed downloads its model on first use and caches it. Left to its
//! default that cache lands in the *current working directory*, which for
//! a daemon means "wherever it happened to be started from". The cache
//! directory is therefore always set explicitly, and the Docker image
//! bakes the model in at build time so a container never downloads at
//! all — the first `POST /v1/memories` after `docker run` should not
//! depend on HuggingFace being reachable.

use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::shared::error::{RaError, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::path::PathBuf;
use std::sync::Mutex;

/// Dimensionality of every model this accepts. The vec0 table's fixed
/// width and the per-collection pin both depend on it, which is why
/// `resolve_model` refuses models that don't match.
pub const DEFAULT_DIMENSIONS: usize = 384;

pub struct FastembedEmbedder {
    // `TextEmbedding::embed` needs &mut, and embedding is CPU-bound
    // anyway, so a mutex is both required and honest about what it costs.
    // Callers already run this on a blocking thread.
    model: Mutex<TextEmbedding>,
    model_id: String,
    dimensions: usize,
}

impl FastembedEmbedder {
    /// Loads the model, downloading it into `cache_dir` if absent.
    ///
    /// Slow (seconds) and possibly network-bound, so it happens once at
    /// startup rather than lazily on the first request — a self-hoster
    /// should find out that their model is unreachable when they start
    /// the daemon, not when their agent first tries to save something.
    pub fn load(model_name: &str, cache_dir: PathBuf) -> Result<Self> {
        let model = resolve_model(model_name)?;

        std::fs::create_dir_all(&cache_dir).map_err(|e| {
            RaError::Internal(format!(
                "failed to create the model cache directory {}: {e}",
                cache_dir.display()
            ))
        })?;

        let embedding = TextEmbedding::try_new(
            InitOptions::new(model.clone())
                .with_cache_dir(cache_dir.clone())
                .with_show_download_progress(false),
        )
        .map_err(|e| {
            RaError::Internal(format!(
                "failed to load embedding model {model_name:?} from {}: {e}. \
                 If this host has no internet access, pre-populate the cache \
                 directory or set [embeddings].provider to one that doesn't \
                 need a local model.",
                cache_dir.display()
            ))
        })?;

        Ok(Self {
            model: Mutex::new(embedding),
            model_id: model_name.to_string(),
            dimensions: DEFAULT_DIMENSIONS,
        })
    }
}

impl Embedder for FastembedEmbedder {
    fn embed(&self, texts: &[String], _task: EmbeddingTask) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut model = self
            .model
            .lock()
            .map_err(|_| RaError::Internal("embedding model mutex poisoned".to_string()))?;

        let embeddings = model
            .embed(texts, None)
            .map_err(|e| RaError::Internal(format!("failed to embed text: {e}")))?;

        if embeddings.len() != texts.len() {
            return Err(RaError::Internal(format!(
                "embedder returned {} vectors for {} inputs",
                embeddings.len(),
                texts.len()
            )));
        }

        Ok(embeddings)
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

fn resolve_model(name: &str) -> Result<EmbeddingModel> {
    // Only models whose dimensionality matches DEFAULT_DIMENSIONS are
    // listed. Adding one with different dims means making `dimensions`
    // vary with the model, which the vec0 table's fixed width and the
    // collection pin both depend on.
    match name.trim().to_ascii_lowercase().as_str() {
        "bge-small-en-v1.5" => Ok(EmbeddingModel::BGESmallENV15),
        "all-minilm-l6-v2" => Ok(EmbeddingModel::AllMiniLML6V2),
        other => Err(RaError::Validation(format!(
            "unknown embedding model {other:?} (expected bge-small-en-v1.5 or \
             all-minilm-l6-v2)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_unknown_model_by_name() {
        let error = resolve_model("text-embedding-3-large").unwrap_err();
        assert!(matches!(error, RaError::Validation(_)), "got {error:?}");
        assert!(error.to_string().contains("bge-small-en-v1.5"));
    }

    #[test]
    fn resolves_the_supported_models() {
        assert!(
            resolve_model("BGE-Small-EN-v1.5").is_ok(),
            "case-insensitive"
        );
        assert!(resolve_model("  all-minilm-l6-v2 ").is_ok(), "trimmed");
    }

    /// Exercises the real model. Ignored by default because it needs the
    /// model present (and a download on a cold cache), which would make
    /// the ordinary suite depend on the network. The nightly CI job runs
    /// the ignored tests.
    #[test]
    #[ignore = "requires the ONNX model; run with --ignored"]
    fn embeds_text_into_vectors_of_the_expected_width() {
        let embedder = FastembedEmbedder::load("bge-small-en-v1.5", model_cache_dir()).unwrap();

        let embeddings = embedder
            .embed(
                &[
                    "User prefers pnpm over npm".to_string(),
                    "The deployment target is Hetzner".to_string(),
                ],
                EmbeddingTask::Document,
            )
            .unwrap();

        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0].len(), DEFAULT_DIMENSIONS);
        assert!(embeddings[0].iter().any(|value| *value != 0.0));
    }

    #[test]
    #[ignore = "requires the ONNX model; run with --ignored"]
    fn places_paraphrases_closer_than_unrelated_text() {
        // The property the whole vector leg rests on. If this fails, the
        // model is loading but not doing anything useful.
        let embedder = FastembedEmbedder::load("bge-small-en-v1.5", model_cache_dir()).unwrap();

        let embeddings = embedder
            .embed(
                &[
                    "User prefers pnpm as their package manager".to_string(),
                    "Which package manager should I use?".to_string(),
                    "The cat sat on the mat".to_string(),
                ],
                EmbeddingTask::Document,
            )
            .unwrap();

        let paraphrase = cosine(&embeddings[0], &embeddings[1]);
        let unrelated = cosine(&embeddings[0], &embeddings[2]);

        assert!(
            paraphrase > unrelated,
            "paraphrase similarity {paraphrase} should exceed unrelated {unrelated}"
        );
    }

    #[test]
    #[ignore = "requires the ONNX model; run with --ignored"]
    fn embedding_is_deterministic() {
        let embedder = FastembedEmbedder::load("bge-small-en-v1.5", model_cache_dir()).unwrap();
        let text = vec!["User prefers pnpm".to_string()];

        let first = embedder.embed(&text, EmbeddingTask::Document).unwrap();
        let second = embedder.embed(&text, EmbeddingTask::Document).unwrap();

        assert_eq!(first, second, "the same text must embed identically");
    }

    #[test]
    #[ignore = "requires the ONNX model; run with --ignored"]
    fn an_empty_batch_is_not_an_error() {
        let embedder = FastembedEmbedder::load("bge-small-en-v1.5", model_cache_dir()).unwrap();
        assert!(
            embedder
                .embed(&[], EmbeddingTask::Document)
                .unwrap()
                .is_empty()
        );
    }

    fn model_cache_dir() -> PathBuf {
        std::env::var("RECUERDOS_AI_MODEL_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/models"))
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (norm_a * norm_b)
    }
}
