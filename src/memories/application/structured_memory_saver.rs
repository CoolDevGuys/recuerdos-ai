//! Saves a memory whose structure the caller states (ai-specs/memory-structure-plan.md
//! §3–§5): what sort of statement it is, what it is about, who it is true for,
//! and where it came from — and refuses one that contradicts what is already
//! stored for the same scope.
//!
//! Every rule that holds whatever the client lives here, so REST, MCP and the
//! web UI refuse the same things for the same reasons. A refusal is a
//! [`RaError::Rejected`] with a stable code and a message that says how to fix
//! it: the caller is usually an agent, and an agent acts on that well.
//!
//! A save runs in three steps, so the one slow part — asking a model — holds
//! no database work open: [`check`](StructuredMemorySaver::check) applies the
//! rules and finds the nearest stored memories, the [`ConflictJudge`] (when a
//! model is configured) judges them, and [`commit`](StructuredMemorySaver::commit)
//! decides and writes. Nothing is written before the commit, and a new project
//! is only created once the save is known to go through, so a refused save
//! leaves no trace.

use super::direct_memory_saver::DirectMemorySaver;
use super::memory_recaller::MemoryRecaller;
use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::applies_to::{AppliesTo, ProjectRef, Target};
use crate::memories::domain::category::Category;
use crate::memories::domain::conflict_judge::{
    ConflictJudge, ConflictQuestion, Judgement, Neighbour, Verdict,
};
use crate::memories::domain::content_checks::{project_phrase, secret_in};
use crate::memories::domain::kind::{ALL_KINDS, Kind};
use crate::memories::domain::legacy_structure::category_for;
use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::memory_structure::{MemoryStructure, Origin};
use crate::memories::domain::project_registry::{
    ProjectRecord, ProjectRegistry, is_remote, normalize_project_name, remote_parts,
};
use crate::memories::domain::recall_query::{MAX_QUERY_LEN, RecallQuery};
use crate::memories::domain::topic::Topic;
use crate::memories::domain::topic_catalog::TopicCatalog;
use crate::shared::blocking::blocking;
use crate::shared::clock::Clock;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{MemoryId, ProjectId};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::sync::{Arc, OnceLock};

/// How many stored memories recall is asked for, before they are narrowed to
/// the save's scope. Wider than [`NEIGHBOURS_SHOWN`] because recall does not
/// filter by scope yet (plan S4), and the nearest memories are often another
/// scope's.
const NEIGHBOUR_SEARCH: i64 = 20;

/// How many neighbours are judged. Small for the reason the reconciler's
/// limit is: a memory ranked eighth is rarely the one being contradicted, and
/// every one shown is tokens on every save.
pub const NEIGHBOURS_SHOWN: usize = 5;

/// The relevance a stored memory needs to be shown to the model at all. The
/// floor `memory_forget` holds candidates to: above what an unrelated memory
/// scores, below the weakest real match.
pub const JUDGED_FLOOR: f32 = 0.3;

/// Without a model, the relevance at which a stored memory in the same scope
/// is reported as a *possible* conflict (plan §5.1, `judged: false`).
///
/// Similarity cannot tell a contradiction from a neighbour, so without a model
/// this is a guard against near-repeats, not a contradiction check. Measured
/// on the default embedder (2026-11-12), pairs that can both hold reach 0.71
/// ("the dentist appointment is on 14 December" / "the haircut appointment
/// is…"), 0.69 (two NAS folders) and 0.72 ("prefers Rust for CLIs" / "Go for
/// CLIs"), while contradictions range from 0.52 ("deploys on Fly.io" / "on
/// Hetzner" is 0.56, "prefers" / "avoids mocks" 0.62) to 0.78 (tabs / spaces,
/// the Spanish mocks pair). No floor separates the two groups, and a false
/// refusal costs the user more than a missed one, so the floor sits above
/// every compatible pair measured: it stops a save only for what nearly
/// repeats a stored memory. A model, when there is one, judges properly and
/// this is not used.
pub const UNJUDGED_FLOOR: f32 = 0.75;

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
    /// A stored memory this one replaces: the user changed their mind (plan
    /// §5.2). It must be theirs, active, and filed for the same scope; it is
    /// superseded, so it leaves recall and stays in history.
    pub replaces: Option<String>,
    /// Stored memories the user said this one does not replace — both hold.
    /// They are not reported as conflicts.
    pub coexists_with: Vec<String>,
}

/// Something about the save the caller should hear, beyond "saved".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveNote {
    /// `applies_to` was not given, so a default was used (plan D4).
    AppliesToDefaulted,
    /// A memory read from a document belongs to its project (R6), and a
    /// project's way of doing things is a convention, not a preference.
    FiledFromDocument,
    /// The user confirmed a new project, and it was created. `instead_of`
    /// is the name it would have had, taken by another repository's
    /// project (plan §6, collisions).
    ProjectCreated {
        name: String,
        instead_of: Option<String>,
    },
    /// It replaced this stored memory, which is kept in history.
    Replaced { id: MemoryId, content: String },
    /// A project memory that contradicts one of the user's own: an exception
    /// for this project, which is normal and not a conflict (§5.1).
    Overrides { kind: Kind, content: String },
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
                SaveNote::ProjectCreated {
                    name,
                    instead_of: None,
                } => format!("Created the project {name}."),
                SaveNote::ProjectCreated {
                    name,
                    instead_of: Some(taken),
                } => {
                    format!("Created the project {name}: {taken} is another repository's project.")
                }
                SaveNote::Replaced { content, .. } => {
                    format!("It replaces \"{content}\", which is kept in history.")
                }
                SaveNote::Overrides { kind, content } => {
                    format!("It overrides your personal {kind} \"{content}\" in this project.")
                }
            });
        }
        message
    }
}

/// What a save did: stored it, or found the store already knew it.
#[derive(Debug, Clone)]
pub enum SaveResult {
    Saved(SavedMemory),
    /// A stored memory in the same scope already says this; nothing was
    /// written. Only a judge (or an identical sentence) decides this.
    AlreadyKnown {
        memory: Memory,
        reason: String,
    },
}

impl SaveResult {
    pub fn message(&self) -> String {
        match self {
            SaveResult::Saved(saved) => saved.message(),
            SaveResult::AlreadyKnown { memory, reason } => {
                let reason = if reason.is_empty() {
                    String::new()
                } else {
                    format!(" ({reason})")
                };
                format!(
                    "Already known: \"{}\" (id {}) says this{reason}, so nothing was saved.",
                    memory.content(),
                    memory.id()
                )
            }
        }
    }
}

pub struct StructuredMemorySaver {
    saver: Arc<DirectMemorySaver>,
    memories: Arc<dyn MemoryRepository>,
    recaller: Arc<MemoryRecaller>,
    projects: Arc<dyn ProjectRegistry>,
    topics: Arc<dyn TopicCatalog>,
    clock: Arc<dyn Clock>,
    /// The model that judges contradictions, set once the understanding
    /// context is wired (it owns the model). Unset means no model: the
    /// stricter model-free rule applies.
    judge: OnceLock<Arc<dyn ConflictJudge>>,
}

pub(crate) fn rejected(code: &str, message: impl Into<String>) -> RaError {
    RaError::rejected(code, message)
}

/// How the project part of a save resolved, before anything is written.
#[derive(Debug, Clone)]
pub(crate) enum ProjectPlan {
    Existing(ProjectRef),
    Create {
        name: String,
        aliases: Vec<String>,
        /// The detected name, when another project already has it.
        instead_of: Option<String>,
    },
}

/// A save that passed every rule that needs no judgement, with what a judge
/// should look at. Nothing has been written.
#[derive(Debug, Clone)]
pub struct CheckedSave {
    new: NewMemory,
    kind: Kind,
    topic: Topic,
    origin: Origin,
    target: Target,
    project: Option<ProjectPlan>,
    notes: Vec<SaveNote>,
    replaces: Option<Memory>,
    coexists_with: Vec<MemoryId>,
    question: ConflictQuestion,
}

impl CheckedSave {
    /// What a judge is asked, or `None` when there is nothing to ask.
    pub fn question(&self) -> Option<&ConflictQuestion> {
        (!self.question.is_empty()).then_some(&self.question)
    }
}

impl StructuredMemorySaver {
    pub fn new(
        saver: Arc<DirectMemorySaver>,
        memories: Arc<dyn MemoryRepository>,
        recaller: Arc<MemoryRecaller>,
        projects: Arc<dyn ProjectRegistry>,
        topics: Arc<dyn TopicCatalog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            saver,
            memories,
            recaller,
            projects,
            topics,
            clock,
            judge: OnceLock::new(),
        }
    }

    /// Gives the saver a model to judge contradictions with. Called once, by
    /// the wiring, when a provider is configured; a second call is ignored.
    pub fn set_judge(&self, judge: Arc<dyn ConflictJudge>) {
        if self.judge.set(judge).is_err() {
            tracing::warn!("the conflict judge was already set; keeping the first");
        }
    }

    /// Saves it, or refuses with the rule that stopped it: the whole save,
    /// with the model's judgement when there is a model. `extra_categories`
    /// are the user's own categories, for an explicit `category`.
    pub async fn save(
        self: &Arc<Self>,
        context: &UserContext,
        save: StructuredSave,
        extra_categories: Vec<String>,
        actor: &str,
    ) -> Result<SaveResult> {
        let this = Arc::clone(self);
        let check_context = context.clone();
        let checked = blocking(move || this.check(&check_context, save, &extra_categories)).await?;

        let judgement = match (self.judge.get(), checked.question()) {
            (Some(judge), Some(question)) => match judge.judge(question).await {
                Ok(judgement) => Some(judgement),
                Err(error) => {
                    // A model outage must not cost the user their memory:
                    // the model-free rule still guards against the clearest
                    // repeats and contradictions.
                    tracing::warn!(
                        %error,
                        "could not judge a save for conflicts; using the model-free rule"
                    );
                    None
                }
            },
            _ => None,
        };

        let this = Arc::clone(self);
        let (context, actor) = (context.clone(), actor.to_string());
        blocking(move || this.commit(&context, checked, judgement, &actor)).await
    }

    /// The whole save without a model's judgement, in one synchronous call.
    #[cfg(test)]
    pub fn execute(
        &self,
        context: &UserContext,
        save: StructuredSave,
        extra_categories: &[String],
        actor: &str,
    ) -> Result<SaveResult> {
        let checked = self.check(context, save, extra_categories)?;
        self.commit(context, checked, None, actor)
    }

    /// Applies every rule that needs no judgement and finds the stored
    /// memories a judge should compare it with. Writes nothing.
    pub fn check(
        &self,
        context: &UserContext,
        save: StructuredSave,
        extra_categories: &[String],
    ) -> Result<CheckedSave> {
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
        refuse_secrets(&content)?;

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

        // R7, the phrase list: the text must not tie itself to one project
        // while claiming to hold everywhere.
        refuse_project_phrases(&content, target)?;

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

        // The scope as it will be filed. A project still to be created has
        // no id yet; a fresh one stands in, which matches no stored memory —
        // correctly, since a new project has none.
        let applies_to = match (target, &project) {
            (Target::Me, _) => AppliesTo::Me,
            (Target::General, _) => AppliesTo::General,
            (Target::Project, Some(ProjectPlan::Existing(project))) => {
                AppliesTo::Project(project.clone())
            }
            (Target::Project, Some(ProjectPlan::Create { name, .. })) => {
                AppliesTo::Project(ProjectRef {
                    id: ProjectId::new(),
                    name: name.clone(),
                })
            }
            (Target::Project, None) => unreachable!("a project target always plans a project"),
        };

        let replaces = match save.replaces.as_deref() {
            Some(raw) => Some(self.replaced_memory(context, raw, &applies_to)?),
            None => None,
        };
        let coexists_with = save
            .coexists_with
            .iter()
            .map(|raw| parse_memory_id("coexists_with", raw))
            .collect::<Result<Vec<_>>>()?;

        let neighbours = self.neighbours(context, &content, &applies_to, replaces.as_ref());

        Ok(CheckedSave {
            question: ConflictQuestion {
                content: content.clone(),
                kind,
                topic: topic.clone(),
                applies_to,
                neighbours,
                check_scope: target != Target::Project,
            },
            new: NewMemory {
                content,
                category,
                subcategory: save.subtopic,
                tags: save.tags,
                entities: Vec::new(),
                confidence: confidence_of(origin),
                source: save.source,
                expires_at: save.valid_until,
            },
            kind,
            topic,
            origin,
            target,
            project,
            notes,
            replaces,
            coexists_with,
        })
    }

    /// Decides what a checked save does, given the judge's verdicts (`None`:
    /// no model, or it failed), and writes it.
    pub fn commit(
        &self,
        context: &UserContext,
        checked: CheckedSave,
        judgement: Option<Judgement>,
        actor: &str,
    ) -> Result<SaveResult> {
        let CheckedSave {
            new,
            kind,
            topic,
            origin,
            target,
            project,
            mut notes,
            replaces,
            coexists_with,
            question,
        } = checked;

        let decision = match &judgement {
            Some(judgement) => judged(&question, judgement, &coexists_with)?,
            None => unjudged(&question, &coexists_with),
        };
        let overrides = match decision {
            Decision::AlreadyKnown { memory, reason } => {
                return Ok(SaveResult::AlreadyKnown {
                    memory: *memory,
                    reason,
                });
            }
            Decision::Conflicts { conflicts, judged } => {
                return Err(conflict_error(&question.applies_to, &conflicts, judged));
            }
            Decision::Store { overrides } => overrides,
        };
        notes.extend(overrides.into_iter().map(|memory| SaveNote::Overrides {
            kind: memory.structure().kind(),
            content: memory.content().to_string(),
        }));

        // Everything is decided: only now may a project be created.
        let applies_to = match (target, project) {
            (Target::Me, _) => AppliesTo::Me,
            (Target::General, _) => AppliesTo::General,
            (Target::Project, Some(ProjectPlan::Existing(project))) => AppliesTo::Project(project),
            (
                Target::Project,
                Some(ProjectPlan::Create {
                    name,
                    aliases,
                    instead_of,
                }),
            ) => {
                let project = self.projects.create(context, &name, &aliases)?;
                notes.push(SaveNote::ProjectCreated {
                    name: project.name.clone(),
                    instead_of,
                });
                AppliesTo::Project(project)
            }
            (Target::Project, None) => unreachable!("a project target always plans a project"),
        };
        let structure = MemoryStructure::new(kind, topic, applies_to, origin, None)?;

        let memory = self
            .saver
            .execute_structured(context, new, structure, actor)?;

        if let Some(old) = replaces {
            self.memories.supersede(
                context,
                old.id(),
                memory.id(),
                actor,
                "replaced: the user changed their mind",
            )?;
            notes.push(SaveNote::Replaced {
                id: old.id(),
                content: old.content().to_string(),
            });
        }

        Ok(SaveResult::Saved(SavedMemory { memory, notes }))
    }

    /// The project a plan names, creating it when the plan says so — the
    /// last step before a write, once every rule has passed.
    pub(crate) fn settle_project(
        &self,
        context: &UserContext,
        plan: ProjectPlan,
    ) -> Result<(ProjectRef, bool)> {
        match plan {
            ProjectPlan::Existing(project) => Ok((project, false)),
            ProjectPlan::Create { name, aliases, .. } => {
                Ok((self.projects.create(context, &name, &aliases)?, true))
            }
        }
    }

    /// The memory a save replaces, checked: the user's, active, and filed for
    /// the same scope. Replacing across scopes is a rescope, which is an edit.
    fn replaced_memory(
        &self,
        context: &UserContext,
        raw: &str,
        applies_to: &AppliesTo,
    ) -> Result<Memory> {
        let id = parse_memory_id("replaces", raw)?;
        let memory = self.memories.find(context, id)?.ok_or_else(|| {
            rejected(
                "invalid_value",
                format!("replaces names {id}, which is not one of your memories."),
            )
        })?;
        if !memory.is_active_at(self.clock.now()) {
            return Err(rejected(
                "invalid_value",
                format!(
                    "replaces names {id}, which is no longer active (already replaced, expired \
                     or closed)."
                ),
            ));
        }
        let stored = memory.structure().applies_to();
        if !stored.same_scope(applies_to) {
            return Err(rejected(
                "invalid_value",
                format!(
                    "replaces names a memory filed for {}, but this one would be filed for {}. \
                     A memory replaces one in the same scope; to move a memory to another \
                     scope, update it (memory_update).",
                    stored.label(),
                    applies_to.label()
                ),
            ));
        }
        Ok(memory)
    }

    /// The stored memories nearest the new one that share its scope — and,
    /// for a project memory, the user's own, which it may override. Best
    /// effort: a recall failure means no conflict check, not a lost save.
    fn neighbours(
        &self,
        context: &UserContext,
        content: &str,
        applies_to: &AppliesTo,
        replaces: Option<&Memory>,
    ) -> Vec<Neighbour> {
        let text: String = content.chars().take(MAX_QUERY_LEN).collect();
        let query = match RecallQuery::new(&text, NEIGHBOUR_SEARCH) {
            Ok(query) => query.with_min_relevance(JUDGED_FLOOR),
            Err(error) => {
                tracing::warn!(%error, "could not build a conflict-check query");
                return Vec::new();
            }
        };
        let found = match self.recaller.execute_untracked(context, &query) {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "could not look up similar memories; saving without a conflict check"
                );
                return Vec::new();
            }
        };

        let project_save = applies_to.target() == Target::Project;
        found
            .into_iter()
            .filter(|scored| replaces.is_none_or(|old| old.id() != scored.memory.id()))
            .filter_map(|scored| {
                let stored = scored.memory.structure().applies_to();
                let personal = project_save && stored.target() == Target::Me;
                let kept = stored.same_scope(applies_to) || personal;
                kept.then_some(Neighbour {
                    relevance: scored.relevance,
                    personal,
                    memory: scored.memory,
                })
            })
            .take(NEIGHBOURS_SHOWN)
            .collect()
    }

    pub(crate) fn resolve_topic(&self, context: &UserContext, raw: Option<&str>) -> Result<Topic> {
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
    pub(crate) fn resolve_detected(
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
        let Some(project) = self.projects.resolve(context, &name)? else {
            return Ok(None);
        };
        if let Some(alias) = &detected.alias {
            // Another repository with the same name (`acme/quill` and
            // `me/quill`): the project already has a remote, and it is not
            // this one. A different project, not this one's alias.
            if self.remotes_of(context, &project)?.next().is_some() {
                return Ok(None);
            }
            self.projects.add_alias(context, &project, alias)?;
        }
        Ok(Some(project))
    }

    /// The git remotes a project is known by.
    fn remotes_of(
        &self,
        context: &UserContext,
        project: &ProjectRef,
    ) -> Result<impl Iterator<Item = String>> {
        Ok(self
            .projects
            .list(context)?
            .into_iter()
            .find(|record| record.project.id == project.id)
            .map(|record| record.aliases)
            .unwrap_or_default()
            .into_iter()
            .filter(|alias| is_remote(alias)))
    }

    /// A name for a detected repository whose own name another project
    /// has: the owner's, then a number. `me/quill` → `me-quill`.
    fn distinct_name(
        &self,
        context: &UserContext,
        name: &str,
        alias: Option<&str>,
    ) -> Result<String> {
        let owner = alias
            .and_then(remote_parts)
            .and_then(|(owner, _)| owner.rsplit('/').next())
            .and_then(|owner| normalize_project_name(&format!("{owner}-{name}")).ok());
        let base = owner.unwrap_or_else(|| name.to_string());
        for n in 1.. {
            let candidate = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            if candidate != name
                && normalize_project_name(&candidate).is_ok()
                && self.projects.resolve(context, &candidate)?.is_none()
            {
                return Ok(candidate);
            }
            if n > 100 {
                break;
            }
        }
        Err(RaError::Conflict(format!(
            "could not find a free name for another project called {name}; name it in              applies_to.project_name"
        )))
    }

    /// Projects the detected repository may be a rename of: those whose
    /// remote has the same owner, narrowed to the ones sharing the first
    /// word of the name when that leaves any (E15).
    fn renamed_from(
        &self,
        context: &UserContext,
        name: &str,
        alias: Option<&str>,
    ) -> Result<Vec<ProjectRecord>> {
        let Some((owner, _)) = alias.and_then(remote_parts) else {
            return Ok(vec![]);
        };
        let same_owner: Vec<ProjectRecord> = self
            .projects
            .list(context)?
            .into_iter()
            .filter(|record| {
                record
                    .aliases
                    .iter()
                    .filter_map(|alias| remote_parts(alias))
                    .any(|(other, _)| other == owner)
            })
            .collect();
        let first_word = |text: &str| {
            text.split(['-', '_', '.'])
                .next()
                .unwrap_or_default()
                .to_string()
        };
        let word = first_word(name);
        let close: Vec<ProjectRecord> = same_owner
            .iter()
            .filter(|record| first_word(&record.project.name) == word)
            .cloned()
            .collect();
        Ok(if close.is_empty() { same_owner } else { close })
    }

    pub(crate) fn plan_project(
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
                if let Some(taken) = self.projects.resolve(context, &name)? {
                    return self.plan_collision(context, input, detected, &name, &taken);
                }
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
            return Ok(ProjectPlan::Create {
                name,
                aliases,
                instead_of: None,
            });
        }
        let listed = self.known_list(context)?;
        let source = aliases
            .first()
            .map(|alias| format!(" ({alias})"))
            .unwrap_or_default();
        let renamed_from = match detected {
            Some(detected) if input.project_name.is_none() => {
                self.renamed_from(context, &name, detected.alias.as_deref())?
            }
            _ => vec![],
        };
        let hint = match renamed_from.as_slice() {
            [] => "If it is a known one under another name, use that name.".to_string(),
            [one] => format!(
                "Is it a rename of {old} ({remotes})? If so, ask the user and use \
                 project_rename ({old} → {name}): its memories follow, and this remote is \
                 learned.",
                old = one.project.name,
                remotes = one
                    .aliases
                    .iter()
                    .filter(|alias| is_remote(alias))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            several => format!(
                "Is it a rename of one of {}? If so, ask the user which, and use \
                 project_rename: its memories follow, and this remote is learned.",
                several
                    .iter()
                    .map(|record| record.project.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        Err(RaError::Rejected {
            code: "unknown_project".to_string(),
            message: format!(
                "Unknown project {name:?}{source}; known: {listed}. {hint} If it is a new \
                 project, confirm with the user and save again with applies_to.new_project = \
                 true."
            ),
            details: Some(json!({
                "project": name,
                "remote": aliases.first(),
                "rename_of": renamed_from
                    .iter()
                    .map(|record| record.project.name.clone())
                    .collect::<Vec<_>>(),
            })),
        })
    }

    /// The detected repository's name is another project's (plan §6,
    /// collisions): it becomes a project of its own under a distinct name,
    /// once the user confirms it is new.
    fn plan_collision(
        &self,
        context: &UserContext,
        input: &AppliesToInput,
        detected: &DetectedProject,
        name: &str,
        taken: &ProjectRef,
    ) -> Result<ProjectPlan> {
        let distinct = self.distinct_name(context, name, detected.alias.as_deref())?;
        let aliases: Vec<String> = detected.alias.clone().into_iter().collect();
        if input.new_project {
            return Ok(ProjectPlan::Create {
                name: distinct,
                aliases,
                instead_of: Some(taken.name.clone()),
            });
        }
        let theirs: Vec<String> = self.remotes_of(context, taken)?.collect();
        let source = aliases
            .first()
            .map(|alias| format!(" ({alias})"))
            .unwrap_or_default();
        Err(RaError::Rejected {
            code: "unknown_project".to_string(),
            message: format!(
                "This repository{source} is not a known project: the project {taken} is \
                 {theirs}. If it is a new project, confirm with the user and save again with \
                 applies_to.new_project = true; it will be called {distinct}. If it is {taken}, \
                 moved to a new remote, ask the user and use project_rename ({taken} → \
                 {taken}) from here, which adds this remote.",
                taken = taken.name,
                theirs = if theirs.is_empty() {
                    "another repository".to_string()
                } else {
                    theirs.join(", ")
                },
            ),
            details: Some(json!({
                "project": distinct,
                "remote": aliases.first(),
                "collides_with": taken.name,
            })),
        })
    }

    fn known_list(&self, context: &UserContext) -> Result<String> {
        let known = self.projects.names(context)?;
        Ok(if known.is_empty() {
            "none yet".to_string()
        } else {
            known.join(", ")
        })
    }

    pub(crate) fn check_valid_until(
        &self,
        kind: Kind,
        valid_until: Option<DateTime<Utc>>,
    ) -> Result<()> {
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

/// What the conflict check decided.
enum Decision {
    Store {
        overrides: Vec<Memory>,
    },
    AlreadyKnown {
        memory: Box<Memory>,
        reason: String,
    },
    Conflicts {
        conflicts: Vec<Conflict>,
        judged: bool,
    },
}

struct Conflict {
    memory: Memory,
    /// The judge's reason, when there was a judge.
    reason: Option<String>,
}

/// The decision with a model's judgement (plan §5.1).
fn judged(
    question: &ConflictQuestion,
    judgement: &Judgement,
    coexists_with: &[MemoryId],
) -> Result<Decision> {
    // R7, the model layer: it catches what the phrase list cannot (other
    // languages, paraphrase), in the call already made for conflicts.
    if question.check_scope
        && let Some(reason) = &judgement.scope_mismatch
    {
        return Err(rejected(
            "scope_contradiction",
            format!(
                "This reads as being about one project ({reason}), but it would be filed as \
                 {}. Use applies_to.target \"project\", or reword it if it really holds \
                 everywhere.",
                question.applies_to.target()
            ),
        ));
    }

    let verdict_of = |id: MemoryId| {
        judgement
            .verdicts
            .iter()
            .find(|verdict| verdict.memory_id == id)
    };
    let mut conflicts = Vec::new();
    let mut overrides = Vec::new();
    for neighbour in &question.neighbours {
        let id = neighbour.memory.id();
        let Some(verdict) = verdict_of(id) else {
            continue;
        };
        if coexists_with.contains(&id) {
            continue;
        }
        match (verdict.verdict, neighbour.personal) {
            (Verdict::Same, false) => {
                return Ok(Decision::AlreadyKnown {
                    memory: Box::new(neighbour.memory.clone()),
                    reason: verdict.reason.clone(),
                });
            }
            (Verdict::Contradicts, false) => conflicts.push(Conflict {
                memory: neighbour.memory.clone(),
                reason: Some(verdict.reason.clone()).filter(|reason| !reason.is_empty()),
            }),
            (Verdict::Contradicts, true) => overrides.push(neighbour.memory.clone()),
            (Verdict::Same | Verdict::Unrelated, _) => {}
        }
    }
    Ok(if conflicts.is_empty() {
        Decision::Store { overrides }
    } else {
        Decision::Conflicts {
            conflicts,
            judged: true,
        }
    })
}

/// The decision without a model. It cannot tell a contradiction from a
/// neighbour, so it stops only for what is nearly the same sentence, and says
/// it could not judge (plan §5.1, `judged: false`).
fn unjudged(question: &ConflictQuestion, coexists_with: &[MemoryId]) -> Decision {
    let wanted = normalised(&question.content);
    let mut conflicts = Vec::new();
    for neighbour in &question.neighbours {
        if neighbour.personal || coexists_with.contains(&neighbour.memory.id()) {
            continue;
        }
        if normalised(neighbour.memory.content()) == wanted {
            return Decision::AlreadyKnown {
                memory: Box::new(neighbour.memory.clone()),
                reason: "the same sentence is already stored".to_string(),
            };
        }
        if neighbour.relevance >= UNJUDGED_FLOOR {
            conflicts.push(Conflict {
                memory: neighbour.memory.clone(),
                reason: None,
            });
        }
    }
    if conflicts.is_empty() {
        Decision::Store {
            overrides: Vec::new(),
        }
    } else {
        Decision::Conflicts {
            conflicts,
            judged: false,
        }
    }
}

/// Lowercase words, so "Prefers pnpm." and "prefers pnpm" are one sentence.
fn normalised(content: &str) -> String {
    content
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `conflict` refusal: which stored memories, and the three ways out.
fn conflict_error(scope: &AppliesTo, conflicts: &[Conflict], judged: bool) -> RaError {
    let listed = conflicts
        .iter()
        .map(|conflict| {
            let reason = conflict
                .reason
                .as_deref()
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default();
            format!(
                "\"{}\" (id {}){reason}",
                conflict.memory.content(),
                conflict.memory.id()
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let ids = conflicts
        .iter()
        .map(|conflict| format!("\"{}\"", conflict.memory.id()))
        .collect::<Vec<_>>()
        .join(", ");
    let first = conflicts[0].memory.id();
    let opening = if judged {
        format!(
            "It contradicts what is stored for {}: {listed}.",
            scope.label()
        )
    } else {
        format!(
            "It is very close to what is stored for {}: {listed}. Without a language model the \
             server cannot tell whether it repeats or contradicts it.",
            scope.label()
        )
    };
    let message = format!(
        "{opening} Ask the user which holds. If they changed their mind, save it again with \
         replaces: \"{first}\" (the old one is kept in history). If the stored one is wrong, \
         correct it with memory_update. If both hold, save it again with coexists_with: \
         [{ids}]."
    );
    let details = json!({
        "judged": judged,
        "conflicts": conflicts
            .iter()
            .map(|conflict| json!({
                "id": conflict.memory.id().to_string(),
                "content": conflict.memory.content(),
                "kind": conflict.memory.structure().kind().as_str(),
                "applies_to": conflict.memory.structure().applies_to().label(),
                "reason": conflict.reason,
            }))
            .collect::<Vec<_>>(),
    });
    RaError::Rejected {
        code: "conflict".to_string(),
        message,
        details: Some(details),
    }
}

/// R9: a credential is never stored.
pub(crate) fn refuse_secrets(content: &str) -> Result<()> {
    match secret_in(content) {
        Some(found) => Err(rejected(
            "secret",
            format!(
                "This looks like it contains {found}. Store how to reach the thing (host, user, \
                 which key file or vault entry), never the secret itself."
            ),
        )),
        None => Ok(()),
    }
}

/// R7, the phrase list: text bound to one codebase cannot be filed as the
/// user's or as general.
pub(crate) fn refuse_project_phrases(content: &str, target: Target) -> Result<()> {
    if target == Target::Project {
        return Ok(());
    }
    match project_phrase(content) {
        Some(phrase) => Err(rejected(
            "scope_contradiction",
            format!(
                "This reads as being about one project (\"{phrase}\"), but it would be filed as \
                 {target}. Use applies_to.target \"project\", or reword it if it really holds \
                 everywhere."
            ),
        )),
        None => Ok(()),
    }
}

fn parse_memory_id(field: &str, raw: &str) -> Result<MemoryId> {
    raw.trim().parse().map_err(|_| {
        rejected(
            "invalid_value",
            format!("{field}: {raw:?} is not a memory id."),
        )
    })
}

pub(crate) fn invalid(error: RaError) -> RaError {
    match error {
        RaError::Validation(message) => rejected("invalid_value", message),
        other => other,
    }
}

pub(crate) fn parse_kind(raw: Option<&str>) -> Result<Kind> {
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

pub(crate) fn parse_origin(raw: Option<&str>) -> Result<Origin> {
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
pub(crate) fn mismatch_message(kind: Kind, target: Target, defaulted: bool) -> String {
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
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::new(fixture.recaller()),
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
            match self.saver.execute(&self.fixture.alex, save, &[], "test")? {
                SaveResult::Saved(saved) => Ok(saved),
                SaveResult::AlreadyKnown { memory, .. } => {
                    panic!(
                        "expected a save, but it was already known: {}",
                        memory.content()
                    )
                }
            }
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

    fn detected_in(save: StructuredSave, name: &str, remote: &str) -> StructuredSave {
        StructuredSave {
            detected_project: Some(DetectedProject {
                name: name.to_string(),
                alias: Some(remote.to_string()),
            }),
            ..save
        }
    }

    fn project_rule(content: &str) -> StructuredSave {
        save(content, "rule", "coding", Some("project"), "user_said")
    }

    fn details(error: RaError) -> (String, serde_json::Value) {
        match error {
            RaError::Rejected {
                message, details, ..
            } => (message, details.unwrap_or_default()),
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn another_repository_with_the_same_name_is_another_project() {
        // Plan §6, collisions: acme/quill and me/quill are not one project.
        let harness = harness();
        let quill = harness.quill();
        let fork = detected_in(
            project_rule("Releases are cut from the fork's main"),
            "quill",
            "github.com/me/quill",
        );

        let (message, details) = details(harness.run(fork.clone()).unwrap_err());
        assert!(message.contains("github.com/acme/quill"), "{message}");
        assert!(message.contains("called me-quill"), "{message}");
        assert_eq!(details["collides_with"], "quill");
        assert_eq!(
            harness
                .projects
                .resolve(&harness.fixture.alex, "github.com/me/quill")
                .unwrap(),
            None,
            "the fork's remote was not filed under quill"
        );

        let mut confirmed = fork;
        confirmed.applies_to.as_mut().unwrap().new_project = true;
        let saved = harness.run(confirmed.clone()).unwrap();
        assert!(
            saved
                .message()
                .contains("Created the project me-quill: quill is another repository's project."),
            "{}",
            saved.message()
        );
        let fork_project = harness
            .projects
            .resolve(&harness.fixture.alex, "github.com/me/quill")
            .unwrap()
            .unwrap();
        assert_eq!(fork_project.name, "me-quill");
        assert_ne!(fork_project.id, quill.id);

        // From then on the fork resolves by its remote, with nothing to ask.
        let mut again = confirmed;
        again.content = "The fork tracks upstream weekly".to_string();
        again.applies_to.as_mut().unwrap().new_project = false;
        let saved = harness.run(again).unwrap();
        assert_eq!(
            saved.memory.structure().applies_to().project().unwrap().id,
            fork_project.id
        );
    }

    #[test]
    fn a_name_without_a_remote_learns_the_first_one_it_is_seen_with() {
        // A project created by name (the web UI) is the folder's, not a
        // collision.
        let harness = harness();
        let quill = harness
            .projects
            .create(&harness.fixture.alex, "quill", &[])
            .unwrap();

        let saved = harness
            .run(in_quill(project_rule("Migrations are forward-only")))
            .unwrap();

        assert_eq!(
            saved.memory.structure().applies_to().project().unwrap().id,
            quill.id
        );
        assert_eq!(
            harness
                .projects
                .resolve(&harness.fixture.alex, "github.com/acme/quill")
                .unwrap()
                .unwrap()
                .id,
            quill.id
        );
    }

    #[test]
    fn e15_a_renamed_repository_is_offered_as_a_rename() {
        let harness = harness();
        harness.quill();
        harness
            .projects
            .create(
                &harness.fixture.alex,
                "fern",
                &["github.com/other/fern".to_string()],
            )
            .unwrap();
        let moved = detected_in(
            project_rule("Migrations are forward-only"),
            "quill-app",
            "github.com/acme/quill-app",
        );

        let (message, details) = details(harness.run(moved).unwrap_err());

        assert!(
            message.contains("Is it a rename of quill (github.com/acme/quill)?"),
            "{message}"
        );
        assert!(
            message.contains("project_rename (quill → quill-app)"),
            "{message}"
        );
        assert_eq!(details["rename_of"], serde_json::json!(["quill"]));
        assert_eq!(details["remote"], "github.com/acme/quill-app");
    }

    #[test]
    fn no_rename_is_offered_for_a_repository_of_another_owner() {
        let harness = harness();
        harness.quill();
        let elsewhere = detected_in(
            project_rule("Migrations are forward-only"),
            "ledger",
            "github.com/someone/ledger",
        );

        let (message, details) = details(harness.run(elsewhere).unwrap_err());

        assert!(!message.contains("rename"), "{message}");
        assert_eq!(details["rename_of"], serde_json::json!([]));
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

    // ---- S2: conflicts, replacing (plan §5) ----

    use crate::memories::domain::conflict_judge::NeighbourVerdict;
    use std::sync::Mutex;

    fn mine(content: &str) -> StructuredSave {
        save(content, "preference", "coding", Some("me"), "user_said")
    }

    impl Harness {
        fn stored(&self, save: StructuredSave) -> Memory {
            self.run(save).unwrap().memory
        }

        /// Checks a save and commits it with the verdict a judge would give
        /// on each neighbour.
        fn judged(
            &self,
            save: StructuredSave,
            verdict: impl Fn(&Neighbour) -> Verdict,
            scope_mismatch: Option<&str>,
        ) -> Result<SaveResult> {
            let checked = self.saver.check(&self.fixture.alex, save, &[])?;
            let judgement = Judgement {
                verdicts: checked
                    .question
                    .neighbours
                    .iter()
                    .map(|neighbour| NeighbourVerdict {
                        memory_id: neighbour.memory.id(),
                        verdict: verdict(neighbour),
                        reason: "the scripted judge said so".to_string(),
                    })
                    .collect(),
                scope_mismatch: scope_mismatch.map(str::to_string),
            };
            self.saver
                .commit(&self.fixture.alex, checked, Some(judgement), "test")
        }
    }

    fn rejection(result: Result<SaveResult>) -> (String, String, Option<serde_json::Value>) {
        match result {
            Err(RaError::Rejected {
                code,
                message,
                details,
            }) => (code, message, details),
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn e6_a_contradiction_in_the_same_scope_is_refused_naming_the_stored_memory() {
        let harness = harness();
        let old = harness.stored(mine("The user prefers mocks in unit tests"));

        let (code, message, details) = rejection(harness.judged(
            mine("The user avoids mocks in unit tests"),
            |_| Verdict::Contradicts,
            None,
        ));

        assert_eq!(code, "conflict");
        assert!(message.contains("The user prefers mocks"), "{message}");
        assert!(
            message.contains(&format!("replaces: \"{}\"", old.id())),
            "{message}"
        );
        assert!(message.contains("memory_update"), "{message}");
        assert!(message.contains("coexists_with"), "{message}");
        let details = details.expect("a conflict carries its details");
        assert_eq!(details["judged"], true);
        assert_eq!(details["conflicts"][0]["id"], old.id().to_string());
    }

    #[test]
    fn e6_replacing_supersedes_the_old_memory_and_keeps_it_in_history() {
        let harness = harness();
        let old = harness.stored(mine("The user prefers mocks in unit tests"));

        let mut replacing = mine("The user avoids mocks in unit tests");
        replacing.replaces = Some(old.id().to_string());
        let result = harness
            .judged(replacing, |_| Verdict::Contradicts, None)
            .unwrap();
        let SaveResult::Saved(saved) = result else {
            panic!("expected a save");
        };

        assert!(
            saved
                .message()
                .contains("It replaces \"The user prefers mocks in unit tests\""),
            "{}",
            saved.message()
        );
        let kept = harness
            .fixture
            .memories
            .find(&harness.fixture.alex, old.id())
            .unwrap()
            .expect("kept in history");
        assert_eq!(kept.superseded_by(), Some(saved.memory.id()));
    }

    #[test]
    fn replaces_must_name_an_active_memory_in_the_same_scope() {
        let harness = harness();
        let general = harness.stored(save(
            "The NAS backups are in /mnt/tank/backups",
            "reference",
            "tech",
            Some("general"),
            "user_said",
        ));

        let mut across = mine("The user keeps backups on the NAS");
        across.replaces = Some(general.id().to_string());
        let (code, message, _) = rejection(harness.saver.execute(
            &harness.fixture.alex,
            across,
            &[],
            "test",
        ));
        assert_eq!(code, "invalid_value");
        assert!(message.contains("memory_update"), "{message}");

        let mut unknown = mine("Prefers pnpm");
        unknown.replaces = Some(MemoryId::new().to_string());
        assert_eq!(harness.code(unknown), "invalid_value");

        let mut garbled = mine("Prefers pnpm");
        garbled.replaces = Some("mem_123".to_string());
        assert_eq!(harness.code(garbled), "invalid_value");
    }

    #[test]
    fn what_the_store_already_says_is_not_stored_twice() {
        let harness = harness();
        let old = harness.stored(mine("The user prefers pnpm"));

        let result = harness
            .judged(
                mine("pnpm is the user's package manager"),
                |_| Verdict::Same,
                None,
            )
            .unwrap();

        let SaveResult::AlreadyKnown { memory, .. } = &result else {
            panic!("expected already known, got {result:?}");
        };
        assert_eq!(memory.id(), old.id());
        assert!(
            result.message().starts_with("Already known"),
            "{}",
            result.message()
        );
        assert_eq!(
            harness
                .fixture
                .memories
                .list(&harness.fixture.alex, true)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn coexists_with_lets_both_stand() {
        let harness = harness();
        let old = harness.stored(mine("The user prefers mocks in unit tests"));

        let mut both = mine("The user avoids mocks in integration tests");
        both.coexists_with = vec![old.id().to_string()];
        let result = harness
            .judged(both, |_| Verdict::Contradicts, None)
            .unwrap();

        assert!(matches!(result, SaveResult::Saved(_)), "{result:?}");
    }

    #[test]
    fn e2_a_project_exception_to_the_users_own_rule_is_noted_not_refused() {
        let harness = harness();
        harness.quill();
        harness.stored(mine("The user prefers mocks in unit tests"));

        let result = harness
            .judged(
                in_quill(save(
                    "In this repo we never mock the database",
                    "rule",
                    "coding",
                    Some("project"),
                    "user_said",
                )),
                |neighbour| {
                    assert!(neighbour.personal, "the user's own memory is the neighbour");
                    Verdict::Contradicts
                },
                None,
            )
            .unwrap();

        let SaveResult::Saved(saved) = result else {
            panic!("expected a save");
        };
        assert!(
            saved.message().contains(
                "It overrides your personal preference \"The user prefers mocks in unit tests\" \
                 in this project."
            ),
            "{}",
            saved.message()
        );
    }

    #[test]
    fn another_projects_memories_are_never_neighbours() {
        let harness = harness();
        harness.quill();
        harness
            .projects
            .create(&harness.fixture.alex, "fern", &[])
            .unwrap();
        let mut fern = save(
            "Fern never mocks the database",
            "rule",
            "coding",
            Some("project"),
            "user_said",
        );
        fern.applies_to.as_mut().unwrap().project_name = Some("fern".to_string());
        harness.stored(fern);

        let checked = harness
            .saver
            .check(
                &harness.fixture.alex,
                in_quill(save(
                    "Quill mocks the database in unit tests",
                    "rule",
                    "coding",
                    Some("project"),
                    "user_said",
                )),
                &[],
            )
            .unwrap();
        assert!(checked.question.neighbours.is_empty());
        assert!(checked.question().is_none(), "nothing to ask a model");
    }

    #[test]
    fn e22_the_models_scope_verdict_refuses_what_the_phrase_list_misses() {
        let harness = harness();
        let (code, message, _) = rejection(harness.judged(
            mine("Here the convention is to name handlers after commands"),
            |_| Verdict::Unrelated,
            Some("\"here the convention\" is about one codebase"),
        ));

        assert_eq!(code, "scope_contradiction");
        assert!(message.contains("here the convention"), "{message}");
    }

    fn neighbour(content: &str, relevance: f32) -> Neighbour {
        Neighbour {
            memory: Fixture::new().save(&Fixture::new().alex, content),
            relevance,
            personal: false,
        }
    }

    fn question(content: &str, neighbours: Vec<Neighbour>) -> ConflictQuestion {
        ConflictQuestion {
            content: content.to_string(),
            kind: Kind::Preference,
            topic: Topic::Coding,
            applies_to: AppliesTo::Me,
            neighbours,
            check_scope: true,
        }
    }

    #[test]
    fn without_a_model_only_a_near_identical_memory_stops_a_save() {
        let near = question(
            "The user prefers spaces for indentation",
            vec![neighbour("The user prefers tabs for indentation", 0.8)],
        );
        let Decision::Conflicts { judged, conflicts } = unjudged(&near, &[]) else {
            panic!("a near-identical memory should be a possible conflict");
        };
        assert!(!judged);
        assert_eq!(conflicts.len(), 1);

        let related = question(
            "The user prefers Vitest",
            vec![neighbour("The user prefers pnpm", 0.7)],
        );
        assert!(matches!(unjudged(&related, &[]), Decision::Store { .. }));

        let same = question(
            "the user prefers pnpm.",
            vec![neighbour("The user prefers pnpm", 0.95)],
        );
        assert!(matches!(
            unjudged(&same, &[]),
            Decision::AlreadyKnown { .. }
        ));
    }

    #[test]
    fn without_a_model_the_refusal_says_it_could_not_judge() {
        let error = conflict_error(
            &AppliesTo::Me,
            &[Conflict {
                memory: Fixture::new().save(&Fixture::new().alex, "The user prefers mocks"),
                reason: None,
            }],
            false,
        );
        let RaError::Rejected {
            message, details, ..
        } = error
        else {
            panic!("expected a rejection");
        };
        assert!(message.contains("Without a language model"), "{message}");
        assert_eq!(details.unwrap()["judged"], false);
    }

    /// A judge that records what it was asked and answers from a script.
    struct ScriptedJudge {
        asked: Mutex<Vec<ConflictQuestion>>,
        answer: std::result::Result<Verdict, String>,
    }

    #[async_trait::async_trait]
    impl ConflictJudge for ScriptedJudge {
        async fn judge(&self, question: &ConflictQuestion) -> Result<Judgement> {
            self.asked.lock().unwrap().push(question.clone());
            match &self.answer {
                Ok(verdict) => Ok(Judgement {
                    verdicts: question
                        .neighbours
                        .iter()
                        .map(|neighbour| NeighbourVerdict {
                            memory_id: neighbour.memory.id(),
                            verdict: *verdict,
                            reason: String::new(),
                        })
                        .collect(),
                    scope_mismatch: None,
                }),
                Err(message) => Err(RaError::Internal(message.clone())),
            }
        }
    }

    fn judged_harness(
        answer: std::result::Result<Verdict, String>,
    ) -> (Arc<StructuredMemorySaver>, Fixture, Arc<ScriptedJudge>) {
        let fixture = Fixture::new();
        let saver = Arc::new(fixture.structured_saver());
        let judge = Arc::new(ScriptedJudge {
            asked: Mutex::new(Vec::new()),
            answer,
        });
        saver.set_judge(Arc::clone(&judge) as Arc<dyn ConflictJudge>);
        (saver, fixture, judge)
    }

    #[tokio::test]
    async fn a_save_asks_the_judge_once_and_acts_on_it() {
        let (saver, fixture, judge) = judged_harness(Ok(Verdict::Contradicts));
        saver
            .save(
                &fixture.alex,
                mine("The user prefers mocks in unit tests"),
                vec![],
                "test",
            )
            .await
            .unwrap();
        assert_eq!(
            judge.asked.lock().unwrap().len(),
            1,
            "a personal memory is judged for its scope even with no neighbours"
        );

        let refused = saver
            .save(
                &fixture.alex,
                mine("The user avoids mocks in unit tests"),
                vec![],
                "test",
            )
            .await;
        assert!(
            matches!(&refused, Err(RaError::Rejected { code, .. }) if code == "conflict"),
            "{refused:?}"
        );
        assert_eq!(judge.asked.lock().unwrap().len(), 2, "one call per save");
    }

    #[tokio::test]
    async fn a_project_memory_with_nothing_near_it_costs_no_model_call() {
        let (saver, fixture, judge) = judged_harness(Ok(Verdict::Unrelated));
        let mut rule = save(
            "Never mock the database",
            "rule",
            "coding",
            Some("project"),
            "user_said",
        );
        rule.applies_to.as_mut().unwrap().project_name = Some("quill".to_string());
        rule.applies_to.as_mut().unwrap().new_project = true;

        saver
            .save(&fixture.alex, rule, vec![], "test")
            .await
            .unwrap();
        assert!(judge.asked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_judge_that_fails_does_not_cost_the_user_their_memory() {
        let (saver, fixture, _) = judged_harness(Err("provider down".to_string()));
        let result = saver
            .save(&fixture.alex, mine("The user prefers pnpm"), vec![], "test")
            .await
            .unwrap();
        assert!(matches!(result, SaveResult::Saved(_)));
    }
}
