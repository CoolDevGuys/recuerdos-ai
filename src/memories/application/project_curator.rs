//! A user curating their projects (ai-specs/memory-structure-plan.md §6):
//! listing them, renaming one, and merging two.
//!
//! # Rename is one row; merge moves memories
//!
//! A memory stores its project's id, so a **rename** touches only the
//! project: every memory follows, and nothing is re-embedded or audited per
//! memory. The old name stays as an alias, so a checkout or a harness config
//! that still says it keeps resolving. Renamed from inside the repository
//! whose remote changed (E15), the new remote is learned too.
//!
//! A **merge** changes which project a memory applies to, which is an edit:
//! the memories move through `update_many`, audited each and all or nothing,
//! and then the registry folds the project away — its aliases and its name
//! now point at the one it was merged into. Merged memories are not checked
//! for contradictions here: two projects that turn out to be one may well
//! hold near-duplicates, and consolidation is what merges those.

use super::structured_memory_saver::DetectedProject;
use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::applies_to::ProjectRef;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::project_registry::{
    ProjectRecord, ProjectRegistry, normalize_project_name,
};
use crate::shared::clock::Clock;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::ProjectId;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

/// One project as `project_list` and `GET /v1/projects` show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectView {
    pub record: ProjectRecord,
    /// Active memories filed under it.
    pub memories: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renamed {
    pub project: ProjectView,
    pub previous: String,
    /// Identifiers learned with the rename: the remote of the folder the
    /// rename was asked from, when the new name is that folder's project.
    pub learned: Vec<String>,
}

impl Renamed {
    pub fn message(&self) -> String {
        let name = &self.project.record.project.name;
        if *name == self.previous {
            return match self.learned.as_slice() {
                [] => format!("The project is already called {name}."),
                learned => format!(
                    "The project is already called {name}; it is now also found by {}.",
                    learned.join(", ")
                ),
            };
        }
        let mut message = format!(
            "Renamed {} to {name}: its {} {} with it, and {} still finds it.",
            self.previous,
            self.project.memories,
            if self.project.memories == 1 {
                "memory moved"
            } else {
                "memories moved"
            },
            self.previous
        );
        if !self.learned.is_empty() {
            message.push_str(&format!(
                " It is now also found by {}.",
                self.learned.join(", ")
            ));
        }
        message
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub project: ProjectView,
    pub from: String,
    /// Memories that changed project, superseded ones included.
    pub moved: usize,
}

impl Merged {
    pub fn message(&self) -> String {
        format!(
            "Merged {} into {}: {} {} moved, and {} now finds {}.",
            self.from,
            self.project.record.project.name,
            self.moved,
            if self.moved == 1 {
                "memory"
            } else {
                "memories"
            },
            self.from,
            self.project.record.project.name,
        )
    }
}

pub struct ProjectCurator {
    projects: Arc<dyn ProjectRegistry>,
    memories: Arc<dyn MemoryRepository>,
    clock: Arc<dyn Clock>,
}

impl ProjectCurator {
    pub fn new(
        projects: Arc<dyn ProjectRegistry>,
        memories: Arc<dyn MemoryRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            projects,
            memories,
            clock,
        }
    }

    pub fn list(&self, context: &UserContext) -> Result<Vec<ProjectView>> {
        let counts = self.counts(context)?;
        Ok(self
            .projects
            .list(context)?
            .into_iter()
            .map(|record| ProjectView {
                memories: counts.get(&record.project.id).copied().unwrap_or(0),
                record,
            })
            .collect())
    }

    /// The project an identifier names: its id, its name, or one of its
    /// aliases (a former name, a remote, a folder name).
    pub fn find(&self, context: &UserContext, identifier: &str) -> Result<ProjectRef> {
        let raw = identifier.trim();
        if let Ok(id) = ProjectId::from_str(raw)
            && let Some(project) = self.projects.find(context, id)?
        {
            return Ok(project);
        }
        let lowered = raw.to_lowercase();
        if let Some(project) = self.projects.resolve(context, &lowered)? {
            return Ok(project);
        }
        let known = self.projects.names(context)?;
        Err(RaError::rejected(
            "unknown_project",
            format!(
                "Unknown project {raw:?}; known: {}.",
                if known.is_empty() {
                    "none yet".to_string()
                } else {
                    known.join(", ")
                }
            ),
        ))
    }

    pub fn view(&self, context: &UserContext, project: &ProjectRef) -> Result<ProjectView> {
        self.list(context)?
            .into_iter()
            .find(|view| view.record.project.id == project.id)
            .ok_or_else(|| RaError::NotFound(format!("project {}", project.name)))
    }

    /// Renames a project. `detected` is the folder the request came from,
    /// when the local MCP server knows it: if its project is the new name,
    /// its remote becomes an alias — the fix for a repository renamed on
    /// its host, whose new remote no project knew (E15).
    pub fn rename(
        &self,
        context: &UserContext,
        identifier: &str,
        raw_new_name: &str,
        detected: Option<&DetectedProject>,
    ) -> Result<Renamed> {
        let project = self.find(context, identifier)?;
        let new_name = normalize_project_name(raw_new_name)?;

        if new_name != project.name
            && let Some(other) = self.projects.resolve(context, &new_name)?
            && other.id != project.id
            && other.name == new_name
        {
            return Err(RaError::Conflict(format!(
                "Another project is already called {new_name}. If {} and {new_name} are the \
                 same project, merge {} into {new_name} instead.",
                project.name, project.name
            )));
        }

        let learned: Vec<String> = detected
            .filter(|detected| {
                normalize_project_name(&detected.name).ok().as_deref() == Some(&new_name)
            })
            .and_then(|detected| detected.alias.clone())
            .into_iter()
            .collect();
        // Only what is new is worth reporting.
        let known = self
            .projects
            .list(context)?
            .into_iter()
            .find(|record| record.project.id == project.id)
            .map(|record| record.aliases)
            .unwrap_or_default();
        let learned: Vec<String> = learned
            .into_iter()
            .filter(|alias| !known.contains(alias))
            .collect();

        let renamed = self
            .projects
            .rename(context, &project, &new_name, &learned)?;
        Ok(Renamed {
            project: self.view(context, &renamed)?,
            previous: project.name,
            learned,
        })
    }

    /// Folds `from` into `into`: its memories move, and its name and
    /// aliases find `into` from now on.
    pub fn merge(
        &self,
        context: &UserContext,
        from: &str,
        into: &str,
        actor: &str,
    ) -> Result<Merged> {
        let from = self.find(context, from)?;
        let into = self.find(context, into)?;
        if from.id == into.id {
            return Err(RaError::rejected(
                "invalid_value",
                format!("{} cannot be merged into itself.", from.name),
            ));
        }

        let now = self.clock.now();
        let moved: Vec<_> = self
            .memories
            .list(context, true)?
            .into_iter()
            .filter(|memory| {
                memory
                    .structure()
                    .applies_to()
                    .project()
                    .is_some_and(|project| project.id == from.id)
            })
            .map(|memory| memory.moved_to_project(&into, now))
            .collect();
        self.memories.update_many(
            context,
            &moved,
            actor,
            &format!("project {} merged into {}", from.name, into.name),
        )?;
        self.projects.absorb(context, &from, &into)?;

        Ok(Merged {
            project: self.view(context, &into)?,
            from: from.name,
            moved: moved.len(),
        })
    }

    fn counts(&self, context: &UserContext) -> Result<HashMap<ProjectId, usize>> {
        let now = self.clock.now();
        let mut counts = HashMap::new();
        for memory in self.memories.list(context, false)? {
            if !memory.is_active_at(now) {
                continue;
            }
            if let Some(project) = memory.structure().applies_to().project() {
                *counts.entry(project.id).or_default() += 1;
            }
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, fixed_clock, new_memory};
    use crate::memories::domain::applies_to::AppliesTo;
    use crate::memories::domain::kind::Kind;
    use crate::memories::domain::memory::Memory;
    use crate::memories::domain::memory_structure::{MemoryStructure, Origin};
    use crate::memories::domain::topic::Topic;

    struct Harness {
        fixture: Fixture,
        registry: Arc<dyn ProjectRegistry>,
        curator: ProjectCurator,
    }

    fn harness() -> Harness {
        let fixture = Fixture::new();
        let registry = fixture.project_registry();
        let curator = ProjectCurator::new(
            Arc::clone(&registry),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            fixed_clock(),
        );
        Harness {
            fixture,
            registry,
            curator,
        }
    }

    impl Harness {
        fn project(&self, name: &str, aliases: &[&str]) -> ProjectRef {
            let aliases: Vec<String> = aliases.iter().map(|a| a.to_string()).collect();
            self.registry
                .create(&self.fixture.alex, name, &aliases)
                .unwrap()
        }

        fn convention(&self, project: &ProjectRef, content: &str) -> Memory {
            let structure = MemoryStructure::new(
                Kind::Convention,
                Topic::Coding,
                AppliesTo::Project(project.clone()),
                Origin::UserSaid,
                None,
            )
            .unwrap();
            self.fixture
                .saver()
                .execute_structured(&self.fixture.alex, new_memory(content), structure, "test")
                .unwrap()
        }

        fn project_of(&self, memory: &Memory) -> ProjectRef {
            self.fixture
                .memories
                .find(&self.fixture.alex, memory.id())
                .unwrap()
                .unwrap()
                .structure()
                .applies_to()
                .project()
                .cloned()
                .unwrap()
        }
    }

    #[test]
    fn projects_are_listed_with_their_aliases_and_memory_counts() {
        let h = harness();
        let quill = h.project("quill", &["github.com/acme/quill"]);
        h.project("fern", &[]);
        h.convention(&quill, "Use cases are named by intent");

        let listed = h.curator.list(&h.fixture.alex).unwrap();

        let names: Vec<&str> = listed
            .iter()
            .map(|view| view.record.project.name.as_str())
            .collect();
        assert_eq!(names, ["fern", "quill"]);
        assert_eq!(listed[1].memories, 1);
        assert_eq!(listed[1].record.aliases, ["github.com/acme/quill"]);
        assert!(
            h.curator.list(&h.fixture.sam).unwrap().is_empty(),
            "one user's projects are not another's"
        );
    }

    #[test]
    fn a_rename_keeps_every_memory_and_the_old_name_still_finds_it() {
        let h = harness();
        let quill = h.project("quill", &["github.com/acme/quill"]);
        let memory = h.convention(&quill, "Use cases are named by intent");

        let renamed = h
            .curator
            .rename(&h.fixture.alex, "quill", "Quill-App", None)
            .unwrap();

        assert_eq!(renamed.project.record.project.name, "quill-app");
        assert_eq!(renamed.project.memories, 1);
        assert!(
            renamed.message().contains("quill still finds it"),
            "{}",
            renamed.message()
        );
        // The memory stores the id; the name is the project's (read through
        // SQL in the real store, tested end to end in tests/projects.rs).
        assert_eq!(h.project_of(&memory).id, quill.id);
        for old in ["quill", "github.com/acme/quill"] {
            assert_eq!(
                h.registry
                    .resolve(&h.fixture.alex, old)
                    .unwrap()
                    .unwrap()
                    .id,
                quill.id,
                "{old}"
            );
        }
    }

    #[test]
    fn renaming_from_the_renamed_repository_learns_its_new_remote() {
        // E15: the repository was renamed on its host, so the folder's
        // remote is one no project knows.
        let h = harness();
        let quill = h.project("quill", &["github.com/acme/quill"]);
        let here = DetectedProject {
            name: "quill-app".to_string(),
            alias: Some("github.com/acme/quill-app".to_string()),
        };

        let renamed = h
            .curator
            .rename(&h.fixture.alex, "quill", "quill-app", Some(&here))
            .unwrap();

        assert_eq!(renamed.learned, ["github.com/acme/quill-app"]);
        assert_eq!(
            h.registry
                .resolve(&h.fixture.alex, "github.com/acme/quill-app")
                .unwrap()
                .unwrap()
                .id,
            quill.id
        );
    }

    #[test]
    fn a_remote_is_not_learned_for_a_name_that_is_not_the_folders() {
        let h = harness();
        h.project("quill", &[]);
        let elsewhere = DetectedProject {
            name: "fern".to_string(),
            alias: Some("github.com/acme/fern".to_string()),
        };

        let renamed = h
            .curator
            .rename(&h.fixture.alex, "quill", "quill-app", Some(&elsewhere))
            .unwrap();

        assert!(renamed.learned.is_empty());
        assert!(
            h.registry
                .resolve(&h.fixture.alex, "github.com/acme/fern")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_rename_onto_another_projects_name_is_refused_with_merge_offered() {
        let h = harness();
        h.project("quill", &[]);
        h.project("fern", &[]);

        let error = h
            .curator
            .rename(&h.fixture.alex, "quill", "fern", None)
            .unwrap_err();

        assert!(
            matches!(&error, RaError::Conflict(message) if message.contains("merge")),
            "{error}"
        );
    }

    #[test]
    fn a_rename_onto_a_former_name_of_another_project_takes_the_name() {
        // The former name was an alias; a name is what the user calls a
        // project now, and wins.
        let h = harness();
        let old = h.project("quill", &[]);
        h.curator
            .rename(&h.fixture.alex, "quill", "quill-legacy", None)
            .unwrap();
        let fresh = h.project("fern", &[]);

        h.curator
            .rename(&h.fixture.alex, "fern", "quill", None)
            .unwrap();

        let found = h
            .registry
            .resolve(&h.fixture.alex, "quill")
            .unwrap()
            .unwrap();
        assert_eq!(found.id, fresh.id);
        assert_ne!(found.id, old.id);
    }

    #[test]
    fn an_invalid_or_unknown_name_is_refused() {
        let h = harness();
        h.project("quill", &[]);

        let error = h
            .curator
            .rename(&h.fixture.alex, "quill", "Quill App", None)
            .unwrap_err();
        assert!(
            matches!(&error, RaError::Rejected { code, .. } if code == "invalid_value"),
            "{error}"
        );

        let error = h
            .curator
            .rename(&h.fixture.alex, "quil", "quill-app", None)
            .unwrap_err();
        assert!(
            matches!(&error, RaError::Rejected { code, message, .. }
                if code == "unknown_project" && message.contains("known: quill")),
            "{error}"
        );
    }

    #[test]
    fn a_project_is_found_by_its_id() {
        let h = harness();
        let quill = h.project("quill", &[]);
        assert_eq!(
            h.curator
                .find(&h.fixture.alex, &quill.id.to_string())
                .unwrap(),
            quill
        );
    }

    #[test]
    fn a_merge_moves_the_memories_and_the_names_of_the_project_that_goes() {
        let h = harness();
        let quill = h.project("quill", &["github.com/acme/quill"]);
        let copy = h.project("me-quill", &["github.com/me/quill"]);
        let kept = h.convention(&quill, "Use cases are named by intent");
        let moved = h.convention(&copy, "Never mock the database");

        let merged = h
            .curator
            .merge(&h.fixture.alex, "me-quill", "quill", "test")
            .unwrap();

        assert_eq!(merged.moved, 1);
        assert_eq!(merged.project.memories, 2);
        assert!(merged.message().starts_with("Merged me-quill into quill"));
        assert_eq!(h.project_of(&moved).id, quill.id);
        assert_eq!(h.project_of(&kept).id, quill.id);
        for alias in ["me-quill", "github.com/me/quill"] {
            assert_eq!(
                h.registry
                    .resolve(&h.fixture.alex, alias)
                    .unwrap()
                    .unwrap()
                    .id,
                quill.id,
                "{alias}"
            );
        }
        assert_eq!(h.registry.names(&h.fixture.alex).unwrap(), ["quill"]);

        let trail = h.fixture.memories.audit_trail(&h.fixture.alex, 10).unwrap();
        assert!(
            trail
                .iter()
                .any(|entry| entry.memory_id == moved.id()
                    && entry.detail.contains("merged into quill")),
            "the move is audited"
        );
    }

    #[test]
    fn a_merged_memory_keeps_its_review_state() {
        let h = harness();
        let quill = h.project("quill", &[]);
        let fern = h.project("fern", &[]);
        let memory = h.convention(&fern, "Never mock the database");
        let flagged = memory
            .clone()
            .with_structure(memory.structure().clone().flagged_for_review(true));
        h.fixture
            .memories
            .update(&h.fixture.alex, &flagged, "test")
            .unwrap();

        h.curator
            .merge(&h.fixture.alex, "fern", "quill", "test")
            .unwrap();

        let stored = h
            .fixture
            .memories
            .find(&h.fixture.alex, memory.id())
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.structure().applies_to().project().unwrap().id,
            quill.id
        );
        assert!(stored.structure().needs_review());
    }

    #[test]
    fn a_project_cannot_be_merged_into_itself() {
        let h = harness();
        h.project("quill", &["github.com/acme/quill"]);
        let error = h
            .curator
            .merge(&h.fixture.alex, "github.com/acme/quill", "quill", "test")
            .unwrap_err();
        assert!(
            matches!(&error, RaError::Rejected { code, .. } if code == "invalid_value"),
            "{error}"
        );
    }
}
