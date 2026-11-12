//! Whether a new memory contradicts what is already stored
//! (ai-specs/memory-structure-plan.md §5, rule R8), and whether its text fits
//! the scope it was given (the model layer of R7).
//!
//! The question is asked once per save, about the nearest stored memories in
//! the same scope. A store that accepts "the user avoids mocks" next to "the
//! user prefers mocks" hands both to every later recall, and the agent then
//! guesses which is current. Refusing the second and asking the user is
//! cheaper than either guess.
//!
//! The contract lives here; judging with a language model lives in the
//! understanding context, which owns the model. Without one, the saver
//! applies a stricter, model-free rule of its own (see
//! `StructuredMemorySaver`).

use super::applies_to::AppliesTo;
use super::kind::Kind;
use super::memory::Memory;
use super::topic::Topic;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;

/// What one stored memory is to the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// They cannot both be true: the same subject, a different answer.
    Contradicts,
    /// The new memory says nothing the stored one does not.
    Same,
    /// Related, or not, but both can stand.
    Unrelated,
}

/// A stored memory the new one is compared against.
#[derive(Debug, Clone)]
pub struct Neighbour {
    pub memory: Memory,
    /// How close recall found it, `0.0..=1.0`.
    pub relevance: f32,
    /// A memory about the user, compared against a project memory. A
    /// project may make an exception to the user's own standard, so a
    /// contradiction here is noted as an override, never refused (§5.1).
    pub personal: bool,
}

/// One save's question: the new memory, as it would be filed, and its
/// nearest neighbours.
#[derive(Debug, Clone)]
pub struct ConflictQuestion {
    pub content: String,
    pub kind: Kind,
    pub topic: Topic,
    pub applies_to: AppliesTo,
    pub neighbours: Vec<Neighbour>,
    /// Whether to also judge if the text fits `applies_to`. Only asked when
    /// it would be filed as `me` or `general`: a project memory cannot claim
    /// too much.
    pub check_scope: bool,
}

impl ConflictQuestion {
    /// Whether there is anything to ask a model at all.
    pub fn is_empty(&self) -> bool {
        self.neighbours.is_empty() && !self.check_scope
    }
}

/// The verdict on one neighbour, with the reason to show the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeighbourVerdict {
    pub memory_id: MemoryId,
    pub verdict: Verdict,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Judgement {
    /// One per neighbour the judge spoke about. A neighbour left out is
    /// treated as unrelated.
    pub verdicts: Vec<NeighbourVerdict>,
    /// `Some(reason)` when the text does not fit its `applies_to` — it reads
    /// as one project's, but would be filed as the user's or as general.
    pub scope_mismatch: Option<String>,
}

#[async_trait::async_trait]
pub trait ConflictJudge: Send + Sync {
    async fn judge(&self, question: &ConflictQuestion) -> Result<Judgement>;
}
