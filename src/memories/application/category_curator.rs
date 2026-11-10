//! A user curating their own taxonomy (ai-specs/web-ui-plan.md §4.2):
//! listing categories with counts, creating, relabelling, renaming,
//! merging and deleting them.
//!
//! # What touches vectors
//!
//! - A **label** — of a built-in or a custom category — is display-only.
//!   The enriched vector header is written from the category's *id*
//!   (`Category::label` of the id), so a label never re-embeds anything and
//!   built-in vectors stay comparable across users.
//! - **Renaming** a custom category's id, **merging** it into another and
//!   **deleting** it with `move_to` all re-file its members. That is a memory
//!   edit like any other, done through [`MemoryBulkEditor::apply`]: audited
//!   per memory, all or nothing, and re-embedded where the embedded text
//!   changed.
//!
//! Members are the non-forgotten memories filed under the category,
//! superseded ones included: they are still the user's history, and
//! `include_superseded` reads would otherwise show them under a category
//! that no longer exists. Re-embedding runs in the request; the plan's
//! background-job variant for very large categories is not needed at the
//! store sizes the map handles (see §9.4).

use crate::identity::domain::user_context::UserContext;
use crate::memories::application::memory_bulk_editor::{BulkOutcome, MemoryBulkEditor};
use crate::memories::domain::category::Category;
use crate::memories::domain::category_catalog::{CatalogEntry, CategoryCatalog, normalise_label};
use crate::memories::domain::memory::{Memory, MemoryEdit};
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::shared::error::{RaError, Result};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One category as the categories manager lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryView {
    pub category: Category,
    /// What to show: the user's label, else the default wording.
    pub label: String,
    /// Whether `label` is the user's own rather than the default.
    pub relabelled: bool,
    /// Active memories filed here.
    pub count: usize,
    /// In the user's catalog (or built in). False for a category still on
    /// a stored memory but not in the catalog — shown so it can be moved.
    pub known: bool,
}

pub struct CategoryCurator {
    catalog: Arc<dyn CategoryCatalog>,
    memories: Arc<dyn MemoryRepository>,
    editor: Arc<MemoryBulkEditor>,
}

impl CategoryCurator {
    pub fn new(
        catalog: Arc<dyn CategoryCatalog>,
        memories: Arc<dyn MemoryRepository>,
        editor: Arc<MemoryBulkEditor>,
    ) -> Self {
        Self {
            catalog,
            memories,
            editor,
        }
    }

    /// Built-ins in taxonomy order, then custom ones by id, then any
    /// category still on an active memory but missing from the catalog.
    pub fn list(&self, context: &UserContext) -> Result<Vec<CategoryView>> {
        let entries = self.catalog.all(context)?;
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for memory in self.memories.list(context, false)? {
            *counts
                .entry(memory.category().as_str().to_string())
                .or_default() += 1;
        }

        let mut views: Vec<CategoryView> = entries
            .into_iter()
            .map(|entry| CategoryView {
                count: counts.remove(entry.category.as_str()).unwrap_or(0),
                label: entry.display_label(),
                relabelled: entry.label.is_some(),
                category: entry.category,
                known: true,
            })
            .collect();
        views.extend(counts.into_iter().map(|(id, count)| {
            let category = Category::from_stored(&id);
            CategoryView {
                label: category.label(),
                relabelled: false,
                category,
                count,
                known: false,
            }
        }));
        Ok(views)
    }

    /// Creates a custom category.
    pub fn create(
        &self,
        context: &UserContext,
        raw_id: &str,
        label: Option<&str>,
    ) -> Result<CatalogEntry> {
        let entry = CatalogEntry {
            category: Category::custom(raw_id)?,
            label: normalise_label(label)?,
        };
        self.catalog.create(context, &entry)?;
        Ok(entry)
    }

    /// Sets or clears the display label of any of the user's categories.
    pub fn relabel(
        &self,
        context: &UserContext,
        raw_id: &str,
        label: Option<&str>,
    ) -> Result<CatalogEntry> {
        let category = self.known(context, raw_id)?;
        let label = normalise_label(label)?;
        self.catalog
            .set_label(context, &category, label.as_deref())?;
        Ok(CatalogEntry { category, label })
    }

    /// Gives a custom category a new id, re-filing its members.
    pub fn rename(
        &self,
        context: &UserContext,
        raw_from: &str,
        raw_to: &str,
        actor: &str,
    ) -> Result<(CatalogEntry, BulkOutcome)> {
        let from = self.custom_known(context, raw_from)?;
        let to = Category::custom(raw_to)?;
        if to == from {
            return Err(RaError::Validation(format!("{from} already has that id")));
        }
        let label = self
            .catalog
            .entries(context)?
            .into_iter()
            .find(|e| e.category == from)
            .and_then(|e| e.label);
        let entry = CatalogEntry {
            category: to.clone(),
            label,
        };

        // The new id first, so that at no point does a member name a
        // category its owner cannot see; the old one goes last.
        self.catalog.create(context, &entry)?;
        let moved = match self.move_members(
            context,
            &from,
            &to,
            actor,
            &format!("category renamed {from} → {to}"),
        ) {
            Ok(moved) => moved,
            Err(error) => {
                if let Err(undo) = self.catalog.remove(context, &to) {
                    tracing::error!(%undo, category = %to, "could not undo a failed rename");
                }
                return Err(error);
            }
        };
        self.catalog.remove(context, &from)?;
        Ok((entry, moved))
    }

    /// Moves every member of custom category `raw_from` into `raw_into` (a
    /// built-in or another of the user's categories), then deletes it.
    pub fn merge(
        &self,
        context: &UserContext,
        raw_from: &str,
        raw_into: &str,
        actor: &str,
    ) -> Result<BulkOutcome> {
        let from = self.custom_known(context, raw_from)?;
        let into = self.known(context, raw_into)?;
        if into == from {
            return Err(RaError::Validation(
                "a category cannot be merged into itself".to_string(),
            ));
        }
        let moved = self.move_members(
            context,
            &from,
            &into,
            actor,
            &format!("category {from} merged into {into}"),
        )?;
        self.catalog.remove(context, &from)?;
        Ok(moved)
    }

    /// Deletes a custom category. One that still has members is refused
    /// with `Conflict` unless `move_to` says where they go.
    pub fn delete(
        &self,
        context: &UserContext,
        raw_id: &str,
        move_to: Option<&str>,
        actor: &str,
    ) -> Result<usize> {
        if let Some(into) = move_to {
            return Ok(self.merge(context, raw_id, into, actor)?.changed);
        }
        let category = self.custom_known(context, raw_id)?;
        let members = self.members(context, &category)?.len();
        if members > 0 {
            return Err(RaError::Conflict(format!(
                "{category} still holds {members} {}; move them first (move_to)",
                if members == 1 { "memory" } else { "memories" }
            )));
        }
        self.catalog.remove(context, &category)?;
        Ok(0)
    }

    fn move_members(
        &self,
        context: &UserContext,
        from: &Category,
        into: &Category,
        actor: &str,
        detail: &str,
    ) -> Result<BulkOutcome> {
        let members = self.members(context, from)?;
        self.editor.apply(
            context,
            members,
            |_| {
                Some(MemoryEdit {
                    category: Some(into.clone()),
                    ..Default::default()
                })
            },
            actor,
            detail,
        )
    }

    fn members(&self, context: &UserContext, category: &Category) -> Result<Vec<Memory>> {
        Ok(self
            .memories
            .list(context, true)?
            .into_iter()
            .filter(|memory| memory.category() == category)
            .collect())
    }

    /// A category the user has: a built-in, or one in their catalog.
    fn known(&self, context: &UserContext, raw: &str) -> Result<Category> {
        self.catalog
            .resolve(context, raw)
            .map_err(|_| RaError::NotFound(format!("category {:?} not found", raw.trim())))
    }

    fn custom_known(&self, context: &UserContext, raw: &str) -> Result<Category> {
        let category = self.known(context, raw)?;
        if category.is_built_in() {
            return Err(RaError::Conflict(format!(
                "{category} is a built-in category: it can be relabelled, not renamed, merged or deleted"
            )));
        }
        Ok(category)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, new_memory};
    use crate::memories::domain::document_text::DocumentText;
    use crate::memories::domain::recall_query::RecallQuery;
    use crate::memories::domain::vector_index::VectorIndex;

    fn save_in(fixture: &Fixture, content: &str, category: Category) -> Memory {
        let mut new = new_memory(content);
        new.category = category;
        fixture.saver().execute(&fixture.alex, new, "test").unwrap()
    }

    fn view<'a>(views: &'a [CategoryView], id: &str) -> Option<&'a CategoryView> {
        views.iter().find(|v| v.category.as_str() == id)
    }

    #[test]
    fn the_list_has_every_built_in_and_the_seeded_extras_with_counts() {
        let fixture = Fixture::new();
        let curator = fixture.category_curator(&["health.sleep"]);
        fixture.save(&fixture.alex, "uses pnpm");

        let views = curator.list(&fixture.alex).unwrap();

        assert_eq!(views.len(), 9);
        assert_eq!(view(&views, "preference.coding").unwrap().count, 1);
        let sleep = view(&views, "health.sleep").unwrap();
        assert_eq!((sleep.count, sleep.known), (0, true));
        assert_eq!(sleep.label, "Health sleep");
    }

    #[test]
    fn a_created_category_is_valid_for_that_user_only() {
        let fixture = Fixture::new();
        let curator = fixture.category_curator(&[]);
        let catalog = fixture.catalog(&[]);

        curator
            .create(&fixture.alex, "Health.Sleep", Some("Sleep log"))
            .unwrap();

        assert_eq!(
            catalog.resolve(&fixture.alex, "health.sleep").unwrap(),
            Category::Custom("health.sleep".to_string())
        );
        assert!(catalog.resolve(&fixture.sam, "health.sleep").is_err());
        let views = curator.list(&fixture.alex).unwrap();
        assert_eq!(view(&views, "health.sleep").unwrap().label, "Sleep log");
    }

    #[test]
    fn creating_a_duplicate_or_a_built_in_is_a_conflict() {
        let fixture = Fixture::new();
        let curator = fixture.category_curator(&[]);
        curator.create(&fixture.alex, "health", None).unwrap();

        for raw in ["health", "decision"] {
            assert!(matches!(
                curator.create(&fixture.alex, raw, None),
                Err(RaError::Conflict(_))
            ));
        }
    }

    #[test]
    fn relabelling_a_built_in_changes_only_its_label() {
        let fixture = Fixture::new();
        let curator = fixture.category_curator(&[]);
        let memory = fixture.save(&fixture.alex, "uses pnpm");
        let vector_before = fixture.vectors.all(&fixture.alex).unwrap();

        curator
            .relabel(&fixture.alex, "preference.coding", Some("How I code"))
            .unwrap();

        let views = curator.list(&fixture.alex).unwrap();
        let coding = view(&views, "preference.coding").unwrap();
        assert_eq!(
            (coding.label.as_str(), coding.relabelled),
            ("How I code", true)
        );
        assert_eq!(fixture.vectors.all(&fixture.alex).unwrap(), vector_before);
        let _ = memory;
        // Clearing goes back to the default wording.
        curator
            .relabel(&fixture.alex, "preference.coding", Some(" "))
            .unwrap();
        let views = curator.list(&fixture.alex).unwrap();
        assert_eq!(
            view(&views, "preference.coding").unwrap().label,
            "Coding preference"
        );
    }

    #[test]
    fn renaming_refiles_and_re_embeds_the_members_and_recall_still_finds_them() {
        let fixture = Fixture::new();
        let editor = Arc::new(
            fixture
                .bulk_editor()
                .with_document_text(DocumentText::Enriched),
        );
        let curator = CategoryCurator::new(
            fixture.catalog(&[]),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            editor,
        );
        curator
            .create(&fixture.alex, "health", Some("Health"))
            .unwrap();
        let memory = save_in(
            &fixture,
            "sleeps eight hours",
            Category::Custom("health".into()),
        );

        let (entry, moved) = curator
            .rename(&fixture.alex, "health", "health.sleep", "web-ui")
            .unwrap();

        assert_eq!(entry.label.as_deref(), Some("Health"), "the label travels");
        assert_eq!((moved.changed, moved.reembedded), (1, 1));
        let stored = fixture
            .memories
            .find(&fixture.alex, memory.id())
            .unwrap()
            .unwrap();
        assert_eq!(stored.category().as_str(), "health.sleep");
        let views = curator.list(&fixture.alex).unwrap();
        assert!(view(&views, "health").is_none());
        let hits = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &RecallQuery::new("sleeps eight hours", 5).unwrap(),
            )
            .unwrap();
        assert_eq!(hits[0].memory.id(), memory.id());
    }

    #[test]
    fn a_failed_rename_leaves_the_old_category_and_its_members() {
        let fixture = Fixture::new();
        let editor = Arc::new(
            fixture
                .bulk_editor()
                .with_document_text(DocumentText::Enriched),
        );
        let curator = CategoryCurator::new(
            fixture.catalog(&[]),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            editor,
        );
        curator.create(&fixture.alex, "health", None).unwrap();
        let memory = save_in(&fixture, "sleeps", Category::Custom("health".into()));
        fixture.embedder.fail_next();

        assert!(
            curator
                .rename(&fixture.alex, "health", "wellbeing", "t")
                .is_err()
        );

        let views = curator.list(&fixture.alex).unwrap();
        assert!(view(&views, "health").is_some());
        assert!(
            view(&views, "wellbeing").is_none(),
            "the new id was rolled back"
        );
        let stored = fixture
            .memories
            .find(&fixture.alex, memory.id())
            .unwrap()
            .unwrap();
        assert_eq!(stored.category().as_str(), "health");
    }

    #[test]
    fn merging_moves_every_member_and_removes_the_category() {
        let fixture = Fixture::new();
        let curator = fixture.category_curator(&["homelab"]);
        let a = save_in(&fixture, "runs proxmox", Category::Custom("homelab".into()));
        let b = save_in(&fixture, "has a nas", Category::Custom("homelab".into()));

        let moved = curator
            .merge(&fixture.alex, "homelab", "fact.project", "t")
            .unwrap();

        assert_eq!(moved.changed, 2);
        for id in [a.id(), b.id()] {
            let stored = fixture.memories.find(&fixture.alex, id).unwrap().unwrap();
            assert_eq!(stored.category(), &Category::FactProject);
        }
        assert!(view(&curator.list(&fixture.alex).unwrap(), "homelab").is_none());
    }

    #[test]
    fn deleting_a_non_empty_category_needs_a_destination() {
        let fixture = Fixture::new();
        let curator = fixture.category_curator(&["homelab", "empty"]);
        save_in(&fixture, "runs proxmox", Category::Custom("homelab".into()));

        assert!(matches!(
            curator.delete(&fixture.alex, "homelab", None, "t"),
            Err(RaError::Conflict(m)) if m.contains("1 memory")
        ));
        assert_eq!(
            curator.delete(&fixture.alex, "empty", None, "t").unwrap(),
            0
        );
        assert_eq!(
            curator
                .delete(&fixture.alex, "homelab", Some("skill"), "t")
                .unwrap(),
            1
        );
        let views = curator.list(&fixture.alex).unwrap();
        assert!(view(&views, "homelab").is_none() && view(&views, "empty").is_none());
    }

    #[test]
    fn a_deleted_seed_stays_deleted() {
        let fixture = Fixture::new();
        let curator = fixture.category_curator(&["homelab"]);
        curator.delete(&fixture.alex, "homelab", None, "t").unwrap();

        // A fresh catalog over the same database, as after a restart.
        let views = fixture
            .category_curator(&["homelab"])
            .list(&fixture.alex)
            .unwrap();

        assert!(view(&views, "homelab").is_none());
    }

    #[test]
    fn built_ins_cannot_be_renamed_merged_or_deleted() {
        let fixture = Fixture::new();
        let curator = fixture.category_curator(&[]);

        assert!(matches!(
            curator.rename(&fixture.alex, "decision", "choices", "t"),
            Err(RaError::Conflict(_))
        ));
        assert!(matches!(
            curator.merge(&fixture.alex, "decision", "skill", "t"),
            Err(RaError::Conflict(_))
        ));
        assert!(matches!(
            curator.delete(&fixture.alex, "decision", None, "t"),
            Err(RaError::Conflict(_))
        ));
    }

    #[test]
    fn an_unknown_category_is_not_found() {
        let fixture = Fixture::new();
        let curator = fixture.category_curator(&[]);
        assert!(matches!(
            curator.relabel(&fixture.alex, "nope", Some("x")),
            Err(RaError::NotFound(_))
        ));
        assert!(matches!(
            curator.merge(&fixture.alex, "nope", "skill", "t"),
            Err(RaError::NotFound(_))
        ));
    }
}
