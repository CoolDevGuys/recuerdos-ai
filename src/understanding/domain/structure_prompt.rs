//! Asking a model what stored memories really are (ai-specs/memory-structure-plan.md
//! §8, the Needs review queue's **Suggest**): kind, topic and who each is
//! true for, for the user to accept or change. Nothing is written from here.
//!
//! Pure: the prompt, the schema and reading the answer. Memories are shown by
//! number, and an answer is kept only when it fits the rules a save is held
//! to — a kind its target allows, a topic and a project the user has — so a
//! suggestion the user accepts is one the store will take.

use super::chat_model::StructuredRequest;
use crate::memories::domain::applies_to::Target;
use crate::memories::domain::kind::{ALL_KINDS, Kind};
use crate::memories::domain::memory::Memory;
use serde_json::{Value, json};

const STRUCTURE_PROMPT: &str = include_str!("../prompts/structure.md");

pub const SCHEMA_NAME: &str = "structure_suggestions";

/// What the model is told the user has.
pub struct SuggestionContext<'a> {
    pub topics: &'a [String],
    pub projects: &'a [String],
}

/// One proposal, already checked against the rules.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// Index into the memories asked about.
    pub index: usize,
    pub kind: Kind,
    pub topic: String,
    pub target: Target,
    /// With `target: project`, one of the user's projects.
    pub project: Option<String>,
    pub reason: String,
}

pub fn structure_request(
    memories: &[Memory],
    context: &SuggestionContext<'_>,
) -> StructuredRequest {
    StructuredRequest::new(
        STRUCTURE_PROMPT,
        user_message(memories, context),
        SCHEMA_NAME,
        schema(),
    )
}

pub fn user_message(memories: &[Memory], context: &SuggestionContext<'_>) -> String {
    let mut message = format!(
        "Topics: {}\nProjects: {}\n\n<<<BEGIN MEMORIES>>>\n",
        context.topics.join(", "),
        if context.projects.is_empty() {
            "(none)".to_string()
        } else {
            context.projects.join(", ")
        }
    );
    for (index, memory) in memories.iter().enumerate() {
        let tags = if memory.tags().is_empty() {
            String::new()
        } else {
            format!(" [tags: {}]", memory.tags().join(", "))
        };
        message.push_str(&format!(
            "{}. (filed as {}){tags} {}\n",
            index + 1,
            memory.category().as_str(),
            memory.content().replace('\n', " ")
        ));
    }
    message.push_str("<<<END MEMORIES>>>\n");
    message
}

pub fn schema() -> Value {
    let kinds: Vec<&str> = ALL_KINDS.iter().map(Kind::as_str).collect();
    json!({
        "type": "object",
        "properties": {
            "suggestions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "number": {"type": "integer"},
                        "kind": {"type": "string", "enum": kinds},
                        "topic": {"type": "string"},
                        "target": {"type": "string", "enum": ["me", "project", "general"]},
                        "project": {"type": ["string", "null"]},
                        "reason": {"type": "string"},
                    },
                    "required": ["number", "kind", "topic", "target", "reason"],
                },
            },
        },
        "required": ["suggestions"],
    })
}

/// The usable suggestions, one per memory at most. One that breaks a rule —
/// an unknown number, kind, topic or project, a kind its target does not
/// allow — is dropped: no suggestion is better than one the save refuses.
pub fn parse_suggestions(
    answer: &Value,
    count: usize,
    context: &SuggestionContext<'_>,
) -> Vec<Suggestion> {
    let Some(items) = answer["suggestions"].as_array() else {
        return Vec::new();
    };
    let mut seen = vec![false; count];
    let mut suggestions = Vec::new();
    for item in items {
        let Some(index) = item["number"]
            .as_u64()
            .and_then(|n| (n as usize).checked_sub(1))
            .filter(|index| *index < count && !seen[*index])
        else {
            continue;
        };
        let Some(kind) = item["kind"]
            .as_str()
            .and_then(|raw| raw.parse::<Kind>().ok())
        else {
            continue;
        };
        let Some(topic) = item["topic"]
            .as_str()
            .map(|raw| raw.trim().to_ascii_lowercase())
            .filter(|topic| context.topics.contains(topic))
        else {
            continue;
        };
        let Some(mut target) = item["target"]
            .as_str()
            .and_then(|raw| raw.parse::<Target>().ok())
        else {
            continue;
        };
        let mut project = None;
        if target == Target::Project {
            match item["project"]
                .as_str()
                .map(|raw| raw.trim().to_ascii_lowercase())
                .filter(|name| context.projects.contains(name))
            {
                Some(name) => project = Some(name),
                // A project the user has not got: general, for them to move.
                None => target = Target::General,
            }
        }
        if !kind.allows(target) {
            continue;
        }
        seen[index] = true;
        suggestions.push(Suggestion {
            index,
            kind,
            topic,
            target,
            project,
            reason: item["reason"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_string(),
        });
    }
    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory::{MemorySource, NewMemory};
    use crate::shared::ids::UserId;

    fn memory(content: &str) -> Memory {
        Memory::create(
            UserId::new(),
            NewMemory {
                content: content.to_string(),
                category: Category::FactProject,
                subcategory: None,
                tags: vec![],
                entities: vec![],
                confidence: 1.0,
                source: MemorySource::default(),
                expires_at: None,
            },
            chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        )
        .unwrap()
    }

    fn context() -> (Vec<String>, Vec<String>) {
        (
            vec!["coding".to_string(), "tech".to_string()],
            vec!["quill".to_string()],
        )
    }

    #[test]
    fn memories_are_numbered_fenced_and_shown_with_what_the_user_has() {
        let (topics, projects) = context();
        let message = user_message(
            &[memory("The API is written in Rust")],
            &SuggestionContext {
                topics: &topics,
                projects: &projects,
            },
        );
        assert!(message.contains("Topics: coding, tech"), "{message}");
        assert!(message.contains("Projects: quill"), "{message}");
        assert!(message.contains("1. (filed as fact.project) The API is written in Rust"));
        assert!(message.contains("<<<BEGIN MEMORIES>>>"));
    }

    #[test]
    fn only_suggestions_the_rules_allow_are_kept() {
        let (topics, projects) = context();
        let context = SuggestionContext {
            topics: &topics,
            projects: &projects,
        };
        let answer = json!({"suggestions": [
            {"number": 1, "kind": "fact", "topic": "coding", "target": "project",
             "project": "quill", "reason": "about the quill codebase"},
            // A preference cannot be a project's.
            {"number": 2, "kind": "preference", "topic": "coding", "target": "project",
             "project": "quill", "reason": "x"},
            // An unknown project becomes general.
            {"number": 3, "kind": "reference", "topic": "tech", "target": "project",
             "project": "fern", "reason": "a NAS path"},
            // An unknown topic, and a number never shown.
            {"number": 4, "kind": "fact", "topic": "gardening", "target": "general", "reason": "x"},
            {"number": 9, "kind": "fact", "topic": "tech", "target": "general", "reason": "x"},
            // A second answer for the same memory is ignored.
            {"number": 1, "kind": "rule", "topic": "coding", "target": "me", "reason": "x"},
        ]});

        let suggestions = parse_suggestions(&answer, 4, &context);

        assert_eq!(suggestions.len(), 2, "{suggestions:?}");
        assert_eq!(suggestions[0].index, 0);
        assert_eq!(suggestions[0].project.as_deref(), Some("quill"));
        assert_eq!(suggestions[1].index, 2);
        assert_eq!(suggestions[1].target, Target::General);
        assert_eq!(suggestions[1].project, None);
    }

    #[test]
    fn an_answer_of_the_wrong_shape_suggests_nothing() {
        let (topics, projects) = context();
        let context = SuggestionContext {
            topics: &topics,
            projects: &projects,
        };
        assert!(parse_suggestions(&json!({"x": 1}), 2, &context).is_empty());
    }
}
