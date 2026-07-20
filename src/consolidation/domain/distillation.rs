//! What a session distillation is asked to do, and what came of it.
//!
//! Value objects rather than loose arguments, for the usual reason: a
//! transcript and a session id are both strings, and transposing them
//! would send the session id to the model and file the transcript as
//! metadata — a mistake that produces no error anywhere.

use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;

/// Beyond this, a transcript is not a session — it is a corpus, and
/// sending it costs more in one call than the memories it yields are
/// worth.
///
/// Generous on purpose: roughly 50k tokens, which covers a long working
/// session comfortably. A caller with more than this has a summarisation
/// problem upstream, and telling them so beats an opaque provider error
/// or a bill they did not expect.
pub const MAX_TRANSCRIPT_LEN: usize = 200_000;

/// A finished session, handed over to be reduced to what survives it.
#[derive(Debug, Clone)]
pub struct SessionTranscript {
    /// The raw material: a transcript, or a summary of one.
    content: String,
    /// The client's own id for the session, carried onto every memory so
    /// "where did this come from?" can be answered later.
    pub session_id: Option<String>,
    /// Which client ran the session — `claude-code`, `opencode`.
    pub client: Option<String>,
    /// Tags applied to everything distilled out of it.
    pub tags: Vec<String>,
}

impl SessionTranscript {
    pub fn new(content: impl Into<String>) -> Result<Self> {
        let content = content.into();
        let trimmed = content.trim();

        if trimmed.is_empty() {
            return Err(RaError::Validation(
                "there is nothing to distil from an empty session".to_string(),
            ));
        }
        if trimmed.chars().count() > MAX_TRANSCRIPT_LEN {
            return Err(RaError::Validation(format!(
                "session is longer than {MAX_TRANSCRIPT_LEN} characters; summarize it \
                 before distilling"
            )));
        }

        Ok(Self {
            content: trimmed.to_string(),
            session_id: None,
            client: None,
            tags: Vec::new(),
        })
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn from(mut self, client: Option<String>, session_id: Option<String>) -> Self {
        self.client = client;
        self.session_id = session_id;
        self
    }

    pub fn tagged(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}

/// What a session left behind.
#[derive(Debug, Clone, Default)]
pub struct Distillation {
    /// The memories stored. Empty is the common, correct answer: most
    /// sessions produce nothing that outlives them.
    pub memory_ids: Vec<MemoryId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transcript_is_trimmed_and_kept() {
        let transcript = SessionTranscript::new("  we decided on SQLite  ").unwrap();
        assert_eq!(transcript.content(), "we decided on SQLite");
    }

    #[test]
    fn an_empty_session_is_rejected_rather_than_sent_to_a_model() {
        for content in ["", "   ", "\n\t"] {
            assert!(
                SessionTranscript::new(content).is_err(),
                "{content:?} should be rejected"
            );
        }
    }

    #[test]
    fn an_oversized_session_is_refused_with_advice() {
        let error = SessionTranscript::new("x".repeat(MAX_TRANSCRIPT_LEN + 1)).unwrap_err();

        assert!(
            error.to_string().contains("summarize it"),
            "the caller needs to know what to do about it: {error}"
        );
    }

    #[test]
    fn a_session_at_the_limit_is_accepted() {
        assert!(SessionTranscript::new("x".repeat(MAX_TRANSCRIPT_LEN)).is_ok());
    }

    #[test]
    fn provenance_rides_along_with_the_transcript() {
        let transcript = SessionTranscript::new("content")
            .unwrap()
            .from(Some("claude-code".to_string()), Some("s-42".to_string()))
            .tagged(vec!["backend".to_string()]);

        assert_eq!(transcript.client.as_deref(), Some("claude-code"));
        assert_eq!(transcript.session_id.as_deref(), Some("s-42"));
        assert_eq!(transcript.tags, ["backend"]);
    }
}
