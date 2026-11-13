//! Who or what a memory is true for (ai-specs/memory-structure-plan.md §3.4).
//!
//! Before this, a memory had no notion of where it applies, so one project's
//! rule and the user's own standard were indistinguishable — both were
//! "coding preferences". `applies_to` is the axis recall needs to answer "what
//! are *my* coding preferences" from inside any project.

use crate::shared::error::{RaError, Result};
use crate::shared::ids::ProjectId;
use std::fmt;
use std::str::FromStr;

/// The three answers to "who is this true for?", without the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    /// Who the user is and how they like things, everywhere.
    Me,
    /// True of one project.
    Project,
    /// True of the user's world, not tied to a project: machines, files,
    /// services, places, appointments.
    General,
}

pub const ALL_TARGETS: &[Target] = &[Target::Me, Target::Project, Target::General];

impl Target {
    pub fn as_str(&self) -> &'static str {
        match self {
            Target::Me => "me",
            Target::Project => "project",
            Target::General => "general",
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Target {
    type Err = RaError;

    fn from_str(raw: &str) -> Result<Self> {
        let wanted = raw.trim().to_ascii_lowercase();
        ALL_TARGETS
            .iter()
            .copied()
            .find(|target| target.as_str() == wanted)
            .ok_or_else(|| {
                RaError::Validation(format!(
                    "unknown applies_to target {raw:?}: one of me, project, general"
                ))
            })
    }
}

/// A project as a memory refers to it: the id it is stored by, and the name
/// it is shown and typed as. The id is what a rename leaves alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    pub id: ProjectId,
    pub name: String,
}

/// The full answer: the target, and the project when the target is one.
///
/// Holding the project inside the variant is what makes "a project name on a
/// memory that applies to me" unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppliesTo {
    Me,
    Project(ProjectRef),
    General,
}

impl AppliesTo {
    pub fn target(&self) -> Target {
        match self {
            AppliesTo::Me => Target::Me,
            AppliesTo::Project(_) => Target::Project,
            AppliesTo::General => Target::General,
        }
    }

    pub fn project(&self) -> Option<&ProjectRef> {
        match self {
            AppliesTo::Project(project) => Some(project),
            AppliesTo::Me | AppliesTo::General => None,
        }
    }

    /// How a prompt or a reply names it: `me`, `project quill`, `general`.
    pub fn label(&self) -> String {
        match self {
            AppliesTo::Project(project) => format!("project {}", project.name),
            AppliesTo::Me | AppliesTo::General => self.target().to_string(),
        }
    }

    /// Whether two memories share a scope: the same target and, for a
    /// project, the same project (by id, so a rename does not split it).
    /// Contradiction checks and merges only compare memories that do.
    pub fn same_scope(&self, other: &AppliesTo) -> bool {
        match (self, other) {
            (AppliesTo::Project(a), AppliesTo::Project(b)) => a.id == b.id,
            _ => self.target() == other.target(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> ProjectRef {
        ProjectRef {
            id: ProjectId::new(),
            name: name.to_string(),
        }
    }

    #[test]
    fn targets_round_trip_through_their_names() {
        for target in ALL_TARGETS {
            assert_eq!(target.as_str().parse::<Target>().unwrap(), *target);
        }
        assert!("everywhere".parse::<Target>().is_err());
    }

    #[test]
    fn the_target_follows_the_variant() {
        assert_eq!(AppliesTo::Me.target(), Target::Me);
        assert_eq!(AppliesTo::General.target(), Target::General);
        assert_eq!(
            AppliesTo::Project(project("quill")).target(),
            Target::Project
        );
        assert_eq!(AppliesTo::Me.project(), None);
    }

    #[test]
    fn scopes_match_by_target_and_project_id() {
        let quill = project("quill");
        let renamed = ProjectRef {
            id: quill.id,
            name: "quill-app".to_string(),
        };

        assert!(AppliesTo::Me.same_scope(&AppliesTo::Me));
        assert!(!AppliesTo::Me.same_scope(&AppliesTo::General));
        assert!(
            AppliesTo::Project(quill.clone()).same_scope(&AppliesTo::Project(renamed)),
            "a rename keeps the scope"
        );
        assert!(!AppliesTo::Project(quill).same_scope(&AppliesTo::Project(project("fern"))));
    }
}
