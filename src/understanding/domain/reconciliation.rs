//! Deciding what a candidate does to the memories already stored.
//!
//! The decision vocabulary is Mem0's — ADD, UPDATE, DELETE, NOOP — and it
//! is the difference between a memory store and a log. Without it, "we
//! moved to Hetzner" is stored *alongside* "we deploy on Fly.io", both
//! come back on the next recall, and the agent has to guess which is
//! current. With it, the old memory is superseded: retained for audit,
//! absent from recall.
//!
//! Everything here is pure — prompt assembly, schema, and parsing the
//! answer. Applying the decisions is the use case's job.

use super::candidate::Candidate;
use super::chat_model::StructuredRequest;
use crate::memories::domain::memory::Memory;
use crate::shared::ids::MemoryId;
use serde_json::{Value, json};
use std::str::FromStr;

const RECONCILIATION_PROMPT: &str = include_str!("../prompts/reconciliation.md");

pub const SCHEMA_NAME: &str = "reconciliation_decisions";

/// What to do about one existing memory, or about the candidate itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Store the candidate as a new memory.
    Add { reason: String },
    /// Store the candidate and mark `superseded` as replaced by it.
    Update {
        superseded: MemoryId,
        reason: String,
    },
    /// Remove an existing memory the user retracted. Does not store the
    /// candidate — a retraction's content ("I no longer use Docker") is
    /// not itself worth remembering.
    Delete { target: MemoryId, reason: String },
    /// The store already knows this.
    Noop { reason: String },
}

impl Decision {
    pub fn reason(&self) -> &str {
        match self {
            Decision::Add { reason }
            | Decision::Update { reason, .. }
            | Decision::Delete { reason, .. }
            | Decision::Noop { reason } => reason,
        }
    }

    /// Whether this decision means the candidate becomes a memory.
    pub fn stores_the_candidate(&self) -> bool {
        matches!(self, Decision::Add { .. } | Decision::Update { .. })
    }
}

/// Builds the reconciliation request for one candidate and its neighbours.
pub fn reconciliation_request(candidate: &Candidate, neighbours: &[Memory]) -> StructuredRequest {
    StructuredRequest::new(
        RECONCILIATION_PROMPT,
        user_message(candidate, neighbours),
        SCHEMA_NAME,
        schema(),
    )
}

pub fn user_message(candidate: &Candidate, neighbours: &[Memory]) -> String {
    let mut message = String::from("Candidate memory:\n\n");
    message.push_str(&format!(
        "[{}] {}\n",
        candidate.category.as_str(),
        candidate.content
    ));

    message.push_str("\nExisting memories most similar to it:\n\n");
    if neighbours.is_empty() {
        message.push_str("(none)\n");
    } else {
        for memory in neighbours {
            // The id comes first and is spelled out in full: the model
            // has to echo it back exactly for an UPDATE or DELETE to
            // resolve, and a truncated id is an unusable decision.
            message.push_str(&format!(
                "- id: {}\n  [{}] {}\n  saved {}\n",
                memory.id(),
                memory.category().as_str(),
                memory.content(),
                memory.created_at().format("%Y-%m-%d")
            ));
        }
    }

    message.push_str("\nDecide what should happen.");
    message
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "decisions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["ADD", "UPDATE", "DELETE", "NOOP"],
                        },
                        "memory_id": {
                            "type": "string",
                            "description":
                                "Required for UPDATE and DELETE: the id of the existing \
                                 memory being replaced or removed. Must be one of the ids \
                                 shown.",
                        },
                        "reason": {
                            "type": "string",
                            "description":
                                "A short explanation, written for the user who will read \
                                 it in their audit trail.",
                        },
                    },
                    "required": ["action", "reason"],
                },
            }
        },
        "required": ["decisions"],
    })
}

/// Why a decision from the model could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionError {
    UnknownAction(String),
    MissingMemoryId(String),
    UnparseableMemoryId(String),
    /// The model named a memory it was not shown. Refused rather than
    /// attempted: the neighbour list is the only set it has grounds to
    /// judge, and an id from anywhere else is a hallucination that would
    /// delete something at random.
    NotANeighbour(MemoryId),
}

impl std::fmt::Display for DecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecisionError::UnknownAction(action) => write!(f, "unknown action {action:?}"),
            DecisionError::MissingMemoryId(action) => {
                write!(f, "{action} needs a memory_id and none was given")
            }
            DecisionError::UnparseableMemoryId(raw) => {
                write!(f, "memory_id {raw:?} is not a valid id")
            }
            DecisionError::NotANeighbour(id) => write!(
                f,
                "the model named memory {id}, which was not among the ones it was shown"
            ),
        }
    }
}

/// Reads one decision, checking any id against what the model was shown.
pub fn parse_decision(value: &Value, shown: &[MemoryId]) -> Result<Decision, DecisionError> {
    let action = value
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();

    let reason = value
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    let target = |action: &str| -> Result<MemoryId, DecisionError> {
        let raw = value
            .get("memory_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
            .ok_or_else(|| DecisionError::MissingMemoryId(action.to_string()))?;

        let id = MemoryId::from_str(raw)
            .map_err(|_| DecisionError::UnparseableMemoryId(raw.to_string()))?;

        if !shown.contains(&id) {
            return Err(DecisionError::NotANeighbour(id));
        }
        Ok(id)
    };

    match action.as_str() {
        "ADD" => Ok(Decision::Add { reason }),
        "NOOP" => Ok(Decision::Noop { reason }),
        "UPDATE" => Ok(Decision::Update {
            superseded: target("UPDATE")?,
            reason,
        }),
        "DELETE" => Ok(Decision::Delete {
            target: target("DELETE")?,
            reason,
        }),
        other => Err(DecisionError::UnknownAction(other.to_string())),
    }
}

/// Reads the `decisions` array, tolerating a bare array.
pub fn decisions_array(answer: &Value) -> Option<&Vec<Value>> {
    answer
        .get("decisions")
        .and_then(Value::as_array)
        .or_else(|| answer.as_array())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
    use crate::shared::ids::UserId;
    use chrono::{DateTime, Utc};

    fn candidate(content: &str) -> Candidate {
        Candidate {
            content: content.to_string(),
            category: Category::FactProject,
            tags: vec![],
            entities: vec![],
            confidence: 0.9,
        }
    }

    fn memory(content: &str) -> Memory {
        Memory::create(
            UserId::new(),
            NewMemory {
                content: content.to_string(),
                category: Category::FactProject,
                tags: vec![],
                entities: vec![],
                confidence: 1.0,
                source: MemorySource::default(),
                expires_at: None,
            },
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn every_action_parses() {
        let id = MemoryId::new();
        let shown = vec![id];

        assert_eq!(
            parse_decision(&json!({"action": "ADD", "reason": "new"}), &shown).unwrap(),
            Decision::Add {
                reason: "new".to_string()
            }
        );
        assert_eq!(
            parse_decision(&json!({"action": "NOOP", "reason": "known"}), &shown).unwrap(),
            Decision::Noop {
                reason: "known".to_string()
            }
        );
        assert_eq!(
            parse_decision(
                &json!({"action": "UPDATE", "memory_id": id.to_string(), "reason": "moved"}),
                &shown
            )
            .unwrap(),
            Decision::Update {
                superseded: id,
                reason: "moved".to_string()
            }
        );
        assert_eq!(
            parse_decision(
                &json!({"action": "DELETE", "memory_id": id.to_string(), "reason": "retracted"}),
                &shown
            )
            .unwrap(),
            Decision::Delete {
                target: id,
                reason: "retracted".to_string()
            }
        );
    }

    #[test]
    fn actions_are_case_insensitive() {
        assert!(parse_decision(&json!({"action": "  noop  "}), &[]).is_ok());
    }

    #[test]
    fn an_id_the_model_was_not_shown_is_refused() {
        // The safety property that matters most here. The neighbour list
        // is the only set the model has grounds to judge; an id from
        // anywhere else deletes something at random.
        let stranger = MemoryId::new();
        let error = parse_decision(
            &json!({"action": "DELETE", "memory_id": stranger.to_string(), "reason": "x"}),
            &[MemoryId::new()],
        )
        .unwrap_err();

        assert_eq!(error, DecisionError::NotANeighbour(stranger));
    }

    #[test]
    fn update_and_delete_require_an_id() {
        for action in ["UPDATE", "DELETE"] {
            let error = parse_decision(&json!({"action": action, "reason": "x"}), &[]).unwrap_err();
            assert_eq!(error, DecisionError::MissingMemoryId(action.to_string()));
        }
    }

    #[test]
    fn a_mangled_id_is_reported_rather_than_ignored() {
        let error = parse_decision(
            &json!({"action": "UPDATE", "memory_id": "mem_123", "reason": "x"}),
            &[],
        )
        .unwrap_err();

        assert!(matches!(error, DecisionError::UnparseableMemoryId(_)));
    }

    #[test]
    fn an_invented_action_is_refused_rather_than_guessed_at() {
        // Guessing — say, treating "MERGE" as UPDATE — would act on the
        // store in a way the model did not ask for.
        let error = parse_decision(&json!({"action": "MERGE", "reason": "x"}), &[]).unwrap_err();
        assert_eq!(error, DecisionError::UnknownAction("MERGE".to_string()));
    }

    #[test]
    fn only_add_and_update_store_the_candidate() {
        // DELETE is a retraction: "I no longer use Docker" removes a
        // memory, and storing the retraction itself would just be noise.
        assert!(
            Decision::Add {
                reason: String::new()
            }
            .stores_the_candidate()
        );
        assert!(
            Decision::Update {
                superseded: MemoryId::new(),
                reason: String::new()
            }
            .stores_the_candidate()
        );
        assert!(
            !Decision::Delete {
                target: MemoryId::new(),
                reason: String::new()
            }
            .stores_the_candidate()
        );
        assert!(
            !Decision::Noop {
                reason: String::new()
            }
            .stores_the_candidate()
        );
    }

    #[test]
    fn the_prompt_shows_each_neighbour_with_its_full_id() {
        // A truncated or reformatted id cannot be echoed back, so every
        // UPDATE and DELETE would be unusable.
        let neighbours = vec![memory("Backend deploys on Fly.io")];
        let message = user_message(&candidate("Backend runs on Hetzner"), &neighbours);

        assert!(
            message.contains(&neighbours[0].id().to_string()),
            "{message}"
        );
        assert!(message.contains("Backend deploys on Fly.io"), "{message}");
        assert!(message.contains("Backend runs on Hetzner"), "{message}");
    }

    #[test]
    fn no_neighbours_is_stated_rather_than_left_blank() {
        // An empty section reads to a model as a truncated prompt.
        let message = user_message(&candidate("something new"), &[]);
        assert!(message.contains("(none)"), "{message}");
    }

    #[test]
    fn a_bare_array_of_decisions_is_accepted() {
        let answer = json!([{"action": "NOOP", "reason": "x"}]);
        assert_eq!(decisions_array(&answer).unwrap().len(), 1);
    }
}
