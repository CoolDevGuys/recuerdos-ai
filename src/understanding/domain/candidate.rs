//! A proposed memory, before anything has been stored.
//!
//! Extraction produces candidates; reconciliation decides which become
//! memories. Keeping them a separate type from [`Memory`] is what makes
//! that possible — a candidate has no id, belongs to no user, and can be
//! discarded without an audit entry, because nothing ever committed to it.
//!
//! [`Memory`]: crate::memories::domain::memory::Memory

use crate::memories::domain::category::Category;
use crate::memories::domain::memory::{Entity, MAX_CONTENT_LEN};
use serde::Deserialize;

/// Applied when the model omits a confidence.
///
/// Not 1.0: a memory the model chose to report but did not rate is an
/// inference, and rating inferences as certainly-true would let them
/// outrank things the user said outright.
pub const DEFAULT_CONFIDENCE: f32 = 0.8;

/// A candidate as it arrives from the model, before validation.
///
/// Every field but `content` is optional, and `deny_unknown_fields` is
/// deliberately absent: a model that adds a `reasoning` field it was not
/// asked for should have its useful output kept, not rejected wholesale.
#[derive(Debug, Clone, Deserialize)]
pub struct RawCandidate {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub subcategory: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub entities: Vec<RawEntity>,
    #[serde(default)]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawEntity {
    #[serde(default)]
    pub name: String,
    /// The model is asked for `kind`; `type` is what it often writes
    /// instead, being the more natural English word. Accepting both costs
    /// one line and saves a repair round trip.
    #[serde(default, alias = "type")]
    pub kind: String,
}

/// A candidate that has been validated and normalised.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub content: String,
    pub category: Category,
    pub subcategory: Option<String>,
    pub tags: Vec<String>,
    pub entities: Vec<Entity>,
    pub confidence: f32,
}

/// Why a candidate was discarded. Reported rather than silently dropped:
/// a model whose output is being thrown away should be visible in logs,
/// not inferred from memories that never appear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    Empty,
    TooLong { characters: usize },
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejection::Empty => write!(f, "the candidate had no content"),
            Rejection::TooLong { characters } => write!(
                f,
                "the candidate was {characters} characters, over the {MAX_CONTENT_LEN} limit — \
                 the model returned a passage rather than an atomic memory"
            ),
        }
    }
}

impl RawCandidate {
    /// Validates and normalises, or explains why this one is unusable.
    ///
    /// Over-long candidates are rejected rather than truncated. Cutting a
    /// memory mid-sentence produces something that reads as fact but is
    /// not — far worse than losing it, because nothing downstream can
    /// tell it happened.
    pub fn validate(self, category: Category) -> Result<Candidate, Rejection> {
        let content = self.content.trim();
        if content.is_empty() {
            return Err(Rejection::Empty);
        }

        let characters = content.chars().count();
        if characters > MAX_CONTENT_LEN {
            return Err(Rejection::TooLong { characters });
        }

        Ok(Candidate {
            content: content.to_string(),
            category,
            subcategory: normalise_subcategory(self.subcategory),
            tags: normalise_tags(self.tags),
            entities: normalise_entities(self.entities),
            confidence: self
                .confidence
                .unwrap_or(DEFAULT_CONFIDENCE)
                .clamp(0.0, 1.0),
        })
    }
}

/// Lowercased, trimmed, empty → None.
fn normalise_subcategory(subcategory: Option<String>) -> Option<String> {
    match subcategory {
        Some(s) => {
            let s = s.trim().to_ascii_lowercase();
            if s.is_empty() { None } else { Some(s) }
        }
        None => None,
    }
}

/// Lowercased, trimmed, de-duplicated, empties dropped.
///
/// The same normalisation `Memory` applies, done here so two candidates
/// differing only in tag casing are visibly identical to reconciliation.
fn normalise_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = Vec::new();
    for tag in tags {
        let tag = tag.trim().to_ascii_lowercase();
        if !tag.is_empty() && !seen.contains(&tag) {
            seen.push(tag);
        }
    }
    seen
}

/// An entity needs a name; a missing `kind` becomes `thing` rather than
/// dropping the entity, because the name is the part with information in
/// it and the graph layer can refine kinds later.
fn normalise_entities(entities: Vec<RawEntity>) -> Vec<Entity> {
    entities
        .into_iter()
        .filter_map(|entity| {
            let name = entity.name.trim();
            if name.is_empty() {
                return None;
            }
            let kind = entity.kind.trim().to_ascii_lowercase();
            Some(Entity {
                name: name.to_string(),
                kind: if kind.is_empty() {
                    "thing".to_string()
                } else {
                    kind
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(value: serde_json::Value) -> RawCandidate {
        serde_json::from_value(value).expect("valid raw candidate")
    }

    #[test]
    fn a_well_formed_candidate_validates() {
        let candidate = raw(json!({
            "content": "  The backend runs on Hetzner  ",
            "category": "fact.project",
            "tags": ["Infrastructure", "hetzner"],
            "entities": [{"name": "Hetzner", "kind": "Service"}],
            "confidence": 0.9,
        }))
        .validate(Category::FactProject)
        .unwrap();

        assert_eq!(candidate.content, "The backend runs on Hetzner");
        assert_eq!(candidate.subcategory, None);
        assert_eq!(candidate.tags, ["infrastructure", "hetzner"]);
        assert_eq!(
            candidate.entities,
            [Entity {
                name: "Hetzner".to_string(),
                kind: "service".to_string()
            }]
        );
        assert_eq!(candidate.confidence, 0.9);
    }

    #[test]
    fn only_content_is_required() {
        // A model that omits everything optional should still produce a
        // usable memory rather than a repair round trip.
        let candidate = raw(json!({"content": "I prefer pnpm"}))
            .validate(Category::PreferenceCoding)
            .unwrap();

        assert!(candidate.tags.is_empty());
        assert_eq!(candidate.confidence, DEFAULT_CONFIDENCE);
    }

    #[test]
    fn unrequested_fields_do_not_reject_the_candidate() {
        // Models add `reasoning`, `id`, `source`… Throwing away good
        // output over an extra key would be a bad trade.
        let candidate = raw(json!({
            "content": "I prefer pnpm",
            "reasoning": "the user said so",
        }))
        .validate(Category::PreferenceCoding)
        .unwrap();

        assert_eq!(candidate.content, "I prefer pnpm");
    }

    #[test]
    fn an_entity_typed_with_type_instead_of_kind_is_still_read() {
        let candidate = raw(json!({
            "content": "x",
            "entities": [{"name": "Hetzner", "type": "service"}],
        }))
        .validate(Category::FactProject)
        .unwrap();

        assert_eq!(candidate.entities[0].kind, "service");
    }

    #[test]
    fn an_entity_with_no_kind_keeps_its_name() {
        // The name carries the information; a missing kind is a detail
        // the graph layer can refine later.
        let candidate = raw(json!({
            "content": "x",
            "entities": [{"name": "Hetzner"}, {"name": "   "}],
        }))
        .validate(Category::FactProject)
        .unwrap();

        assert_eq!(candidate.entities.len(), 1);
        assert_eq!(candidate.entities[0].kind, "thing");
    }

    #[test]
    fn tags_are_deduplicated_after_lowercasing() {
        // Otherwise two candidates that differ only in casing look
        // different to reconciliation and both get stored.
        let candidate = raw(json!({"content": "x", "tags": ["Go", "go", " GO ", ""]}))
            .validate(Category::PreferenceCoding)
            .unwrap();

        assert_eq!(candidate.tags, ["go"]);
    }

    #[test]
    fn an_empty_candidate_is_rejected_with_a_reason() {
        let rejection = raw(json!({"content": "   "}))
            .validate(Category::FactProject)
            .unwrap_err();
        assert_eq!(rejection, Rejection::Empty);
    }

    #[test]
    fn an_over_long_candidate_is_rejected_rather_than_truncated() {
        // Truncation would produce something that reads as a complete
        // fact but isn't — worse than losing it, because nothing
        // downstream can tell.
        let long = "x".repeat(MAX_CONTENT_LEN + 1);
        let rejection = raw(json!({"content": long}))
            .validate(Category::FactProject)
            .unwrap_err();

        assert!(matches!(rejection, Rejection::TooLong { .. }));
        assert!(rejection.to_string().contains("atomic"), "{rejection}");
    }

    #[test]
    fn confidence_is_clamped_rather_than_trusted() {
        for (given, expected) in [(5.0, 1.0), (-2.0, 0.0)] {
            let candidate = raw(json!({"content": "x", "confidence": given}))
                .validate(Category::FactProject)
                .unwrap();
            assert_eq!(candidate.confidence, expected);
        }
    }

    #[test]
    fn subcategory_is_normalized_on_validate() {
        let candidate = raw(json!({
            "content": "x",
            "subcategory": "  Testing  ",
        }))
        .validate(Category::PreferenceCoding)
        .unwrap();

        assert_eq!(candidate.subcategory, Some("testing".to_string()));
    }

    #[test]
    fn subcategory_empty_becomes_none() {
        let candidate = raw(json!({
            "content": "x",
            "subcategory": "   ",
        }))
        .validate(Category::PreferenceCoding)
        .unwrap();

        assert_eq!(candidate.subcategory, None);
    }
}
