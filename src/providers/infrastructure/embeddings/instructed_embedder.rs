//! Prepends a retrieval instruction to recall queries, for any provider.
//!
//! Some embedding models are trained asymmetrically *by text* rather than
//! by API flag: BGE v1.5 expects short queries to carry "Represent this
//! sentence for searching relevant passages: " and its stored passages to
//! carry nothing; E5 wants "query: ". The local model and the
//! OpenAI-compatible endpoint both serve such models but have no task
//! parameter, so the instruction has to travel in the text — and it has to
//! be the same whichever provider serves the model, which is why this
//! wraps an [`Embedder`] instead of living inside one.
//!
//! Only [`EmbeddingTask::Query`] is touched. Stored documents are embedded
//! exactly as before, so turning the instruction on or off needs no
//! reindex, and the model id — the collection pin — is passed through
//! unchanged.

use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::shared::error::Result;
use std::sync::Arc;

pub struct InstructedEmbedder {
    inner: Arc<dyn Embedder>,
    query_instruction: String,
}

impl InstructedEmbedder {
    /// Wraps `inner`, or returns it untouched when `query_instruction` is
    /// blank — "no instruction" should cost nothing, not even a wrapper.
    pub fn wrap(inner: Arc<dyn Embedder>, query_instruction: &str) -> Arc<dyn Embedder> {
        if query_instruction.trim().is_empty() {
            return inner;
        }
        Arc::new(Self {
            inner,
            query_instruction: query_instruction.to_string(),
        })
    }
}

impl Embedder for InstructedEmbedder {
    fn embed(&self, texts: &[String], task: EmbeddingTask) -> Result<Vec<Vec<f32>>> {
        match task {
            EmbeddingTask::Document => self.inner.embed(texts, task),
            EmbeddingTask::Query => {
                let instructed: Vec<String> = texts
                    .iter()
                    .map(|text| format!("{}{text}", self.query_instruction))
                    .collect();
                self.inner.embed(&instructed, task)
            }
        }
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records what it was asked to embed.
    #[derive(Default)]
    struct Recording {
        seen: Mutex<Vec<(String, EmbeddingTask)>>,
    }

    impl Embedder for Recording {
        fn embed(&self, texts: &[String], task: EmbeddingTask) -> Result<Vec<Vec<f32>>> {
            let mut seen = self.seen.lock().unwrap();
            seen.extend(texts.iter().map(|t| (t.clone(), task)));
            Ok(texts.iter().map(|_| vec![1.0]).collect())
        }

        fn model_id(&self) -> &str {
            "recording"
        }

        fn dimensions(&self) -> usize {
            1
        }
    }

    #[test]
    fn prefixes_queries_only() {
        let recording = Arc::new(Recording::default());
        let embedder = InstructedEmbedder::wrap(recording.clone(), "query: ");

        embedder
            .embed_one("which package manager?", EmbeddingTask::Query)
            .unwrap();
        embedder
            .embed_one("prefers pnpm", EmbeddingTask::Document)
            .unwrap();

        assert_eq!(
            *recording.seen.lock().unwrap(),
            vec![
                (
                    "query: which package manager?".to_string(),
                    EmbeddingTask::Query
                ),
                ("prefers pnpm".to_string(), EmbeddingTask::Document),
            ]
        );
    }

    #[test]
    fn keeps_the_inner_model_pin() {
        let embedder = InstructedEmbedder::wrap(Arc::new(Recording::default()), "query: ");
        assert_eq!(embedder.model_id(), "recording");
        assert_eq!(embedder.dimensions(), 1);
    }

    #[test]
    fn a_blank_instruction_is_no_wrapper_at_all() {
        let inner: Arc<dyn Embedder> = Arc::new(Recording::default());
        let wrapped = InstructedEmbedder::wrap(Arc::clone(&inner), "  ");
        assert!(Arc::ptr_eq(&inner, &wrapped));
    }
}
