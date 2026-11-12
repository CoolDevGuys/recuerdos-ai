//! What sort of statement a memory is (ai-specs/memory-structure-plan.md §3.2).
//!
//! The kind is half of what used to be fused into one category label: a
//! project's agreed rule and the user's own preference are different kinds,
//! and recall can only tell them apart if the store does. Each kind carries
//! the question an agent asks itself to choose it, and the targets it may
//! apply to; an illegal pair is the cue for the question that should have
//! been asked ("a project's way of doing things is a convention").

use super::applies_to::Target;
use crate::shared::error::{RaError, Result};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Preference,
    Rule,
    Convention,
    Decision,
    Fact,
    Experience,
    Learning,
    Procedure,
    Reference,
    Goal,
    Plan,
}

/// Every kind, in the order the plan and the docs list them.
pub const ALL_KINDS: &[Kind] = &[
    Kind::Preference,
    Kind::Rule,
    Kind::Convention,
    Kind::Decision,
    Kind::Fact,
    Kind::Experience,
    Kind::Learning,
    Kind::Procedure,
    Kind::Reference,
    Kind::Goal,
    Kind::Plan,
];

const ANY: &[Target] = &[Target::Me, Target::Project, Target::General];

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Preference => "preference",
            Kind::Rule => "rule",
            Kind::Convention => "convention",
            Kind::Decision => "decision",
            Kind::Fact => "fact",
            Kind::Experience => "experience",
            Kind::Learning => "learning",
            Kind::Procedure => "procedure",
            Kind::Reference => "reference",
            Kind::Goal => "goal",
            Kind::Plan => "plan",
        }
    }

    /// The question that decides this kind, for tool descriptions and docs.
    #[allow(dead_code)] // read by the save tool's description from S1 on
    pub fn question(&self) -> &'static str {
        match self {
            Kind::Preference => {
                "Is this what the user picks when free to choose? Agents may deviate, with a reason."
            }
            Kind::Rule => "Is this a hard line an agent must never cross?",
            Kind::Convention => "Is this how one project agreed to do things?",
            Kind::Decision => "Was a choice made at a point in time, for a reason?",
            Kind::Fact => "Is this a durable truth about something or someone?",
            Kind::Experience => "Is this something that happened?",
            Kind::Learning => "Is this the lesson drawn from something that happened?",
            Kind::Procedure => "Is this a set of steps to follow again?",
            Kind::Reference => "Is this a pointer to something else (a URL, a path, a ticket)?",
            Kind::Goal => {
                "Is this something the user is working towards? It stays active until the user \
                 confirms it is done."
            }
            Kind::Plan => "Is this a dated thing ahead? It needs valid_until.",
        }
    }

    /// The targets this kind may apply to.
    ///
    /// A preference is the user's by definition; a convention is a project's
    /// by definition. A decision is never "who the user is" — a personal
    /// decision is a preference or a rule — so it applies to a project or to
    /// the user's world.
    pub fn allowed_targets(&self) -> &'static [Target] {
        match self {
            Kind::Preference => &[Target::Me],
            Kind::Convention => &[Target::Project],
            Kind::Decision => &[Target::Project, Target::General],
            Kind::Goal => &[Target::Me, Target::Project],
            Kind::Plan => &[Target::General, Target::Project],
            Kind::Rule
            | Kind::Fact
            | Kind::Experience
            | Kind::Learning
            | Kind::Procedure
            | Kind::Reference => ANY,
        }
    }

    pub fn allows(&self, target: Target) -> bool {
        self.allowed_targets().contains(&target)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Kind {
    type Err = RaError;

    fn from_str(raw: &str) -> Result<Self> {
        let wanted = raw.trim().to_ascii_lowercase();
        ALL_KINDS
            .iter()
            .copied()
            .find(|kind| kind.as_str() == wanted)
            .ok_or_else(|| {
                let known: Vec<&str> = ALL_KINDS.iter().map(Kind::as_str).collect();
                RaError::Validation(format!("unknown kind {raw:?}: one of {}", known.join(", ")))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_round_trips_through_its_name() {
        for kind in ALL_KINDS {
            assert_eq!(kind.as_str().parse::<Kind>().unwrap(), *kind);
        }
    }

    #[test]
    fn parsing_ignores_case_and_surrounding_space() {
        assert_eq!(" Preference ".parse::<Kind>().unwrap(), Kind::Preference);
    }

    #[test]
    fn an_unknown_kind_names_the_known_ones() {
        let error = "habit".parse::<Kind>().unwrap_err().to_string();
        assert!(error.contains("habit"), "{error}");
        assert!(error.contains("preference, rule, convention"), "{error}");
    }

    #[test]
    fn the_allowed_pairs_match_the_plan() {
        // ai-specs/memory-structure-plan.md §3.2, row by row.
        let only = |kind: Kind, targets: &[Target]| {
            for target in [Target::Me, Target::Project, Target::General] {
                assert_eq!(
                    kind.allows(target),
                    targets.contains(&target),
                    "{kind} × {target}"
                );
            }
        };
        only(Kind::Preference, &[Target::Me]);
        only(Kind::Convention, &[Target::Project]);
        only(Kind::Decision, &[Target::Project, Target::General]);
        only(Kind::Goal, &[Target::Me, Target::Project]);
        only(Kind::Plan, &[Target::General, Target::Project]);
        for kind in [
            Kind::Rule,
            Kind::Fact,
            Kind::Experience,
            Kind::Learning,
            Kind::Procedure,
            Kind::Reference,
        ] {
            only(kind, ANY);
        }
    }

    #[test]
    fn every_kind_has_a_question() {
        for kind in ALL_KINDS {
            assert!(kind.question().ends_with('?') || kind.question().ends_with('.'));
        }
    }
}
