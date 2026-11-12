//! Saves a memory whose structure the caller states (ai-specs/memory-structure-plan.md
//! §3–§4): what sort of statement it is, what it is about, who it is true for,
//! and where it came from.
//!
//! Every rule that holds whatever the client lives here, so REST, MCP and the
//! web UI refuse the same things for the same reasons. A refusal is a
//! [`RaError::Rejected`] with a stable code and a message that says how to fix
//! it: the caller is usually an agent, and an agent acts on that well.
//!
//! Checks run before anything is written, and a new project is only created
//! once the save is known to go through, so a rejected save leaves no trace.

use super::direct_memory_saver::DirectMemorySaver;
use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::applies_to::{AppliesTo, ProjectRef, Target};
use crate::memories::domain::category::Category;
use crate::memories::domain::content_checks::{project_phrase, secret_in};
use crate::memories::domain::kind::{ALL_KINDS, Kind};
use crate::memories::domain::legacy_structure::category_for;
use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
use crate::memories::domain::memory_structure::{MemoryStructure, Origin};
use crate::memories::domain::project_registry::{ProjectRegistry, normalize_project_name};
use crate::memories::domain::topic::Topic;
use crate::memories::domain::topic_catalog::TopicCatalog;
use crate::shared::clock::Clock;
use crate::shared::error::{RaError, Result};
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// The project the caller is working in, as the local MCP server detected it
/// from its folder: the name to show, and the identifier to remember it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedProject {
    /// `quill`: the repository's name, or the folder's.
    pub name: String,
    /// `github.com/acme/quill`: the normalised remote, when there is one.
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AppliesToInput {
    pub target: Option<String>,
    pub project_name: Option<String>,
    /// The user confirmed that an unknown project is a new one.
    pub new_project: bool,
}

/// A save as the caller sent it. Strings rather than parsed values, so every
/// refusal comes from here, with the same codes and wording.
#[derive(Debug, Clone, Default)]
pub struct StructuredSave {
    pub content: String,
    pub kind: Option<String>,
    pub topic: Option<String>,
    pub applies_to: Option<AppliesToInput>,
    pub origin: Option<String>,
    pub subtopic: Option<String>,
    pub tags: Vec<String>,
    pub valid_until: Option<DateTime<Utc>>,
    /// The old category to also file it under. Optional, and only while
    /// categories still drive recall filters and the map (plan S4–S5);
    /// derived from kind and topic when absent.
    pub category: Option<String>,
    pub source: MemorySource,
    pub detected_project: Option<DetectedProject>,
}

/// Something about the save the caller should hear, beyond "saved".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveNote {
    /// `applies_to` was not given, so a default was used (plan D4).
    AppliesToDefaulted,
    /// A memory read from a document belongs to its project (R6), and a
    /// project's way of doing things is a convention, not a preference.
    FiledFromDocument,
    /// The user confirmed a new project, and it was created.
    ProjectCreated(String),
}

#[derive(Debug, Clone)]
pub struct SavedMemory {
    pub memory: Memory,
    pub notes: Vec<SaveNote>,
}

impl SavedMemory {
    /// The sentence the caller repeats to the user: how it was filed, and
    /// anything they should confirm.
    pub fn message(&self) -> String {
        let structure = self.memory.structure();
        let what = format!("{} {}", structure.topic(), structure.kind());
        let filed = match structure.applies_to() {
            AppliesTo::Me => format!("your personal {what} (applies to you everywhere)"),
            AppliesTo::Project(project) => format!("a {what} of {}", project.name),
            AppliesTo::General => format!("a general {what} (not tied to a project)"),
        };
        let mut message = format!("Saved as {filed}.");
        for note in &self.notes {
            message.push(' ');
            message.push_str(&match note {
                SaveNote::AppliesToDefaulted => format!(
                    "applies_to was not given, so it was filed as {}: confirm that with the user.",
                    structure.applies_to().target()
                ),
                SaveNote::FiledFromDocument => {
                    "It was read from a document, so it is filed under its project.".to_string()
                }
                SaveNote::ProjectCreated(name) => format!("Created the project {name}."),
            });
        }
        message
    }
}

pub struct StructuredMemorySaver {
    saver: Arc<DirectMemorySaver>,
    projects: Arc<dyn ProjectRegistry>,
    topics: Arc<dyn TopicCatalog>,
    clock: Arc<dyn Clock>,
}

pub(crate) fn rejected(code: &str, message: impl Into<String>) -> RaError {
    RaError::Rejected {
        code: code.to_string(),
        message: message.into(),
    }
}

/// How the project part of a save resolved, before anything is written.
enum ProjectPlan {
    Existing(ProjectRef),
    Create { name: String, aliases: Vec<String> },
}

impl StructuredMemorySaver {
    pub fn new(
        saver: Arc<DirectMemorySaver>,
        projects: Arc<dyn ProjectRegistry>,
        topics: Arc<dyn TopicCatalog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            saver,
            projects,
            topics,
            clock,
        }
    }

    /// Saves it, or refuses with the rule that stopped it. `extra_categories`
    /// are the user's own categories, for an explicit `category`.
    pub fn execute(
        &self,
        context: &UserContext,
        save: StructuredSave,
        extra_categories: &[String],
        actor: &str,
    ) -> Result<SavedMemory> {
        let content = save.content.trim().to_string();
        if content.is_empty() {
            return Err(rejected(
                "missing_field",
                "content is required: the memory, as one standalone sentence.",
            ));
        }

        // R1, R11: what sort of statement, about what, from where.
        let mut kind = parse_kind(save.kind.as_deref())?;
        let topic = self.resolve_topic(context, save.topic.as_deref())?;
        let origin = parse_origin(save.origin.as_deref())?;

        // R9 before anything else that could be worth keeping: a credential
        // is refused however the rest of the save looks.
        if let Some(found) = secret_in(&content) {
            return Err(rejected(
                "secret",
                format!(
                    "This looks like it contains {found}. Store how to reach the thing \
                     (host, user, which key file or vault entry), never the secret itself."
                ),
            ));
        }

        let mut notes = Vec::new();
        let input = save.applies_to.clone().unwrap_or_default();
        let stated = input
            .target
            .as_deref()
            .map(|raw| raw.parse::<Target>().map_err(invalid))
            .transpose()?;

        // R4: a project name belongs to a project.
        if input.project_name.is_some() && stated.is_some_and(|t| t != Target::Project) {
            return Err(rejected(
                "invalid_value",
                "applies_to.project_name is only for target \"project\".",
            ));
        }

        let detected_known = match &save.detected_project {
            Some(detected) => self.resolve_detected(context, detected)?,
            None => None,
        };

        // R6: a memory read from a document belongs to its project.
        let target = if origin == Origin::Document {
            if stated != Some(Target::Project) || kind == Kind::Preference {
                notes.push(SaveNote::FiledFromDocument);
            }
            if kind == Kind::Preference {
                kind = Kind::Convention;
            }
            Target::Project
        } else {
            match stated {
                Some(target) => target,
                // D4: the current project when one is known, else general;
                // never "me", which is the one wrong default that pollutes
                // the user's own preferences everywhere.
                None => {
                    notes.push(SaveNote::AppliesToDefaulted);
                    if detected_known.is_some() || input.project_name.is_some() {
                        Target::Project
                    } else {
                        Target::General
                    }
                }
            }
        };

        // R5: the user's own preferences are theirs to state.
        if origin == Origin::Inferred && target == Target::Me {
            return Err(rejected(
                "needs_confirmation",
                "This is about the user, and it was inferred. Ask the user whether it is true \
                 of them, then save it again with origin \"user_confirmed\".",
            ));
        }

        // R2: the kind must be allowed to apply there.
        if !kind.allows(target) {
            let defaulted = notes.contains(&SaveNote::AppliesToDefaulted);
            return Err(rejected(
                "kind_target_mismatch",
                mismatch_message(kind, target, defaulted),
            ));
        }

        // R7: the text must not tie itself to one project while claiming to
        // hold everywhere.
        if target != Target::Project
            && let Some(phrase) = project_phrase(&content)
        {
            return Err(rejected(
                "scope_contradiction",
                format!(
                    "This reads as being about one project (\"{phrase}\"), but it would be filed \
                     as {target}. Use applies_to.target \"project\", or reword it if it really \
                     holds everywhere."
                ),
            ));
        }

        // R10, R12: plans end on a date; goals end when the user says so.
        self.check_valid_until(kind, save.valid_until)?;

        // R3: a project memory needs a known project, or a confirmed new one.
        let project = if target == Target::Project {
            Some(self.plan_project(
                context,
                &input,
                save.detected_project.as_ref(),
                detected_known,
            )?)
        } else {
            None
        };

        let category = match save.category.as_deref() {
            Some(raw) => Category::parse_with_extras(raw, extra_categories).map_err(invalid)?,
            None => category_for(kind, &topic),
        };

        // Everything is checked: only now may a project be created.
        let applies_to = match (target, project) {
            (Target::Me, _) => AppliesTo::Me,
            (Target::General, _) => AppliesTo::General,
            (Target::Project, Some(ProjectPlan::Existing(project))) => AppliesTo::Project(project),
            (Target::Project, Some(ProjectPlan::Create { name, aliases })) => {
                let project = self.projects.create(context, &name, &aliases)?;
                notes.push(SaveNote::ProjectCreated(project.name.clone()));
                AppliesTo::Project(project)
            }
            (Target::Project, None) => unreachable!("a project target always plans a project"),
        };
        let structure = MemoryStructure::new(kind, topic, applies_to, origin, None)?;

        let memory = self.saver.execute_structured(
            context,
            NewMemory {
                content,
                category,
                subcategory: save.subtopic,
                tags: save.tags,
                entities: Vec::new(),
                confidence: confidence_of(origin),
                source: save.source,
                expires_at: save.valid_until,
            },
            structure,
            actor,
        )?;

        Ok(SavedMemory { memory, notes })
    }

    fn resolve_topic(&self, context: &UserContext, raw: Option<&str>) -> Result<Topic> {
        let known = || -> Result<String> {
            Ok(self
                .topics
                .all(context)?
                .iter()
                .map(|topic| topic.as_str().to_string())
                .collect::<Vec<_>>()
                .join(", "))
        };
        let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
            return Err(rejected(
                "missing_field",
                format!(
                    "topic is required: what the memory is about. One of {}.",
                    known()?
                ),
            ));
        };
        self.topics
            .find(context, raw)?
            .ok_or_else(|| match known() {
                Ok(list) => rejected(
                    "unknown_topic",
                    format!("Unknown topic {raw:?}; known: {list}."),
                ),
                Err(error) => error,
            })
    }

    /// The detected project, when the user already has it (by remote first,
    /// then by name). Learns the remote as an alias when only the name
    /// matched, so a later rename keeps resolving.
    fn resolve_detected(
        &self,
        context: &UserContext,
        detected: &DetectedProject,
    ) -> Result<Option<ProjectRef>> {
        if let Some(alias) = &detected.alias
            && let Some(project) = self.projects.resolve(context, alias)?
        {
            return Ok(Some(project));
        }
        let Ok(name) = normalize_project_name(&detected.name) else {
            return Ok(None);
        };
        let found = self.projects.resolve(context, &name)?;
        if let (Some(project), Some(alias)) = (&found, &detected.alias) {
            self.projects.add_alias(context, project, alias)?;
        }
        Ok(found)
    }

    fn plan_project(
        &self,
        context: &UserContext,
        input: &AppliesToInput,
        detected: Option<&DetectedProject>,
        detected_known: Option<ProjectRef>,
    ) -> Result<ProjectPlan> {
        let named = input
            .project_name
            .as_deref()
            .map(normalize_project_name)
            .transpose()?;

        let (name, aliases) = match (named, detected) {
            (Some(name), _) => {
                if let Some(project) = self.projects.resolve(context, &name)? {
                    return Ok(ProjectPlan::Existing(project));
                }
                // The detected remote is this project's only when the user
                // named the project the folder holds.
                let aliases = detected
                    .filter(|d| normalize_project_name(&d.name).ok().as_deref() == Some(&name))
                    .and_then(|d| d.alias.clone())
                    .into_iter()
                    .collect();
                (name, aliases)
            }
            (None, Some(detected)) => {
                if let Some(project) = detected_known {
                    return Ok(ProjectPlan::Existing(project));
                }
                let name = normalize_project_name(&detected.name)?;
                (name, detected.alias.clone().into_iter().collect())
            }
            (None, None) => {
                return Err(rejected(
                    "no_project",
                    "No project is known here. Ask the user which project this is about and \
                     name it in applies_to.project_name, or save it as \"general\".",
                ));
            }
        };

        if input.new_project {
            return Ok(ProjectPlan::Create { name, aliases });
        }
        let known = self.projects.names(context)?;
        let listed = if known.is_empty() {
            "none yet".to_string()
        } else {
            known.join(", ")
        };
        let source = aliases
            .first()
            .map(|alias| format!(" ({alias})"))
            .unwrap_or_default();
        Err(rejected(
            "unknown_project",
            format!(
                "Unknown project {name:?}{source}; known: {listed}. If it is a new project, \
                 confirm with the user and save again with applies_to.new_project = true. If it \
                 is a known one under another name, use that name."
            ),
        ))
    }

    fn check_valid_until(&self, kind: Kind, valid_until: Option<DateTime<Utc>>) -> Result<()> {
        match (kind, valid_until) {
            (Kind::Plan, None) => Err(rejected(
                "missing_field",
                "A plan needs valid_until: when does it stop being true?",
            )),
            (Kind::Goal, Some(_)) => Err(rejected(
                "invalid_value",
                "A goal has no valid_until: it stays active until the user says it is achieved \
                 or abandoned.",
            )),
            (_, Some(at)) if at <= self.clock.now() => Err(rejected(
                "invalid_value",
                "valid_until is already in the past.",
            )),
            _ => Ok(()),
        }
    }
}

fn invalid(error: RaError) -> RaError {
    match error {
        RaError::Validation(message) => rejected("invalid_value", message),
        other => other,
    }
}

fn parse_kind(raw: Option<&str>) -> Result<Kind> {
    let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
        let known: Vec<&str> = ALL_KINDS.iter().map(Kind::as_str).collect();
        return Err(rejected(
            "missing_field",
            format!(
                "kind is required: what sort of statement this is. One of {}.",
                known.join(", ")
            ),
        ));
    };
    raw.parse().map_err(invalid)
}

fn parse_origin(raw: Option<&str>) -> Result<Origin> {
    let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
        return Err(rejected(
            "missing_field",
            "origin is required: user_said (the user stated it), user_confirmed (the user said \
             yes to it), inferred (you noticed it) or document (read from a file).",
        ));
    };
    raw.parse().map_err(invalid)
}

/// The trust a memory earns from where it came from (plan §3.5). Agents'
/// self-reported confidence was never calibrated; the origin is the honest
/// signal.
fn confidence_of(origin: Origin) -> f32 {
    match origin {
        Origin::UserSaid | Origin::UserConfirmed => 1.0,
        Origin::Document => 0.9,
        Origin::Inferred => 0.7,
    }
}

/// The refusal for a kind that cannot apply there, naming what would fit.
fn mismatch_message(kind: Kind, target: Target, defaulted: bool) -> String {
    let fix = match (kind, target) {
        (Kind::Preference, Target::Project) => {
            "A project's way of doing things is a convention (or a rule). If it is the user's \
             own preference, use target \"me\"."
        }
        (Kind::Preference, Target::General) => {
            "A preference is the user's, so its target is \"me\". If it is about something \
             else, it is a fact or a rule."
        }
        (Kind::Convention, _) => {
            "A convention belongs to a project: use target \"project\". If it is the user's own \
             standard, it is a preference or a rule."
        }
        (Kind::Decision, Target::Me) => {
            "A decision applies to a project or to the user's world. The user's own standing \
             choice is a preference or a rule."
        }
        (Kind::Goal, Target::General) => {
            "A goal is the user's (target \"me\") or a project's (target \"project\")."
        }
        (Kind::Plan, Target::Me) => {
            "A plan applies to the user's world (target \"general\") or to a project."
        }
        _ => "Choose a target this kind allows.",
    };
    let allowed: Vec<&str> = kind.allowed_targets().iter().map(Target::as_str).collect();
    let default = if defaulted {
        format!(
            " applies_to was not given, so it defaulted to \"{target}\": set it (ask the user if \
             unsure)."
        )
    } else {
        String::new()
    };
    format!(
        "A {kind} cannot apply to \"{target}\"; it applies to {}. {fix}{default}",
        allowed.join(" or ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::memory_repository::MemoryRepository;

    struct Harness {
        fixture: Fixture,
        saver: StructuredMemorySaver,
        projects: Arc<dyn ProjectRegistry>,
    }

    fn harness() -> Harness {
        let fixture = Fixture::new();
        let projects = fixture.project_registry();
        let saver = StructuredMemorySaver::new(
            Arc::new(fixture.saver()),
            Arc::clone(&projects),
            fixture.topic_catalog(),
            crate::memories::application::test_doubles::fixed_clock(),
        );
        Harness {
            fixture,
            saver,
            projects,
        }
    }

    fn save(
        content: &str,
        kind: &str,
        topic: &str,
        target: Option<&str>,
        origin: &str,
    ) -> StructuredSave {
        StructuredSave {
            content: content.to_string(),
            kind: Some(kind.to_string()),
            topic: Some(topic.to_string()),
            applies_to: target.map(|target| AppliesToInput {
                target: Some(target.to_string()),
                ..AppliesToInput::default()
            }),
            origin: Some(origin.to_string()),
            ..StructuredSave::default()
        }
    }

    fn in_quill(mut save: StructuredSave) -> StructuredSave {
        save.detected_project = Some(DetectedProject {
            name: "quill".to_string(),
            alias: Some("github.com/acme/quill".to_string()),
        });
        save
    }

    impl Harness {
        fn run(&self, save: StructuredSave) -> Result<SavedMemory> {
            self.saver.execute(&self.fixture.alex, save, &[], "test")
        }

        fn code(&self, save: StructuredSave) -> String {
            match self.run(save) {
                Err(RaError::Rejected { code, .. }) => code,
                other => panic!("expected a rejection, got {other:?}"),
            }
        }

        fn quill(&self) -> ProjectRef {
            self.projects
                .create(
                    &self.fixture.alex,
                    "quill",
                    &["github.com/acme/quill".to_string()],
                )
                .unwrap()
        }
    }

    #[test]
    fn e1_a_personal_preference_said_inside_a_project_is_the_users() {
        let harness = harness();
        harness.quill();

        let saved = harness
            .run(in_quill(save(
                "The user prefers mocks over real dependencies in unit tests",
                "preference",
                "coding",
                Some("me"),
                "user_said",
            )))
            .unwrap();

        let structure = saved.memory.structure();
        assert_eq!(structure.applies_to(), &AppliesTo::Me);
        assert!(!structure.needs_review(), "stated, not derived");
        assert_eq!(saved.memory.category(), &Category::PreferenceCoding);
        assert_eq!(
            saved.message(),
            "Saved as your personal coding preference (applies to you everywhere)."
        );
    }

    #[test]
    fn e2_a_repo_rule_goes_to_the_detected_project() {
        let harness = harness();
        let quill = harness.quill();

        let saved = harness
            .run(in_quill(save(
                "In this repo we never mock the database",
                "rule",
                "coding",
                Some("project"),
                "user_said",
            )))
            .unwrap();

        assert_eq!(
            saved.memory.structure().applies_to().project(),
            Some(&quill)
        );
        assert_eq!(saved.message(), "Saved as a coding rule of quill.");
    }

    #[test]
    fn e3_an_inferred_preference_needs_the_user() {
        let harness = harness();
        let inferred = save(
            "Prefers small pull requests",
            "preference",
            "coding",
            Some("me"),
            "inferred",
        );
        assert_eq!(harness.code(inferred), "needs_confirmation");

        let confirmed = save(
            "Prefers small pull requests",
            "preference",
            "coding",
            Some("me"),
            "user_confirmed",
        );
        assert!(harness.run(confirmed).is_ok());
    }

    #[test]
    fn e4_a_document_preference_becomes_the_projects_convention() {
        let harness = harness();
        harness.quill();

        let saved = harness
            .run(in_quill(save(
                "Use cases are named by intent, with no Service suffix",
                "preference",
                "coding",
                Some("me"),
                "document",
            )))
            .unwrap();

        let structure = saved.memory.structure();
        assert_eq!(structure.kind(), Kind::Convention);
        assert_eq!(structure.applies_to().target(), Target::Project);
        assert_eq!(structure.origin(), Origin::Document);
        assert!(
            saved.message().contains("read from a document"),
            "{}",
            saved.message()
        );
    }

    #[test]
    fn e5_and_e21_text_bound_to_a_project_cannot_claim_to_be_the_users() {
        let harness = harness();
        for text in [
            "The codebase uses Python 3.12+",
            "Este proyecto usa Python 3.12",
        ] {
            assert_eq!(
                harness.code(save(text, "fact", "coding", Some("me"), "user_said")),
                "scope_contradiction",
                "{text}"
            );
        }
        harness.quill();
        let retried = in_quill(save(
            "The codebase uses Python 3.12+",
            "fact",
            "coding",
            Some("project"),
            "user_said",
        ));
        assert!(harness.run(retried).is_ok());
    }

    #[test]
    fn e8_a_file_location_outside_a_project_is_general() {
        let harness = harness();

        let saved = harness
            .run(save(
                "The NAS backups are in /mnt/tank/backups",
                "reference",
                "tech",
                Some("general"),
                "user_said",
            ))
            .unwrap();

        assert_eq!(saved.memory.structure().applies_to(), &AppliesTo::General);
        assert_eq!(
            saved.message(),
            "Saved as a general tech reference (not tied to a project)."
        );
    }

    #[test]
    fn e9_a_credential_is_refused() {
        let harness = harness();
        let secret = save(
            "The server password is hunter2!",
            "fact",
            "tech",
            Some("general"),
            "user_said",
        );
        assert_eq!(harness.code(secret), "secret");
        assert!(
            harness
                .fixture
                .memories
                .list(&harness.fixture.alex, true)
                .unwrap()
                .is_empty(),
            "nothing was stored"
        );
    }

    #[test]
    fn e10_a_plan_needs_an_end_and_keeps_it() {
        let harness = harness();
        let mut plan = save(
            "Dentist appointment on 14 December at 10:00",
            "plan",
            "health",
            Some("general"),
            "user_said",
        );
        assert_eq!(harness.code(plan.clone()), "missing_field");

        let end = crate::memories::application::test_doubles::fixed_clock().now()
            + chrono::Duration::days(30);
        plan.valid_until = Some(end);
        let saved = harness.run(plan).unwrap();
        assert_eq!(saved.memory.expires_at(), Some(end));
    }

    #[test]
    fn e11_a_project_memory_with_no_project_known_is_refused() {
        let harness = harness();
        let save = save(
            "The API uses JWT",
            "fact",
            "coding",
            Some("project"),
            "user_said",
        );
        assert_eq!(harness.code(save), "no_project");
    }

    #[test]
    fn e12_a_missing_applies_to_defaults_to_the_project_else_general_never_me() {
        let harness = harness();
        harness.quill();

        let general = harness
            .run(save(
                "Backups run at 03:00",
                "fact",
                "tech",
                None,
                "user_said",
            ))
            .unwrap();
        assert_eq!(general.memory.structure().applies_to(), &AppliesTo::General);
        assert!(general.message().contains("confirm that with the user"));

        let project = harness
            .run(in_quill(save(
                "Releases are tagged from main",
                "rule",
                "coding",
                None,
                "user_said",
            )))
            .unwrap();
        assert_eq!(
            project.memory.structure().applies_to().target(),
            Target::Project
        );

        // A preference cannot default anywhere: it is the user's to state.
        let preference = save("Prefers pnpm", "preference", "coding", None, "user_said");
        assert_eq!(harness.code(preference), "kind_target_mismatch");
    }

    #[test]
    fn e13_a_preference_for_a_project_is_pointed_at_convention() {
        let harness = harness();
        harness.quill();
        let error = harness
            .run(in_quill(save(
                "Uses Vitest",
                "preference",
                "coding",
                Some("project"),
                "user_said",
            )))
            .unwrap_err()
            .to_string();
        assert!(error.contains("kind_target_mismatch"), "{error}");
        assert!(error.contains("convention"), "{error}");
    }

    #[test]
    fn e19_a_goal_starts_active_and_takes_no_end_date() {
        let harness = harness();
        let goal = save(
            "Run a half marathon in April",
            "goal",
            "health",
            Some("me"),
            "user_said",
        );

        let saved = harness.run(goal.clone()).unwrap();
        assert_eq!(
            saved.memory.structure().goal_status(),
            Some(crate::memories::domain::memory_structure::GoalStatus::Active)
        );

        let mut dated = goal;
        dated.valid_until = Some(
            crate::memories::application::test_doubles::fixed_clock().now()
                + chrono::Duration::days(1),
        );
        assert_eq!(harness.code(dated), "invalid_value");
    }

    #[test]
    fn required_fields_are_named_when_missing() {
        let harness = harness();
        let mut missing = save(
            "Prefers pnpm",
            "preference",
            "coding",
            Some("me"),
            "user_said",
        );
        missing.kind = None;
        assert_eq!(harness.code(missing.clone()), "missing_field");
        missing.kind = Some("preference".to_string());
        missing.origin = None;
        assert_eq!(harness.code(missing.clone()), "missing_field");
        missing.origin = Some("user_said".to_string());
        missing.topic = Some("helth".to_string());
        assert_eq!(harness.code(missing), "unknown_topic");
    }

    #[test]
    fn an_unknown_project_is_created_only_on_confirmation_with_its_remote() {
        let harness = harness();
        let rule = in_quill(save(
            "Migrations are forward-only",
            "rule",
            "coding",
            Some("project"),
            "user_said",
        ));

        let error = harness.run(rule.clone()).unwrap_err().to_string();
        assert!(error.contains("unknown_project"), "{error}");
        assert!(error.contains("github.com/acme/quill"), "{error}");
        assert!(
            harness
                .projects
                .names(&harness.fixture.alex)
                .unwrap()
                .is_empty(),
            "nothing created"
        );

        let mut confirmed = rule;
        confirmed.applies_to.as_mut().unwrap().new_project = true;
        let saved = harness.run(confirmed).unwrap();
        assert!(
            saved.message().contains("Created the project quill."),
            "{}",
            saved.message()
        );
        assert!(
            harness
                .projects
                .resolve(&harness.fixture.alex, "github.com/acme/quill")
                .unwrap()
                .is_some(),
            "the remote is remembered"
        );
    }

    #[test]
    fn a_project_name_only_belongs_to_a_project_target() {
        let harness = harness();
        let mut save = save(
            "Prefers pnpm",
            "preference",
            "coding",
            Some("me"),
            "user_said",
        );
        save.applies_to.as_mut().unwrap().project_name = Some("quill".to_string());
        assert_eq!(harness.code(save), "invalid_value");
    }

    #[test]
    fn a_rejected_save_creates_no_project() {
        let harness = harness();
        let mut secret = in_quill(save(
            "token: 8f3kd02mf93kd02",
            "fact",
            "tech",
            Some("project"),
            "user_said",
        ));
        secret.applies_to.as_mut().unwrap().new_project = true;
        assert_eq!(harness.code(secret), "secret");
        assert!(
            harness
                .projects
                .names(&harness.fixture.alex)
                .unwrap()
                .is_empty()
        );
    }
}
