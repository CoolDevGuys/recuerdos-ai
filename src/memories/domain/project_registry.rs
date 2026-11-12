//! The projects a user's memories apply to (ai-specs/memory-structure-plan.md
//! §6).
//!
//! A memory stores a project's id, so a rename is one row. A project is found
//! by its name or by one of its aliases: the identifiers it is detected by (a
//! normalised git remote, a folder name) and the names it had before a rename.

use super::applies_to::ProjectRef;
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::ProjectId;
use chrono::{DateTime, Utc};

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
        Err(RaError::rejected(
            "invalid_value",
            format!(
                "project name {raw:?} must be 1 to {MAX_PROJECT_NAME_LEN} characters of a-z, \
                 0-9, '.', '_' or '-', like \"quill\""
            ),
        ))
    }
}

/// A project as the registry holds it: what `project_list` and
/// `GET /v1/projects` show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub project: ProjectRef,
    /// Sorted: remotes, folder names and former names.
    pub aliases: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// Whether an alias is a normalised git remote (`github.com/acme/quill`)
/// rather than a folder or former name. A name cannot hold a `/`.
pub fn is_remote(alias: &str) -> bool {
    alias.contains('/')
}

/// The owner and repository of a remote alias: `github.com/acme/quill` →
/// (`github.com/acme`, `quill`). `None` for anything else.
pub fn remote_parts(alias: &str) -> Option<(&str, &str)> {
    alias
        .rsplit_once('/')
        .filter(|(owner, repo)| is_remote(alias) && !owner.is_empty() && !repo.is_empty())
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

    /// Every project this user has, by name.
    fn list(&self, context: &UserContext) -> Result<Vec<ProjectRecord>>;

    /// The project with this id, if it is the user's.
    fn find(&self, context: &UserContext, id: ProjectId) -> Result<Option<ProjectRef>>;

    /// Gives a project a new (already normalised) name. The old name becomes
    /// an alias, so older checkouts and configs keep resolving, and
    /// `aliases` are recorded too. `Conflict` when another project has the
    /// name. Memories store the id, so none of them is touched.
    fn rename(
        &self,
        context: &UserContext,
        project: &ProjectRef,
        new_name: &str,
        aliases: &[String],
    ) -> Result<ProjectRef>;

    /// Folds `from` into `into`, in one transaction: any memory still filed
    /// under `from` moves, its aliases and its name become `into`'s, and
    /// `from` is deleted. The caller moves the memories it can audit first;
    /// this catches the rest (forgotten ones), which would otherwise hold
    /// the row in place.
    fn absorb(&self, context: &UserContext, from: &ProjectRef, into: &ProjectRef) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_alias_splits_into_owner_and_repository() {
        assert_eq!(
            remote_parts("github.com/acme/quill"),
            Some(("github.com/acme", "quill"))
        );
        assert_eq!(remote_parts("quill"), None);
        assert!(is_remote("gitlab.com/group/sub/quill"));
        assert!(!is_remote("quill-app"));
    }

    #[test]
    fn names_are_lowercase_slugs() {
        assert_eq!(normalize_project_name(" Quill ").unwrap(), "quill");
        assert_eq!(
            normalize_project_name("me-quill.v2").unwrap(),
            "me-quill.v2"
        );
        for bad in [
            "",
            "quill app",
            "-quill",
            "café",
            "acme/quill",
            &"q".repeat(65),
        ] {
            let error = normalize_project_name(bad).unwrap_err();
            assert!(
                matches!(&error, RaError::Rejected { code, .. } if code == "invalid_value"),
                "{bad:?}: {error}"
            );
        }
    }
}
