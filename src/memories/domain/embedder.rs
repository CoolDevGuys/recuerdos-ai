//! Turns text into vectors.
//!
//! Owned by `memories` (the consumer), implemented in `providers` — see
//! boundary rule 5. Batch-shaped because every implementation is
//! dramatically faster per item on a batch, whether it's a local ONNX
//! model or a remote API.

use crate::shared::error::Result;

pub trait Embedder: Send + Sync {
    /// Embeds a batch, returning one vector per input, in order.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Identifies the model. Stored alongside the vectors: comparing
    /// embeddings from two different models is meaningless, so a
    /// collection pins the model that built it.
    fn model_id(&self) -> &str;

    fn dimensions(&self) -> usize;

    /// Convenience for the single-text case (a query).
    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut embeddings = self.embed(std::slice::from_ref(&text.to_string()))?;
        embeddings.pop().ok_or_else(|| {
            crate::shared::error::RaError::Internal(
                "embedder returned no vector for one input".to_string(),
            )
        })
    }
}
