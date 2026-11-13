//! Asking a model whether a new memory contradicts its stored neighbours
//! (ai-specs/memory-structure-plan.md §5, R8) and whether its text fits its
//! scope (the model layer of R7, D16) — in one request, so judging a save
//! costs one call at most.
//!
//! Pure: the prompt, the schema and reading the answer. Neighbours are shown
//! by number rather than by id. A number is easy to echo back, and one the
//! model was not shown resolves to nothing rather than to a memory chosen at
//! random.

use super::chat_model::StructuredRequest;
use crate::memories::domain::conflict_judge::{
    ConflictQuestion, Judgement, NeighbourVerdict, Verdict,
};
use serde_json::{Value, json};

const CONFLICT_PROMPT: &str = include_str!("../prompts/conflict.md");

pub const SCHEMA_NAME: &str = "conflict_judgement";

pub fn conflict_request(question: &ConflictQuestion) -> StructuredRequest {
    StructuredRequest::new(
        CONFLICT_PROMPT,
        user_message(question),
        SCHEMA_NAME,
        schema(),
    )
}

pub fn user_message(question: &ConflictQuestion) -> String {
    let mut message = format!(
        "New memory ({} · {} · applies to {}):\n\n{}\n",
        question.kind,
        question.topic,
        question.applies_to.label(),
        question.content
    );

    message.push_str("\nStored memories most similar to it:\n\n");
    if question.neighbours.is_empty() {
        message.push_str("(none)\n");
    }
    for (index, neighbour) in question.neighbours.iter().enumerate() {
        let structure = neighbour.memory.structure();
        let own = if neighbour.personal {
            ", the user's own"
        } else {
            ""
        };
        message.push_str(&format!(
            "{}. ({} · applies to {}{own}) {}\n",
            index + 1,
            structure.kind(),
            structure.applies_to().label(),
            neighbour.memory.content()
        ));
    }

    if question.check_scope {
        message.push_str(&format!(
            "\nAlso give a scope verdict: does the new memory's text fit \"applies to {}\"?\n",
            question.applies_to.label()
        ));
    } else {
        message.push_str("\nNo scope verdict is needed.\n");
    }
    message
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "neighbours": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "number": {
                            "type": "integer",
                            "description": "The stored memory's number, as shown.",
                        },
                        "verdict": {
                            "type": "string",
                            "enum": ["contradicts", "same", "unrelated"],
                        },
                        "reason": {"type": "string"},
                    },
                    "required": ["number", "verdict", "reason"],
                },
            },
            "scope": {
                "type": "object",
                "description": "Only when a scope verdict was asked for.",
                "properties": {
                    "fits": {"type": "boolean"},
                    "reason": {"type": "string"},
                },
                "required": ["fits"],
            },
        },
        "required": ["neighbours"],
    })
}

/// Reads the answer. What cannot be read is dropped and treated as
/// unrelated: a garbled verdict must never refuse a save.
pub fn parse_judgement(answer: &Value, question: &ConflictQuestion) -> Judgement {
    let mut verdicts = Vec::new();
    let listed = answer
        .get("neighbours")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in &listed {
        let Some(number) = item.get("number").and_then(Value::as_u64) else {
            continue;
        };
        let Some(neighbour) = (number as usize)
            .checked_sub(1)
            .and_then(|index| question.neighbours.get(index))
        else {
            tracing::warn!(number, "the conflict judge named a memory it was not shown");
            continue;
        };
        let verdict = match item
            .get("verdict")
            .and_then(Value::as_str)
            .map(|raw| raw.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("contradicts") => Verdict::Contradicts,
            Some("same") => Verdict::Same,
            Some("unrelated") => Verdict::Unrelated,
            other => {
                tracing::warn!(?other, "the conflict judge gave an unknown verdict");
                continue;
            }
        };
        let reason = item
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        verdicts.push(NeighbourVerdict {
            memory_id: neighbour.memory.id(),
            verdict,
            reason,
        });
    }

    let scope_mismatch = if question.check_scope {
        answer.get("scope").and_then(|scope| {
            (scope.get("fits").and_then(Value::as_bool) == Some(false)).then(|| {
                scope
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or("the model judged it to be about one project")
                    .to_string()
            })
        })
    } else {
        None
    };

    Judgement {
        verdicts,
        scope_mismatch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::domain::applies_to::{AppliesTo, ProjectRef};
    use crate::memories::domain::category::Category;
    use crate::memories::domain::conflict_judge::Neighbour;
    use crate::memories::domain::kind::Kind;
    use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
    use crate::memories::domain::topic::Topic;
    use crate::shared::ids::{ProjectId, UserId};

    /// Built here rather than borrowed from the memories test doubles: a
    /// domain module may not reach into an application layer.
    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
    }

    fn memory(content: &str) -> Memory {
        Memory::create(
            UserId::new(),
            NewMemory {
                content: content.to_string(),
                category: Category::PreferenceCoding,
                subcategory: None,
                tags: vec![],
                entities: vec![],
                confidence: 1.0,
                source: MemorySource::default(),
                expires_at: None,
            },
            now(),
        )
        .unwrap()
    }

    fn question(neighbours: &[&str], check_scope: bool) -> ConflictQuestion {
        ConflictQuestion {
            content: "The user avoids mocks in unit tests".to_string(),
            kind: Kind::Preference,
            topic: Topic::Coding,
            applies_to: AppliesTo::Me,
            neighbours: neighbours
                .iter()
                .map(|content| Neighbour {
                    memory: memory(content),
                    relevance: 0.8,
                    personal: false,
                })
                .collect(),
            check_scope,
        }
    }

    #[test]
    fn the_message_numbers_the_neighbours_and_names_the_scope() {
        let question = question(&["The user prefers mocks in unit tests"], true);
        let message = user_message(&question);

        assert!(message.contains("applies to me"), "{message}");
        assert!(
            message.contains("1. (preference · applies to me) The user prefers mocks"),
            "{message}"
        );
        assert!(message.contains("scope verdict"), "{message}");
    }

    #[test]
    fn a_personal_neighbour_of_a_project_memory_is_marked_as_the_users_own() {
        let mut question = question(&["The user prefers mocks in unit tests"], false);
        question.applies_to = AppliesTo::Project(ProjectRef {
            id: ProjectId::new(),
            name: "quill".to_string(),
        });
        question.neighbours[0].personal = true;

        let message = user_message(&question);
        assert!(message.contains("the user's own"), "{message}");
        assert!(message.contains("No scope verdict"), "{message}");
    }

    #[test]
    fn verdicts_map_back_to_the_memories_shown() {
        let question = question(&["prefers mocks", "prefers pnpm"], false);
        let judgement = parse_judgement(
            &json!({"neighbours": [
                {"number": 1, "verdict": "Contradicts", "reason": "opposite"},
                {"number": 2, "verdict": "unrelated", "reason": "another tool"},
            ]}),
            &question,
        );

        assert_eq!(judgement.verdicts.len(), 2);
        assert_eq!(
            judgement.verdicts[0].memory_id,
            question.neighbours[0].memory.id()
        );
        assert_eq!(judgement.verdicts[0].verdict, Verdict::Contradicts);
        assert_eq!(judgement.verdicts[1].verdict, Verdict::Unrelated);
    }

    #[test]
    fn a_number_never_shown_or_a_garbled_verdict_is_dropped() {
        let question = question(&["prefers mocks"], false);
        let judgement = parse_judgement(
            &json!({"neighbours": [
                {"number": 7, "verdict": "contradicts", "reason": "x"},
                {"number": 0, "verdict": "contradicts", "reason": "x"},
                {"number": 1, "verdict": "maybe", "reason": "x"},
            ]}),
            &question,
        );
        assert!(judgement.verdicts.is_empty());
    }

    #[test]
    fn a_scope_verdict_counts_only_when_it_was_asked_for() {
        let answer = json!({"neighbours": [], "scope": {"fits": false, "reason": "\"here\""}});

        let asked = parse_judgement(&answer, &question(&[], true));
        assert_eq!(asked.scope_mismatch.as_deref(), Some("\"here\""));

        let not_asked = parse_judgement(&answer, &question(&[], false));
        assert_eq!(not_asked.scope_mismatch, None);

        let fits = parse_judgement(
            &json!({"neighbours": [], "scope": {"fits": true}}),
            &question(&[], true),
        );
        assert_eq!(fits.scope_mismatch, None);
    }
}
