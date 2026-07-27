//! A deterministic `Embedder` for tests.
//!
//! Every use-case and integration test in the project depends on this:
//! it makes the whole pipeline runnable offline, in milliseconds, with
//! reproducible results. The real model takes seconds to load and needs
//! a download on a cold cache — unusable as a per-test dependency.
//!
//! # Why it is not just random numbers
//!
//! The fake reproduces the one property tests actually assert on:
//! **texts sharing words embed closer together**. It hashes each word
//! into a fixed set of dimensions, so "user prefers pnpm" lands near
//! "which package manager, pnpm?" and far from "the cat sat on the mat".
//!
//! That makes a test like "a paraphrase is recalled" meaningful rather
//! than accidental. It is a bag-of-words stand-in, not a language model:
//! true synonyms ("car"/"automobile") are *not* close, so tests must use
//! shared vocabulary to express semantic proximity.

use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::shared::error::Result;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub const FAKE_MODEL: &str = "fake-hash-embedder";

pub struct FakeEmbedder {
    dimensions: usize,
}

impl FakeEmbedder {
    pub fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }
}

impl Default for FakeEmbedder {
    fn default() -> Self {
        Self::new(64)
    }
}

impl Embedder for FakeEmbedder {
    fn embed(&self, texts: &[String], _task: EmbeddingTask) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                let mut vector = vec![0.0f32; self.dimensions];

                for word in text
                    .to_ascii_lowercase()
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|word| !word.is_empty())
                {
                    let mut hasher = DefaultHasher::new();
                    word.hash(&mut hasher);
                    let hash = hasher.finish();

                    // Two dimensions per word, so distinct words rarely
                    // collide entirely and shared words always overlap.
                    vector[(hash % self.dimensions as u64) as usize] += 1.0;
                    vector[((hash >> 32) % self.dimensions as u64) as usize] += 0.5;
                }

                normalize(vector)
            })
            .collect())
    }

    fn model_id(&self) -> &str {
        FAKE_MODEL
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

/// Unit-normalised, so cosine and euclidean distance agree on ordering —
/// sqlite-vec ranks by L2, and without this a text's *length* would
/// dominate its distance.
fn normalize(mut vector: Vec<f32>) -> Vec<f32> {
    let norm: f32 = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embed(embedder: &FakeEmbedder, text: &str) -> Vec<f32> {
        embedder
            .embed(&[text.to_string()], EmbeddingTask::Document)
            .unwrap()
            .pop()
            .unwrap()
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn is_deterministic() {
        let embedder = FakeEmbedder::default();
        assert_eq!(
            embed(&embedder, "user prefers pnpm"),
            embed(&embedder, "user prefers pnpm")
        );
    }

    #[test]
    fn produces_vectors_of_the_configured_width() {
        let embedder = FakeEmbedder::new(16);
        assert_eq!(embed(&embedder, "anything").len(), 16);
    }

    #[test]
    fn texts_sharing_words_are_closer_than_unrelated_ones() {
        // The property the downstream recall tests rely on.
        let embedder = FakeEmbedder::default();

        let subject = embed(&embedder, "user prefers pnpm as package manager");
        let related = embed(&embedder, "which package manager does the user prefer");
        let unrelated = embed(&embedder, "the cat sat on the mat");

        assert!(
            cosine(&subject, &related) > cosine(&subject, &unrelated),
            "shared vocabulary should be closer than none"
        );
    }

    #[test]
    fn is_case_and_punctuation_insensitive() {
        let embedder = FakeEmbedder::default();
        assert_eq!(
            embed(&embedder, "Prefers pnpm!"),
            embed(&embedder, "prefers, pnpm")
        );
    }

    #[test]
    fn vectors_are_unit_length() {
        let embedder = FakeEmbedder::default();
        let vector = embed(&embedder, "user prefers pnpm");
        let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();

        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
    }

    #[test]
    fn an_empty_text_yields_a_zero_vector_rather_than_nan() {
        let embedder = FakeEmbedder::default();
        let vector = embed(&embedder, "");

        assert!(vector.iter().all(|value| *value == 0.0));
        assert!(vector.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn embeds_a_batch_in_order() {
        let embedder = FakeEmbedder::default();
        let batch = embedder
            .embed(
                &["first".to_string(), "second".to_string()],
                EmbeddingTask::Document,
            )
            .unwrap();

        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0], embed(&embedder, "first"));
        assert_eq!(batch[1], embed(&embedder, "second"));
    }

    #[test]
    fn embed_one_returns_a_single_vector() {
        let embedder = FakeEmbedder::default();
        assert_eq!(
            embedder
                .embed_one("user prefers pnpm", EmbeddingTask::Query)
                .unwrap(),
            embed(&embedder, "user prefers pnpm")
        );
    }
}
