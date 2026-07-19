//! The memory taxonomy.
//!
//! Categories are a *closed* set with an escape hatch, not free-form
//! labels. Free-form-only labelling fragments — `prefs`, `preferences`,
//! `user-prefs` all appear within a week — and a fragmented label space
//! makes filtered retrieval useless, which is the whole point of
//! labelling. Tags stay free-form; the category is the axis you filter on.
//!
//! The default set is project-plan.md §7.1. Deployments may add their own
//! via `[understanding.taxonomy].extra_categories`, which arrive here as
//! [`Category::Custom`].

use crate::shared::error::{RaError, Result};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// Style, tooling and patterns: "prefers pnpm", "no default exports".
    PreferenceCoding,
    /// Life preferences: "vegetarian", "prefers morning meetings".
    PreferencePersonal,
    /// An architecture or product decision *and its rationale*.
    Decision,
    /// Implemented features, stack facts, project constraints.
    FactProject,
    /// People, relationships, roles.
    FactPerson,
    /// What happened: "the pgvector migration failed because…".
    Experience,
    /// A learned procedure (aligns with Hermes Agent's skill files).
    Skill,
    /// Pointers outward: URLs, tickets, dashboards.
    Reference,
    /// A deployment-defined category from config.
    Custom(String),
}

/// Every built-in category, in taxonomy order.
pub const DEFAULT_CATEGORIES: &[Category] = &[
    Category::PreferenceCoding,
    Category::PreferencePersonal,
    Category::Decision,
    Category::FactProject,
    Category::FactPerson,
    Category::Experience,
    Category::Skill,
    Category::Reference,
];

impl Category {
    pub fn as_str(&self) -> &str {
        match self {
            Category::PreferenceCoding => "preference.coding",
            Category::PreferencePersonal => "preference.personal",
            Category::Decision => "decision",
            Category::FactProject => "fact.project",
            Category::FactPerson => "fact.person",
            Category::Experience => "experience",
            Category::Skill => "skill",
            Category::Reference => "reference",
            Category::Custom(name) => name,
        }
    }

    /// Parses a category name, accepting any built-in.
    ///
    /// Unknown names are rejected rather than silently becoming
    /// `Custom`: a typo like `preference.codeing` must not quietly create
    /// a category that nothing will ever match. Deployments widen the set
    /// deliberately, through [`Category::parse_with_extras`].
    pub fn parse(raw: &str) -> Result<Self> {
        Self::parse_with_extras(raw, &[])
    }

    /// Parses a category name, additionally accepting `extras`.
    pub fn parse_with_extras(raw: &str, extras: &[String]) -> Result<Self> {
        let name = raw.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(RaError::Validation("category is empty".to_string()));
        }

        if let Some(known) = DEFAULT_CATEGORIES
            .iter()
            .find(|category| category.as_str() == name)
        {
            return Ok(known.clone());
        }

        if extras.iter().any(|extra| extra.eq_ignore_ascii_case(&name)) {
            return Ok(Category::Custom(name));
        }

        Err(RaError::Validation(format!(
            "unknown category {name:?} (expected one of {}{})",
            DEFAULT_CATEGORIES
                .iter()
                .map(Category::as_str)
                .collect::<Vec<_>>()
                .join(", "),
            if extras.is_empty() {
                String::new()
            } else {
                format!(", or a configured extra: {}", extras.join(", "))
            }
        )))
    }

    /// Rebuilds a category read from storage.
    ///
    /// Never fails: a row written when the taxonomy was wider is
    /// historical fact, and refusing to load it would hide a user's own
    /// memory from them because config changed.
    pub fn from_stored(raw: &str) -> Self {
        Self::parse(raw).unwrap_or_else(|_| Category::Custom(raw.to_string()))
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Category {
    type Err = RaError;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_built_in_category() {
        for category in DEFAULT_CATEGORIES {
            let parsed = Category::parse(category.as_str())
                .unwrap_or_else(|e| panic!("{} should parse: {e}", category.as_str()));
            assert_eq!(&parsed, category);
        }
    }

    #[test]
    fn parsing_is_case_and_whitespace_insensitive() {
        assert_eq!(
            Category::parse("  Preference.Coding  ").unwrap(),
            Category::PreferenceCoding
        );
    }

    #[test]
    fn rejects_an_unknown_category_rather_than_inventing_one() {
        // The typo case: a silently-created `preference.codeing` would
        // never match a filter and would look like data loss.
        let err = Category::parse("preference.codeing").unwrap_err();
        assert!(matches!(err, RaError::Validation(_)), "got {err:?}");
        assert!(err.to_string().contains("preference.coding"), "{err}");
    }

    #[test]
    fn rejects_an_empty_category() {
        assert!(Category::parse("  ").is_err());
    }

    #[test]
    fn accepts_a_configured_extra() {
        let extras = vec!["fact.homelab".to_string()];
        assert_eq!(
            Category::parse_with_extras("fact.homelab", &extras).unwrap(),
            Category::Custom("fact.homelab".to_string())
        );
    }

    #[test]
    fn an_extra_does_not_shadow_a_built_in() {
        let extras = vec!["decision".to_string()];
        assert_eq!(
            Category::parse_with_extras("decision", &extras).unwrap(),
            Category::Decision,
            "a built-in must not degrade into a Custom"
        );
    }

    #[test]
    fn stored_categories_always_load_even_if_no_longer_configured() {
        // Config narrowed after the row was written: the memory is still
        // the user's, and must still be readable.
        let category = Category::from_stored("fact.homelab");
        assert_eq!(category.as_str(), "fact.homelab");
    }

    #[test]
    fn display_round_trips_through_parse() {
        for category in DEFAULT_CATEGORIES {
            let rendered = category.to_string();
            assert_eq!(&Category::parse(&rendered).unwrap(), category);
        }
    }
}
