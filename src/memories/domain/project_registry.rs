//! The projects a user's memories apply to (ai-specs/memory-structure-plan.md
//! §6).
//!
//! A memory stores a project's id, so a rename is one row. A project is found
//! by its name or by one of its aliases: the identifiers it is detected by (a
//! normalised git remote, a folder name) and the names it had before a rename.

use super::applies_to::ProjectRef;
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::{RaError, Result};

pub const MAX_PROJECT_NAME_LEN: usize = 64;

/// A project name as typed: lowercase, starting with a letter or digit, using
/// only `a-z 0-9 . _ -`. Validated rather than slugified, so "Quill App" is
/// refused with the rule instead of silently becoming a second project.
pub fn normalize_project_name(raw: &str) -> Result<String> {
    let name = raw.trim().to_lowercase();
    let valid = !name.is_empty()
        && name.chars().count() <= MAX_PROJECT_NAME_LEN
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".-_".contains(c));
    if valid {
        Ok(name)
    } else {
        Err(RaError::Rejected {
            code: "invalid_value".to_string(),
            message: format!(
                "project name {raw:?} must be 1 to {MAX_PROJECT_NAME_LEN} characters of a-z, \
                 0-9, '.', '_' or '-', like \"quill\""
            ),
        })
    }
}

pub trait ProjectRegistry: Send + Sync {
    /// The project with this name or alias, if the user has one.
    fn resolve(&self, context: &UserContext, identifier: &str) -> Result<Option<ProjectRef>>;

    /// Every project name this user has, sorted.
    fn names(&self, context: &UserContext) -> Result<Vec<String>>;

    /// Creates a project with these aliases. `Conflict` when the name is
    /// taken.
    fn create(&self, context: &UserContext, name: &str, aliases: &[String]) -> Result<ProjectRef>;

    /// Records another identifier for a project. Idempotent; an alias that
    /// already points at a different project is left alone.
    fn add_alias(&self, context: &UserContext, project: &ProjectRef, alias: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_lowercase_slugs() {
        assert_eq!(normalize_project_name(" Quill ").unwrap(), "quill");
        assert_eq!(
            normalize_project_name("me-quill.v2").unwrap(),
            "me-quill.v2"
        );
        for bad in ["", "quill app", "-quill", "café", &"q".repeat(65)] {
            let error = normalize_project_name(bad).unwrap_err();
            assert!(
                matches!(&error, RaError::Rejected { code, .. } if code == "invalid_value"),
                "{bad:?}: {error}"
            );
        }
    }
}
