//! Amends a stored memory (ai-specs/memory-structure-plan.md §5.2): its
//! wording, its structure, or — for a goal — whether it is still open.
//!
//! The same rules a save is held to apply to what the memory becomes: no
//! secret, a kind its target allows, text that does not tie itself to one
//! project while filed as the user's, and only the user closes a goal (R12).
//! What an edit is *not* checked for is conflict with other memories: amending
//! is how a conflict is resolved, and refusing the fix would leave the user no
//! way out.

use super::memory_updater::MemoryUpdater;
use super::structured_memory_saver::{
    AppliesToInput, DetectedProject, ProjectPlan, StructuredMemorySaver, invalid, mismatch_message,
    parse_kind, parse_origin, refuse_project_phrases, refuse_secrets, rejected,
};
use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::applies_to::{AppliesTo, ProjectRef, Target};
use crate::memories::domain::category::Category;
use crate::memories::domain::kind::Kind;
use crate::memories::domain::memory::{Memory, MemoryEdit};
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::memory_structure::{GoalStatus, MemoryStructure, Origin};
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{MemoryId, ProjectId};
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// An edit as the caller sent it. `None` leaves a field alone; for
/// `subtopic` and `valid_until`, `Some(None)` clears it.
#[derive(Debug, Clone, Default)]
pub struct StructuredEdit {
    pub content: Option<String>,
    /// Already resolved against the user's categories by the caller.
    pub category: Option<Category>,
    pub subtopic: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    pub valid_until: Option<Option<DateTime<Utc>>>,
    pub kind: Option<String>,
    pub topic: Option<String>,
    pub applies_to: Option<AppliesToInput>,
    /// `achieved` or `abandoned` closes a goal; `active` reopens it.
    pub goal_status: Option<String>,
    /// Who asked for this change: required to close a goal, and the memory's
    /// new origin when its wording or scope changes.
    pub origin: Option<String>,
    pub expected_updated_at: Option<DateTime<Utc>>,
    pub detected_project: Option<DetectedProject>,
}

impl StructuredEdit {
    fn changes_structure(&self) -> bool {
        self.kind.is_some()
            || self.topic.is_some()
            || self.applies_to.is_some()
            || self.goal_status.is_some()
            || self.origin.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateNote {
    /// The goal was closed: it leaves recall and the profile, and stays in
    /// history.
    GoalClosed(GoalStatus),
    GoalReopened,
    ProjectCreated(String),
}

#[derive(Debug, Clone)]
pub struct UpdatedMemory {
    pub memory: Memory,
    pub notes: Vec<UpdateNote>,
}

impl UpdatedMemory {
    /// The sentence to repeat to the user.
    pub fn message(&self) -> String {
        let structure = self.memory.structure();
        let what = format!("{} {}", structure.topic(), structure.kind());
        let filed = match structure.applies_to() {
            AppliesTo::Me => format!("your personal {what}"),
            AppliesTo::Project(project) => format!("a {what} of {}", project.name),
            AppliesTo::General => format!("a general {what}"),
        };
        let mut message = format!("Updated; filed as {filed}.");
        for note in &self.notes {
            message.push(' ');
            message.push_str(&match note {
                UpdateNote::GoalClosed(status) => format!(
                    "The goal is closed as {status}: it leaves recall and the profile, and \
                     stays in history."
                ),
                UpdateNote::GoalReopened => "The goal is open again.".to_string(),
                UpdateNote::ProjectCreated(name) => format!("Created the project {name}."),
            });
        }
        message
    }
}

pub struct StructuredMemoryUpdater {
    updater: Arc<MemoryUpdater>,
    memories: Arc<dyn MemoryRepository>,
    rules: Arc<StructuredMemorySaver>,
}

impl StructuredMemoryUpdater {
    pub fn new(
        updater: Arc<MemoryUpdater>,
        memories: Arc<dyn MemoryRepository>,
        rules: Arc<StructuredMemorySaver>,
    ) -> Self {
        Self {
            updater,
            memories,
            rules,
        }
    }

    pub fn execute(
        &self,
        context: &UserContext,
        id: MemoryId,
        edit: StructuredEdit,
        actor: &str,
    ) -> Result<UpdatedMemory> {
        let existing = self
            .memories
            .find(context, id)?
            .ok_or_else(|| RaError::NotFound(format!("memory {id} not found")))?;

        // R9 on every new wording.
        if let Some(content) = &edit.content {
            refuse_secrets(content)?;
        }

        let mut notes = Vec::new();
        let (structure, project_to_create) = if edit.changes_structure() {
            self.restructure(context, &existing, &edit, &mut notes)?
        } else {
            (None, None)
        };

        // The rules on what the memory becomes, structure changed or not:
        // a new wording can tie a personal memory to one project, and a
        // valid_until must suit the kind.
        let after = structure.as_ref().unwrap_or_else(|| existing.structure());
        let content = edit.content.as_deref().unwrap_or(existing.content());
        if edit.content.is_some() || structure.is_some() {
            refuse_project_phrases(content, after.applies_to().target())?;
        }
        self.check_valid_until(&existing, after.kind(), &edit)?;

        // Everything is checked: only now may a project be created.
        let structure = match (structure, project_to_create) {
            (Some(structure), Some(plan)) => {
                let (project, created) = self.rules.settle_project(context, plan)?;
                if created {
                    notes.push(UpdateNote::ProjectCreated(project.name.clone()));
                }
                Some(MemoryStructure::new(
                    structure.kind(),
                    structure.topic().clone(),
                    AppliesTo::Project(project),
                    structure.origin(),
                    structure.goal_status(),
                )?)
            }
            (structure, _) => structure,
        };

        let memory = self.updater.execute(
            context,
            id,
            MemoryEdit {
                content: edit.content,
                category: edit.category,
                subcategory: edit.subtopic,
                tags: edit.tags,
                expires_at: edit.valid_until,
                structure,
                expected_updated_at: edit.expected_updated_at,
            },
            actor,
        )?;
        Ok(UpdatedMemory { memory, notes })
    }

    /// The structure the edit asks for, checked; and the project to create
    /// for it, when it moves the memory to a confirmed new one.
    fn restructure(
        &self,
        context: &UserContext,
        existing: &Memory,
        edit: &StructuredEdit,
        notes: &mut Vec<UpdateNote>,
    ) -> Result<(Option<MemoryStructure>, Option<ProjectPlan>)> {
        let before = existing.structure();
        let kind = match &edit.kind {
            Some(raw) => parse_kind(Some(raw))?,
            None => before.kind(),
        };
        let topic = match &edit.topic {
            Some(raw) => self.rules.resolve_topic(context, Some(raw))?,
            None => before.topic().clone(),
        };
        let asked_by = edit
            .origin
            .as_deref()
            .map(|raw| parse_origin(Some(raw)))
            .transpose()?;

        // Where it applies: unchanged, or resolved exactly as a save would.
        let mut plan = None;
        let applies_to = match &edit.applies_to {
            None => before.applies_to().clone(),
            Some(input) => {
                let target = match input.target.as_deref() {
                    Some(raw) => raw.parse::<Target>().map_err(invalid)?,
                    None if input.project_name.is_some() => Target::Project,
                    None => {
                        return Err(rejected(
                            "missing_field",
                            "applies_to.target is required: me, project or general.",
                        ));
                    }
                };
                if input.project_name.is_some() && target != Target::Project {
                    return Err(rejected(
                        "invalid_value",
                        "applies_to.project_name is only for target \"project\".",
                    ));
                }
                match target {
                    Target::Me => AppliesTo::Me,
                    Target::General => AppliesTo::General,
                    Target::Project => {
                        let detected_known = match &edit.detected_project {
                            Some(detected) => self.rules.resolve_detected(context, detected)?,
                            None => None,
                        };
                        let planned = self.rules.plan_project(
                            context,
                            input,
                            edit.detected_project.as_ref(),
                            detected_known,
                        )?;
                        match planned {
                            ProjectPlan::Existing(project) => AppliesTo::Project(project),
                            ProjectPlan::Create { ref name, .. } => {
                                // Stands in until the project exists; the
                                // checks below only need the target.
                                let stand_in = ProjectRef {
                                    id: ProjectId::new(),
                                    name: name.clone(),
                                };
                                plan = Some(planned);
                                AppliesTo::Project(stand_in)
                            }
                        }
                    }
                }
            }
        };
        let target = applies_to.target();

        // R2.
        if !kind.allows(target) {
            return Err(rejected(
                "kind_target_mismatch",
                mismatch_message(kind, target, false),
            ));
        }

        // The memory's origin follows a change to what it says or where it
        // applies; a goal being closed, or a re-label, leaves it as it was.
        let reworded = edit.content.is_some()
            || edit.kind.is_some()
            || !applies_to.same_scope(before.applies_to());
        let origin = match asked_by {
            Some(origin) if reworded => origin,
            _ => before.origin(),
        };
        // R5: a memory about the user that was inferred is theirs to confirm,
        // however it got there.
        if target == Target::Me
            && origin == Origin::Inferred
            && !before.applies_to().same_scope(&AppliesTo::Me)
        {
            return Err(rejected(
                "needs_confirmation",
                "This would make an inferred memory the user's own. Ask the user whether it is \
                 true of them, then update it with origin \"user_confirmed\".",
            ));
        }

        let goal_status = self.goal_status(kind, before.goal_status(), edit, asked_by, notes)?;
        let structure = MemoryStructure::new(kind, topic, applies_to, origin, goal_status)?;
        Ok((Some(structure), plan))
    }

    /// R12: only the user closes a goal.
    fn goal_status(
        &self,
        kind: Kind,
        before: Option<GoalStatus>,
        edit: &StructuredEdit,
        asked_by: Option<Origin>,
        notes: &mut Vec<UpdateNote>,
    ) -> Result<Option<GoalStatus>> {
        let Some(raw) = &edit.goal_status else {
            // A memory that becomes a goal starts active; one that stops
            // being a goal has no status.
            return Ok(match kind {
                Kind::Goal => Some(before.unwrap_or(GoalStatus::Active)),
                _ => None,
            });
        };
        if kind != Kind::Goal {
            return Err(rejected(
                "invalid_value",
                format!("goal_status is only for goals, and this is a {kind}."),
            ));
        }
        let status: GoalStatus = raw.parse().map_err(invalid)?;
        if status.is_closed() {
            match asked_by {
                Some(Origin::UserSaid | Origin::UserConfirmed) => {}
                Some(Origin::Inferred | Origin::Document) => {
                    return Err(rejected(
                        "needs_confirmation",
                        format!(
                            "Only the user closes a goal. Ask them whether it is {status}, and \
                             on a yes update it again with origin \"user_confirmed\"."
                        ),
                    ));
                }
                None => {
                    return Err(rejected(
                        "missing_field",
                        "Closing a goal needs origin: user_said (the user said so) or \
                         user_confirmed (you asked and they said yes). Only the user closes a \
                         goal.",
                    ));
                }
            }
        }
        match (before, status) {
            (Some(old), new) if old == new => {}
            (_, GoalStatus::Active) => notes.push(UpdateNote::GoalReopened),
            (_, closed) => notes.push(UpdateNote::GoalClosed(closed)),
        }
        Ok(Some(status))
    }

    /// R10 and R12 on what the memory becomes: a plan keeps an end date, a
    /// goal takes none, and a new date is in the future.
    fn check_valid_until(
        &self,
        existing: &Memory,
        kind: Kind,
        edit: &StructuredEdit,
    ) -> Result<()> {
        let after = match edit.valid_until {
            Some(value) => value,
            None => existing.expires_at(),
        };
        match (kind, after) {
            (Kind::Plan, None) => Err(rejected(
                "missing_field",
                "A plan needs valid_until: when does it stop being true?",
            )),
            (Kind::Goal, Some(_)) => Err(rejected(
                "invalid_value",
                "A goal has no valid_until: it stays active until the user says it is achieved \
                 or abandoned.",
            )),
            (_, Some(at)) if edit.valid_until.is_some() => {
                self.rules.check_valid_until(Kind::Fact, Some(at))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::structured_memory_saver::{SaveResult, StructuredSave};
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::memory_repository::AuditOperation;

    struct Harness {
        fixture: Fixture,
        saver: Arc<StructuredMemorySaver>,
        updater: StructuredMemoryUpdater,
    }

    fn harness() -> Harness {
        let fixture = Fixture::new();
        let saver = Arc::new(fixture.structured_saver());
        let updater = StructuredMemoryUpdater::new(
            Arc::new(fixture.updater()),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&saver),
        );
        Harness {
            fixture,
            saver,
            updater,
        }
    }

    impl Harness {
        fn save(&self, content: &str, kind: &str, topic: &str, target: &str) -> Memory {
            let save = StructuredSave {
                content: content.to_string(),
                kind: Some(kind.to_string()),
                topic: Some(topic.to_string()),
                applies_to: Some(AppliesToInput {
                    target: Some(target.to_string()),
                    ..AppliesToInput::default()
                }),
                origin: Some("user_said".to_string()),
                ..StructuredSave::default()
            };
            match self
                .saver
                .execute(&self.fixture.alex, save, &[], "test")
                .unwrap()
            {
                SaveResult::Saved(saved) => saved.memory,
                other => panic!("expected a save, got {other:?}"),
            }
        }

        fn update(&self, id: MemoryId, edit: StructuredEdit) -> Result<UpdatedMemory> {
            self.updater.execute(&self.fixture.alex, id, edit, "test")
        }

        fn code(&self, id: MemoryId, edit: StructuredEdit) -> String {
            match self.update(id, edit) {
                Err(RaError::Rejected { code, .. }) => code,
                other => panic!("expected a rejection, got {other:?}"),
            }
        }
    }

    fn closing(status: &str, origin: Option<&str>) -> StructuredEdit {
        StructuredEdit {
            goal_status: Some(status.to_string()),
            origin: origin.map(str::to_string),
            ..StructuredEdit::default()
        }
    }

    #[test]
    fn e7_a_typo_is_amended_in_place_and_the_trail_keeps_the_change() {
        let harness = harness();
        let memory = harness.save(
            "The user prefers mokcs in unit tests",
            "preference",
            "coding",
            "me",
        );

        let updated = harness
            .update(
                memory.id(),
                StructuredEdit {
                    content: Some("The user prefers mocks in unit tests".to_string()),
                    ..StructuredEdit::default()
                },
            )
            .unwrap();

        assert_eq!(updated.memory.id(), memory.id());
        assert_eq!(
            updated.memory.content(),
            "The user prefers mocks in unit tests"
        );
        assert_eq!(updated.memory.structure(), memory.structure());
        assert!(
            harness
                .fixture
                .memories
                .audit_trail(&harness.fixture.alex, 10)
                .unwrap()
                .iter()
                .any(|entry| entry.memory_id == memory.id()
                    && entry.operation == AuditOperation::Update)
        );
    }

    #[test]
    fn an_amendment_is_held_to_the_save_rules() {
        let harness = harness();
        let memory = harness.save("The user prefers pnpm", "preference", "coding", "me");

        let secret = StructuredEdit {
            content: Some("The user's npm token is npm_8Fk2LmQ9zXv7Rt3Wq1Yp6Hs4Jd0Nb5Ce".into()),
            ..StructuredEdit::default()
        };
        assert_eq!(harness.code(memory.id(), secret), "secret");

        let bound = StructuredEdit {
            content: Some("The codebase uses pnpm".to_string()),
            ..StructuredEdit::default()
        };
        assert_eq!(harness.code(memory.id(), bound), "scope_contradiction");

        let to_project = StructuredEdit {
            applies_to: Some(AppliesToInput {
                target: Some("project".to_string()),
                project_name: Some("quill".to_string()),
                new_project: true,
            }),
            ..StructuredEdit::default()
        };
        assert_eq!(
            harness.code(memory.id(), to_project),
            "kind_target_mismatch"
        );
    }

    #[test]
    fn a_memory_can_be_rescoped_to_a_confirmed_new_project() {
        let harness = harness();
        let memory = harness.save("Never mock the database", "rule", "coding", "me");

        let updated = harness
            .update(
                memory.id(),
                StructuredEdit {
                    applies_to: Some(AppliesToInput {
                        target: Some("project".to_string()),
                        project_name: Some("quill".to_string()),
                        new_project: true,
                    }),
                    origin: Some("user_said".to_string()),
                    ..StructuredEdit::default()
                },
            )
            .unwrap();

        assert_eq!(
            updated
                .memory
                .structure()
                .applies_to()
                .project()
                .unwrap()
                .name,
            "quill"
        );
        assert!(updated.message().contains("Created the project quill"));
    }

    #[test]
    fn e20_only_the_user_closes_a_goal() {
        let harness = harness();
        let goal = harness.save("Run a half marathon in April", "goal", "health", "me");

        assert_eq!(
            harness.code(goal.id(), closing("achieved", Some("inferred"))),
            "needs_confirmation"
        );
        assert_eq!(
            harness.code(goal.id(), closing("achieved", None)),
            "missing_field"
        );

        let closed = harness
            .update(goal.id(), closing("achieved", Some("user_confirmed")))
            .unwrap();
        assert_eq!(
            closed.memory.structure().goal_status(),
            Some(GoalStatus::Achieved)
        );
        assert!(
            !closed
                .memory
                .is_active_at(crate::memories::application::test_doubles::now())
        );
        assert!(
            closed.message().contains("closed as achieved"),
            "{}",
            closed.message()
        );
        assert_eq!(
            closed.memory.structure().origin(),
            Origin::UserSaid,
            "closing a goal does not change where it came from"
        );

        let reopened = harness.update(goal.id(), closing("active", None)).unwrap();
        assert!(reopened.message().contains("open again"));
    }

    #[test]
    fn goal_status_is_only_for_goals_and_goals_take_no_end_date() {
        let harness = harness();
        let rule = harness.save("Never push to main", "rule", "coding", "me");
        assert_eq!(
            harness.code(rule.id(), closing("achieved", Some("user_said"))),
            "invalid_value"
        );

        let goal = harness.save("Learn Rust", "goal", "coding", "me");
        let dated = StructuredEdit {
            valid_until: Some(Some(
                crate::memories::application::test_doubles::now() + chrono::Duration::days(30),
            )),
            ..StructuredEdit::default()
        };
        assert_eq!(harness.code(goal.id(), dated), "invalid_value");
    }

    #[test]
    fn a_plan_keeps_its_end_date() {
        let harness = harness();
        let save = StructuredSave {
            content: "Dentist on 14 December at 10:00".to_string(),
            kind: Some("plan".to_string()),
            topic: Some("health".to_string()),
            applies_to: Some(AppliesToInput {
                target: Some("general".to_string()),
                ..AppliesToInput::default()
            }),
            origin: Some("user_said".to_string()),
            valid_until: Some(
                crate::memories::application::test_doubles::now() + chrono::Duration::days(30),
            ),
            ..StructuredSave::default()
        };
        let SaveResult::Saved(saved) = harness
            .saver
            .execute(&harness.fixture.alex, save, &[], "test")
            .unwrap()
        else {
            panic!("expected a save");
        };

        let cleared = StructuredEdit {
            valid_until: Some(None),
            ..StructuredEdit::default()
        };
        assert_eq!(harness.code(saved.memory.id(), cleared), "missing_field");
    }

    #[test]
    fn a_stated_structure_clears_the_review_flag() {
        let harness = harness();
        let legacy = harness.fixture.save(&harness.fixture.alex, "Prefers pnpm");
        assert!(legacy.structure().needs_review());

        let confirmed = harness
            .update(
                legacy.id(),
                StructuredEdit {
                    kind: Some("preference".to_string()),
                    origin: Some("user_confirmed".to_string()),
                    ..StructuredEdit::default()
                },
            )
            .unwrap();
        assert!(!confirmed.memory.structure().needs_review());
    }
}
