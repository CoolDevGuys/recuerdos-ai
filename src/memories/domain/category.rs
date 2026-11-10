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

/// The longest custom category id.
pub const MAX_CATEGORY_ID_LEN: usize = 64;

/// The longest display label a user may give a category.
pub const MAX_CATEGORY_LABEL_LEN: usize = 64;

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

    /// A new custom category id, as a user creates or renames one.
    ///
    /// Stricter than what [`from_stored`](Self::from_stored) accepts, since
    /// this is the moment a name enters the taxonomy: lowercase letters,
    /// digits and `.`, `_`, `-`, starting with a letter, at most
    /// [`MAX_CATEGORY_ID_LEN`] characters, and not a built-in's id (a
    /// built-in is relabelled, never shadowed).
    pub fn custom(raw: &str) -> Result<Self> {
        let id = raw.trim().to_ascii_lowercase();
        if id.is_empty() {
            return Err(RaError::Validation("category id is empty".to_string()));
        }
        if id.chars().count() > MAX_CATEGORY_ID_LEN {
            return Err(RaError::Validation(format!(
                "category id is longer than {MAX_CATEGORY_ID_LEN} characters"
            )));
        }
        let well_formed = id.starts_with(|c: char| c.is_ascii_lowercase())
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c));
        if !well_formed {
            return Err(RaError::Validation(format!(
                "category id {id:?} must start with a letter and use only a-z, 0-9, '.', '_' and '-'"
            )));
        }
        if DEFAULT_CATEGORIES.iter().any(|c| c.as_str() == id) {
            return Err(RaError::Conflict(format!(
                "{id:?} is a built-in category; relabel it instead"
            )));
        }
        Ok(Category::Custom(id))
    }

    pub fn is_built_in(&self) -> bool {
        !matches!(self, Category::Custom(_))
    }

    /// The category as words: "Coding preference", "Health sleep log". The
    /// dotted ids (`preference.coding`) are a filter key, not language.
    ///
    /// Also the header of an enriched vector's text
    /// (`document_text.rs`): changing a label changes what is embedded, so
    /// it needs a new `+enriched-vN` pin, not just an edit here.
    pub fn label(&self) -> String {
        match self {
            Category::PreferenceCoding => "Coding preference".to_string(),
            Category::PreferencePersonal => "Personal preference".to_string(),
            Category::Decision => "Decision".to_string(),
            Category::FactProject => "Project fact".to_string(),
            Category::FactPerson => "Fact about a person".to_string(),
            Category::Experience => "Experience".to_string(),
            Category::Skill => "Skill".to_string(),
            Category::Reference => "Reference".to_string(),
            Category::Custom(name) => {
                let words = name.replace(['.', '_', '-'], " ");
                let mut chars = words.trim().chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => "Memory".to_string(),
                }
            }
        }
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
    fn a_custom_id_is_normalised_and_validated() {
        assert_eq!(
            Category::custom("  Health.Sleep ").unwrap(),
            Category::Custom("health.sleep".to_string())
        );
        for bad in [
            "",
            "9lives",
            "has space",
            "emoji🙂",
            ".dot",
            &"x".repeat(65),
        ] {
            assert!(
                matches!(Category::custom(bad), Err(RaError::Validation(_))),
                "{bad:?} should be refused"
            );
        }
    }

    #[test]
    fn a_custom_id_cannot_shadow_a_built_in() {
        assert!(matches!(
            Category::custom("Decision"),
            Err(RaError::Conflict(_))
        ));
    }

    #[test]
    fn display_round_trips_through_parse() {
        for category in DEFAULT_CATEGORIES {
            let rendered = category.to_string();
            assert_eq!(&Category::parse(&rendered).unwrap(), category);
        }
    }
}
