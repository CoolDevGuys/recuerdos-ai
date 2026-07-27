//! Turns text into vectors.
//!
//! Owned by `memories` (the consumer), implemented in `providers` — see
//! boundary rule 5. Batch-shaped because every implementation is
//! dramatically faster per item on a batch, whether it's a local ONNX
//! model or a remote API.

use crate::shared::error::Result;

/// What a text is being embedded *for*.
///
/// Some providers — Gemini most notably — train separate document and
/// query representations of the same model and embed measurably better
/// when told which is which: the stored text and the search that should
/// find it are pushed *toward* each other rather than both toward some
/// neutral centre. A symmetric model (the local ONNX one, OpenAI's
/// endpoint) has no such distinction and ignores this.
///
/// The rule that keeps it correct: whatever a memory was stored as, every
/// later comparison of that *stored* text must use the same task — so
/// storage, updates and consolidation are all [`Document`], and only the
/// recall query is [`Query`]. Mixing the two for one corpus would compare
/// vectors that were never trained to be compared.
///
/// [`Document`]: EmbeddingTask::Document
/// [`Query`]: EmbeddingTask::Query
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingTask {
    /// Content being stored, or stored content re-embedded to compare
    /// against other stored content (consolidation).
    Document,
    /// A search query, embedded to be matched against stored documents.
    Query,
}

pub trait Embedder: Send + Sync {
    /// Embeds a batch, returning one vector per input, in order.
    ///
    /// `task` lets an asymmetric provider pick the right representation;
    /// symmetric ones ignore it. See [`EmbeddingTask`].
    fn embed(&self, texts: &[String], task: EmbeddingTask) -> Result<Vec<Vec<f32>>>;

    /// Identifies the model. Stored alongside the vectors: comparing
    /// embeddings from two different models is meaningless, so a
    /// collection pins the model that built it.
    fn model_id(&self) -> &str;

    fn dimensions(&self) -> usize;

    /// Convenience for the single-text case — a query, or one memory.
    fn embed_one(&self, text: &str, task: EmbeddingTask) -> Result<Vec<f32>> {
        let mut embeddings = self.embed(std::slice::from_ref(&text.to_string()), task)?;
        embeddings.pop().ok_or_else(|| {
            crate::shared::error::RaError::Internal(
                "embedder returned no vector for one input".to_string(),
            )
        })
    }
}
