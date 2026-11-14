//! Where an extracted candidate is filed (ai-specs/memory-structure-plan.md
//! §9, "the pipeline"): the same structure, under the same rules, as a memory
//! an agent saves directly.
//!
//! A direct save can refuse and let the agent ask the user. A pipeline job
//! cannot ask anyone, so where a save would be refused this either files the
//! candidate somewhere safe and marks it for review, or holds it back:
//!
//! - an inferred memory about the user is never stored (D5). It comes back as
//!   a suggestion for the agent to ask about;
//! - a credential is dropped (R9);
//! - a project memory with no known project is filed as `general` and marked
//!   for review, since a job cannot confirm a new project (R3);
//! - text tied to one project is filed under it when it is known (R7);
//! - a kind its target does not allow is moved to one that fits, and marked
//!   (R2). A project's "preference" is its convention;
//! - a memory whose subject is the user ("The user is…", or one of their
//!   names) is filed as theirs, `me`, not as `general` (R13).
//!
//! "Marked" is `needs_review`: the web UI's review queue lists it, and the
//! user's edit clears it.

use super::candidate::Candidate;
use crate::memories::domain::applies_to::{AppliesTo, ProjectRef, Target};
use crate::memories::domain::content_checks::{project_phrase, secret_in};
use crate::memories::domain::kind::Kind;
use crate::memories::domain::legacy_structure::legacy_structure;
use crate::memories::domain::memory_structure::{MemoryStructure, Origin};
use crate::memories::domain::self_names::{SelfNames, self_subject};

#[derive(Debug, Clone, PartialEq)]
pub enum Filing {
    Store(MemoryStructure),
    /// An inference about the user: theirs to confirm, so not stored.
    Suggest,
    /// Never stored, for this reason.
    Drop(String),
}

pub fn file_candidate(
    candidate: &Candidate,
    project: Option<&ProjectRef>,
    names: &SelfNames,
) -> Filing {
    if let Some(found) = secret_in(&candidate.content) {
        return Filing::Drop(format!("it looks like it contains {found}"));
    }

    let placement = &candidate.placement;
    let derived = legacy_structure(&candidate.category, &candidate.tags);
    let mut kind = placement.kind.unwrap_or(derived.kind());
    let topic = placement
        .topic
        .clone()
        .unwrap_or_else(|| derived.topic().clone());
    let origin = if placement.inferred {
        Origin::Inferred
    } else {
        Origin::UserSaid
    };

    // Unsure means review. The model's own answer is trusted as far as the
    // rules allow; a missing one is a guess.
    let mut review = placement.target.is_none() || placement.kind.is_none();

    // D4 when the model said nothing: the project when one is known, else
    // general, never the user.
    let mut target = placement.target.unwrap_or(if project.is_some() {
        Target::Project
    } else {
        Target::General
    });

    // R7: text tied to one codebase is that project's.
    if target != Target::Project && project_phrase(&candidate.content).is_some() {
        if project.is_some() {
            target = Target::Project;
        } else {
            review = true;
        }
    }

    // R13: a memory whose subject is the user is about them, however the
    // model filed it: "The user is an expert in PHP", or one of their names.
    if target == Target::General
        && kind.allows(Target::Me)
        && self_subject(&candidate.content, names).is_some()
    {
        target = Target::Me;
    }

    // R3: no project to file it under.
    if target == Target::Project && project.is_none() {
        target = Target::General;
        review = true;
    }

    // R2: a project's way of doing things is a convention, and a kind its
    // target does not allow moves to one that fits.
    if kind == Kind::Preference && target == Target::Project {
        kind = Kind::Convention;
    }
    if !kind.allows(target) {
        review = true;
        kind = match (kind, target) {
            (Kind::Preference, _) => Kind::Rule,
            (Kind::Convention, _) => Kind::Rule,
            (Kind::Decision, _) => Kind::Fact,
            (Kind::Goal, _) | (Kind::Plan, _) => Kind::Fact,
            (other, _) => other,
        };
    }
    // R10: a plan needs an end date, which extraction does not give. What it
    // describes is still worth keeping, as a fact to review.
    if kind == Kind::Plan {
        kind = Kind::Fact;
        review = true;
    }

    // D5: what the user prefers is theirs to say.
    if target == Target::Me && origin == Origin::Inferred {
        return Filing::Suggest;
    }

    let applies_to = match (target, project) {
        (Target::Me, _) => AppliesTo::Me,
        (Target::Project, Some(project)) => AppliesTo::Project(project.clone()),
        (Target::General, _) | (Target::Project, None) => AppliesTo::General,
    };
    match MemoryStructure::new(kind, topic, applies_to, origin, None) {
        Ok(structure) => Filing::Store(structure.flagged_for_review(review)),
        // Unreachable by the moves above; filed as its category says rather
        // than lost.
        Err(error) => {
            tracing::warn!(%error, "an extracted memory's structure did not hold together");
            Filing::Store(derived.flagged_for_review(true))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::domain::category::Category;
    use crate::memories::domain::topic::Topic;
    use crate::shared::ids::ProjectId;
    use crate::understanding::domain::candidate::Placement;

    fn file(candidate: &Candidate, project: Option<&ProjectRef>) -> Filing {
        file_candidate(candidate, project, &SelfNames::default())
    }

    fn quill() -> ProjectRef {
        ProjectRef {
            id: ProjectId::new(),
            name: "quill".to_string(),
        }
    }

    fn candidate(content: &str, placement: Placement) -> Candidate {
        Candidate {
            content: content.to_string(),
            category: Category::PreferenceCoding,
            subcategory: None,
            tags: vec![],
            entities: vec![],
            relations: vec![],
            confidence: 0.9,
            placement,
        }
    }

    fn placed(kind: Kind, target: Target, inferred: bool) -> Placement {
        Placement {
            kind: Some(kind),
            topic: Some(Topic::Coding),
            target: Some(target),
            inferred,
        }
    }

    fn stored(filing: Filing) -> MemoryStructure {
        match filing {
            Filing::Store(structure) => structure,
            other => panic!("expected it stored, got {other:?}"),
        }
    }

    #[test]
    fn what_the_user_said_about_themselves_is_theirs() {
        let structure = stored(file(
            &candidate(
                "The user prefers mocks in unit tests",
                placed(Kind::Preference, Target::Me, false),
            ),
            Some(&quill()),
        ));

        assert_eq!(structure.applies_to(), &AppliesTo::Me);
        assert_eq!(structure.kind(), Kind::Preference);
        assert!(!structure.needs_review());
    }

    #[test]
    fn an_inference_about_the_user_is_held_back_as_a_suggestion() {
        let filing = file(
            &candidate(
                "The user prefers small pull requests",
                placed(Kind::Preference, Target::Me, true),
            ),
            None,
        );
        assert_eq!(filing, Filing::Suggest);
    }

    #[test]
    fn a_projects_preference_is_its_convention() {
        let project = quill();
        let structure = stored(file(
            &candidate(
                "Use cases are named by intent",
                placed(Kind::Preference, Target::Project, false),
            ),
            Some(&project),
        ));

        assert_eq!(structure.kind(), Kind::Convention);
        assert_eq!(structure.applies_to(), &AppliesTo::Project(project));
        assert!(!structure.needs_review());
    }

    #[test]
    fn a_project_memory_without_a_known_project_is_general_and_marked() {
        let structure = stored(file(
            &candidate(
                "Never mock the database",
                placed(Kind::Rule, Target::Project, false),
            ),
            None,
        ));

        assert_eq!(structure.applies_to(), &AppliesTo::General);
        assert!(structure.needs_review());
    }

    #[test]
    fn text_tied_to_the_codebase_follows_the_project() {
        let project = quill();
        let structure = stored(file(
            &candidate(
                "The codebase uses Python 3.12",
                placed(Kind::Fact, Target::Me, false),
            ),
            Some(&project),
        ));
        assert_eq!(structure.applies_to(), &AppliesTo::Project(project));
    }

    #[test]
    fn a_missing_placement_defaults_away_from_the_user_and_is_marked() {
        let structure = stored(file(&candidate("Prefers pnpm", Placement::default()), None));

        assert_eq!(structure.applies_to(), &AppliesTo::General);
        assert!(structure.needs_review());
    }

    #[test]
    fn a_credential_is_dropped() {
        let filing = file(
            &candidate(
                "The router password is hunter2hunter2",
                placed(Kind::Fact, Target::General, false),
            ),
            None,
        );
        assert!(matches!(filing, Filing::Drop(_)), "{filing:?}");
    }

    #[test]
    fn a_fact_about_the_user_filed_as_general_is_theirs() {
        // What v0.6.0's extraction did with "The user is an expert in PHP…":
        // a fact about a person, in their world.
        let structure = stored(file(
            &candidate(
                "The user is an expert in PHP and proficient in Python",
                placed(Kind::Fact, Target::General, false),
            ),
            None,
        ));
        assert_eq!(structure.applies_to(), &AppliesTo::Me);
        assert_eq!(structure.kind(), Kind::Fact);
    }

    #[test]
    fn a_memory_that_names_the_user_is_theirs_and_one_about_others_is_not() {
        let names = SelfNames::parse(&["Alejandro Romero".to_string()]).unwrap();
        let about_them = file_candidate(
            &candidate(
                "Alejandro Romero is a Senior Backend Software Engineer in Berlin",
                placed(Kind::Fact, Target::General, false),
            ),
            None,
            &names,
        );
        assert_eq!(stored(about_them).applies_to(), &AppliesTo::Me);

        let about_ana = file_candidate(
            &candidate(
                "The user's sister Ana is allergic to nuts",
                placed(Kind::Fact, Target::General, false),
            ),
            None,
            &names,
        );
        assert_eq!(stored(about_ana).applies_to(), &AppliesTo::General);
    }

    #[test]
    fn an_inferred_fact_about_the_user_is_still_theirs_to_confirm() {
        let filing = file(
            &candidate(
                "The user works mostly at night",
                placed(Kind::Fact, Target::General, true),
            ),
            None,
        );
        assert_eq!(filing, Filing::Suggest);
    }
}
