//! A memory's structure: what sort of statement it is, what it is about, who
//! it is true for, and where it came from (ai-specs/memory-structure-plan.md
//! §3).
//!
//! The rules that hold whatever the entry point live here, so no client can
//! store an impossible combination: a kind only applies to the targets it
//! allows (§3.2), and only a goal has a goal status (§3.8).

use super::applies_to::AppliesTo;
use super::kind::Kind;
use super::topic::Topic;
use crate::shared::error::{RaError, Result};
use std::fmt;
use std::str::FromStr;

/// Where a memory came from (§3.5). The honest signal for how far to trust
/// it, which is why confidence is derived from it rather than asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The user stated it.
    UserSaid,
    /// The agent proposed it and the user said yes.
    UserConfirmed,
    /// The agent noticed a pattern itself.
    Inferred,
    /// Read from a file: CLAUDE.md, CONTRIBUTING, a README.
    Document,
}

impl Origin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Origin::UserSaid => "user_said",
            Origin::UserConfirmed => "user_confirmed",
            Origin::Inferred => "inferred",
            Origin::Document => "document",
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Origin {
    type Err = RaError;

    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "user_said" => Ok(Origin::UserSaid),
            "user_confirmed" => Ok(Origin::UserConfirmed),
            "inferred" => Ok(Origin::Inferred),
            "document" => Ok(Origin::Document),
            _ => Err(RaError::Validation(format!(
                "unknown origin {raw:?}: one of user_said, user_confirmed, inferred, document"
            ))),
        }
    }
}

/// Where a goal stands (§3.8). Goals close when the user says so, not by
/// time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GoalStatus {
    Active,
    Achieved,
    Abandoned,
}

impl GoalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Achieved => "achieved",
            GoalStatus::Abandoned => "abandoned",
        }
    }

    pub fn is_closed(&self) -> bool {
        !matches!(self, GoalStatus::Active)
    }
}

impl fmt::Display for GoalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for GoalStatus {
    type Err = RaError;

    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "active" => Ok(GoalStatus::Active),
            "achieved" => Ok(GoalStatus::Achieved),
            "abandoned" => Ok(GoalStatus::Abandoned),
            _ => Err(RaError::Validation(format!(
                "unknown goal_status {raw:?}: one of active, achieved, abandoned"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryStructure {
    kind: Kind,
    topic: Topic,
    applies_to: AppliesTo,
    origin: Origin,
    /// `Some` exactly when the kind is a goal.
    goal_status: Option<GoalStatus>,
    /// Set when the structure was derived rather than stated — by the
    /// migration from the old categories (§8) — and could be wrong. The web
    /// UI's review queue lists these; a user's edit clears it.
    needs_review: bool,
}

impl MemoryStructure {
    /// A validated structure. A goal without a status starts `Active`.
    pub fn new(
        kind: Kind,
        topic: Topic,
        applies_to: AppliesTo,
        origin: Origin,
        goal_status: Option<GoalStatus>,
    ) -> Result<Self> {
        let target = applies_to.target();
        if !kind.allows(target) {
            let allowed: Vec<&str> = kind.allowed_targets().iter().map(|t| t.as_str()).collect();
            return Err(RaError::Validation(format!(
                "a {kind} cannot apply to {target}: it applies to {}",
                allowed.join(" or ")
            )));
        }
        let goal_status = match (kind, goal_status) {
            (Kind::Goal, status) => Some(status.unwrap_or(GoalStatus::Active)),
            (_, None) => None,
            (_, Some(_)) => {
                return Err(RaError::Validation(format!(
                    "goal_status is only for goals, not a {kind}"
                )));
            }
        };
        Ok(Self {
            kind,
            topic,
            applies_to,
            origin,
            goal_status,
            needs_review: false,
        })
    }

    /// Rebuilds a stored structure without re-validating: stored rows are
    /// historical fact, and refusing to load one would hide a user's memory
    /// from them because the rules moved.
    pub fn from_stored(
        kind: Kind,
        topic: Topic,
        applies_to: AppliesTo,
        origin: Origin,
        goal_status: Option<GoalStatus>,
        needs_review: bool,
    ) -> Self {
        Self {
            kind,
            topic,
            applies_to,
            origin,
            goal_status,
            needs_review,
        }
    }

    pub fn flagged_for_review(mut self, needs_review: bool) -> Self {
        self.needs_review = needs_review;
        self
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn topic(&self) -> &Topic {
        &self.topic
    }

    pub fn applies_to(&self) -> &AppliesTo {
        &self.applies_to
    }

    pub fn origin(&self) -> Origin {
        self.origin
    }

    pub fn goal_status(&self) -> Option<GoalStatus> {
        self.goal_status
    }

    pub fn needs_review(&self) -> bool {
        self.needs_review
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::domain::applies_to::ProjectRef;
    use crate::shared::ids::ProjectId;

    fn quill() -> AppliesTo {
        AppliesTo::Project(ProjectRef {
            id: ProjectId::new(),
            name: "quill".to_string(),
        })
    }

    #[test]
    fn a_valid_pair_is_accepted() {
        let structure = MemoryStructure::new(
            Kind::Preference,
            Topic::Coding,
            AppliesTo::Me,
            Origin::UserSaid,
            None,
        )
        .unwrap();

        assert_eq!(structure.kind(), Kind::Preference);
        assert_eq!(structure.applies_to(), &AppliesTo::Me);
        assert!(!structure.needs_review());
    }

    #[test]
    fn an_illegal_pair_is_refused_with_what_is_allowed() {
        let error = MemoryStructure::new(
            Kind::Preference,
            Topic::Coding,
            quill(),
            Origin::UserSaid,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("a preference cannot apply to project"),
            "{error}"
        );
        assert!(error.contains("applies to me"), "{error}");

        assert!(
            MemoryStructure::new(
                Kind::Convention,
                Topic::Coding,
                AppliesTo::Me,
                Origin::UserSaid,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn a_goal_starts_active_and_only_goals_have_a_status() {
        let goal = MemoryStructure::new(
            Kind::Goal,
            Topic::Health,
            AppliesTo::Me,
            Origin::UserSaid,
            None,
        )
        .unwrap();
        assert_eq!(goal.goal_status(), Some(GoalStatus::Active));

        assert!(
            MemoryStructure::new(
                Kind::Fact,
                Topic::Health,
                AppliesTo::Me,
                Origin::UserSaid,
                Some(GoalStatus::Achieved)
            )
            .is_err()
        );
    }

    #[test]
    fn origins_and_goal_statuses_round_trip() {
        for origin in [
            Origin::UserSaid,
            Origin::UserConfirmed,
            Origin::Inferred,
            Origin::Document,
        ] {
            assert_eq!(origin.as_str().parse::<Origin>().unwrap(), origin);
        }
        for status in [
            GoalStatus::Active,
            GoalStatus::Achieved,
            GoalStatus::Abandoned,
        ] {
            assert_eq!(status.as_str().parse::<GoalStatus>().unwrap(), status);
        }
        assert!(GoalStatus::Achieved.is_closed());
        assert!(!GoalStatus::Active.is_closed());
        assert!("said".parse::<Origin>().is_err());
    }
}
