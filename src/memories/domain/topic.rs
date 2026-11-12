//! What a memory is about (ai-specs/memory-structure-plan.md §3.3).
//!
//! The other half of the old category label. Built-ins are fixed here; users
//! add their own (stored per user in `topics`). A topic id is a short
//! lowercase slug so it can be typed and filtered on.

use crate::shared::error::{RaError, Result};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Topic {
    /// Writing software: languages, tools, style, testing, architecture.
    Coding,
    /// Machines, servers, services, networks, accounts and configuration
    /// outside any codebase.
    Tech,
    /// Jobs, colleagues' roles, ways of working.
    Work,
    /// Relatives and the household.
    Family,
    /// Everyone else: friends, colleagues, contacts.
    People,
    /// Body, sleep, medical, exercise as health.
    Health,
    /// Routines, hobbies, leisure, habits.
    Lifestyle,
    /// The house, appliances, chores, maintenance.
    Home,
    /// Money, bills, subscriptions, taxes.
    Finance,
    /// Trips, transport, places.
    Travel,
    /// Diet, cooking, restaurants.
    Food,
    /// A topic the user created.
    Custom(String),
}

pub const MAX_TOPIC_ID_LEN: usize = 32;

/// Every built-in topic, in the order the plan lists them.
pub const BUILT_IN_TOPICS: &[Topic] = &[
    Topic::Coding,
    Topic::Tech,
    Topic::Work,
    Topic::Family,
    Topic::People,
    Topic::Health,
    Topic::Lifestyle,
    Topic::Home,
    Topic::Finance,
    Topic::Travel,
    Topic::Food,
];

impl Topic {
    pub fn as_str(&self) -> &str {
        match self {
            Topic::Coding => "coding",
            Topic::Tech => "tech",
            Topic::Work => "work",
            Topic::Family => "family",
            Topic::People => "people",
            Topic::Health => "health",
            Topic::Lifestyle => "lifestyle",
            Topic::Home => "home",
            Topic::Finance => "finance",
            Topic::Travel => "travel",
            Topic::Food => "food",
            Topic::Custom(id) => id,
        }
    }

    pub fn is_built_in(&self) -> bool {
        !matches!(self, Topic::Custom(_))
    }

    /// The built-in with this id, if there is one.
    pub fn built_in(raw: &str) -> Option<Topic> {
        BUILT_IN_TOPICS
            .iter()
            .find(|topic| topic.as_str() == raw)
            .cloned()
    }

    /// A user topic id: validated, so a malformed one never reaches storage.
    pub fn custom(raw: &str) -> Result<Topic> {
        let id = raw.trim().to_ascii_lowercase();
        if let Some(topic) = Topic::built_in(&id) {
            return Ok(topic);
        }
        if id.is_empty() || id.chars().count() > MAX_TOPIC_ID_LEN {
            return Err(RaError::Validation(format!(
                "a topic id is 1 to {MAX_TOPIC_ID_LEN} characters"
            )));
        }
        let valid = id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !valid || !id.starts_with(|c: char| c.is_ascii_lowercase()) {
            return Err(RaError::Validation(format!(
                "topic id {raw:?} must start with a letter and use only a-z, 0-9, - and _"
            )));
        }
        Ok(Topic::Custom(id))
    }

    /// Rebuilds a stored topic. Never fails: a stored row is historical fact.
    pub fn from_stored(raw: &str) -> Topic {
        Topic::built_in(raw).unwrap_or_else(|| Topic::Custom(raw.to_string()))
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_round_trip() {
        for topic in BUILT_IN_TOPICS {
            assert_eq!(Topic::from_stored(topic.as_str()), *topic);
            assert!(topic.is_built_in());
        }
    }

    #[test]
    fn a_custom_id_that_names_a_built_in_is_the_built_in() {
        assert_eq!(Topic::custom(" Health ").unwrap(), Topic::Health);
    }

    #[test]
    fn custom_ids_are_slugs() {
        assert_eq!(
            Topic::custom("Homelab").unwrap(),
            Topic::Custom("homelab".to_string())
        );
        assert_eq!(
            Topic::custom("side-projects").unwrap(),
            Topic::Custom("side-projects".to_string())
        );
        for bad in [
            "",
            "  ",
            "9lives",
            "home lab",
            "café",
            "a.b",
            &"x".repeat(33),
        ] {
            assert!(Topic::custom(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn an_unknown_stored_topic_is_kept_as_custom() {
        assert_eq!(
            Topic::from_stored("garden"),
            Topic::Custom("garden".to_string())
        );
    }
}
