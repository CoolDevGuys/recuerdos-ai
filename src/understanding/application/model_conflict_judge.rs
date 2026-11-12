//! Judges a save's conflicts with the configured language model: one
//! request per save, covering both the contradiction check (R8) and the
//! model layer of the scope check (R7).

use crate::memories::domain::conflict_judge::{ConflictJudge, ConflictQuestion, Judgement};
use crate::shared::error::Result;
use crate::understanding::domain::chat_model::ChatModel;
use crate::understanding::domain::conflict_prompt::{conflict_request, parse_judgement};
use std::sync::Arc;

pub struct ModelConflictJudge {
    model: Arc<dyn ChatModel>,
}

impl ModelConflictJudge {
    pub fn new(model: Arc<dyn ChatModel>) -> Self {
        Self { model }
    }
}

#[async_trait::async_trait]
impl ConflictJudge for ModelConflictJudge {
    async fn judge(&self, question: &ConflictQuestion) -> Result<Judgement> {
        let answer = self
            .model
            .complete_structured(&conflict_request(question))
            .await?;
        Ok(parse_judgement(&answer, question))
    }
}
