//! A user curating their topics (ai-specs/memory-structure-plan.md §3.3, the
//! web UI's Topics tab): listing them with how many memories each holds,
//! adding their own, and relabelling any.
//!
//! A label is display only, as a category's is: memories store the topic's
//! id, so nothing is re-filed or re-embedded. Renaming and merging topics is
//! a bulk edit of their memories, which the list view already does.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::topic::Topic;
use crate::memories::domain::topic_catalog::{TopicCatalog, TopicEntry, normalise_topic_label};
use crate::shared::clock::Clock;
use crate::shared::error::{RaError, Result};
use std::collections::HashMap;
use std::sync::Arc;

/// One topic as the Topics tab lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicView {
    pub entry: TopicEntry,
    /// Active memories about it.
    pub count: usize,
}

pub struct TopicCurator {
    catalog: Arc<dyn TopicCatalog>,
    memories: Arc<dyn MemoryRepository>,
    clock: Arc<dyn Clock>,
}

impl TopicCurator {
    pub fn new(
        catalog: Arc<dyn TopicCatalog>,
        memories: Arc<dyn MemoryRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            catalog,
            memories,
            clock,
        }
    }

    /// Built-ins in their order, then the user's own by id.
    pub fn list(&self, context: &UserContext) -> Result<Vec<TopicView>> {
        let now = self.clock.now();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for memory in self.memories.list(context, false)? {
            if memory.is_active_at(now) {
                *counts
                    .entry(memory.structure().topic().as_str().to_string())
                    .or_default() += 1;
            }
        }
        Ok(self
            .catalog
            .entries(context)?
            .into_iter()
            .map(|entry| TopicView {
                count: counts.get(entry.topic.as_str()).copied().unwrap_or(0),
                entry,
            })
            .collect())
    }

    pub fn create(
        &self,
        context: &UserContext,
        raw_id: &str,
        label: Option<&str>,
    ) -> Result<TopicView> {
        let topic = Topic::custom(raw_id)?;
        let label = normalise_topic_label(label)?;
        self.catalog.create(context, &topic, label.as_deref())?;
        self.view(context, &topic)
    }

    /// `label: None` (or blank) goes back to the default wording.
    pub fn relabel(
        &self,
        context: &UserContext,
        raw_id: &str,
        label: Option<&str>,
    ) -> Result<TopicView> {
        let topic = self.known(context, raw_id)?;
        let label = normalise_topic_label(label)?;
        self.catalog.relabel(context, &topic, label.as_deref())?;
        self.view(context, &topic)
    }

    /// The topic with this id, if the user may file under it.
    pub fn known(&self, context: &UserContext, raw_id: &str) -> Result<Topic> {
        self.catalog.find(context, raw_id)?.ok_or_else(|| {
            RaError::rejected("unknown_topic", format!("You have no topic {raw_id:?}."))
        })
    }

    fn view(&self, context: &UserContext, topic: &Topic) -> Result<TopicView> {
        self.list(context)?
            .into_iter()
            .find(|view| &view.entry.topic == topic)
            .ok_or_else(|| RaError::NotFound(format!("topic {topic}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, fixed_clock};

    fn curator(fixture: &Fixture) -> TopicCurator {
        TopicCurator::new(
            fixture.topic_catalog(),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            fixed_clock(),
        )
    }

    #[test]
    fn built_ins_come_first_with_their_counts_then_the_users_own() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "prefers pnpm");
        let curator = curator(&fixture);
        curator
            .create(&fixture.alex, "Homelab", Some("Home lab"))
            .unwrap();

        let topics = curator.list(&fixture.alex).unwrap();

        assert_eq!(topics[0].entry.topic, Topic::Coding);
        assert_eq!(topics[0].count, 1);
        let last = topics.last().unwrap();
        assert_eq!(last.entry.topic, Topic::Custom("homelab".into()));
        assert_eq!(last.entry.label(), "Home lab");
        assert!(
            curator
                .list(&fixture.sam)
                .unwrap()
                .iter()
                .all(|view| view.entry.topic.is_built_in()),
            "one user's topics are not another's"
        );
    }

    #[test]
    fn a_built_in_is_relabelled_and_reset() {
        let fixture = Fixture::new();
        let curator = curator(&fixture);

        let relabelled = curator
            .relabel(&fixture.alex, "coding", Some("Software"))
            .unwrap();
        assert_eq!(relabelled.entry.label(), "Software");

        let reset = curator
            .relabel(&fixture.alex, "coding", Some("  "))
            .unwrap();
        assert_eq!(reset.entry.label(), "Coding");
    }

    #[test]
    fn a_duplicate_a_built_in_or_an_unknown_topic_is_refused() {
        let fixture = Fixture::new();
        let curator = curator(&fixture);
        curator.create(&fixture.alex, "homelab", None).unwrap();

        assert!(matches!(
            curator.create(&fixture.alex, "homelab", None).unwrap_err(),
            RaError::Conflict(_)
        ));
        assert!(matches!(
            curator.create(&fixture.alex, "health", None).unwrap_err(),
            RaError::Conflict(_)
        ));
        assert!(matches!(
            curator.relabel(&fixture.alex, "garden", Some("x")).unwrap_err(),
            RaError::Rejected { code, .. } if code == "unknown_topic"
        ));
        assert!(curator.create(&fixture.alex, "Home Lab", None).is_err());
    }

    #[test]
    fn a_new_topic_can_be_filed_under_at_once() {
        // It is in the catalog a save checks.
        let fixture = Fixture::new();
        curator(&fixture)
            .create(&fixture.alex, "homelab", None)
            .unwrap();
        assert!(
            fixture
                .topic_catalog()
                .find(&fixture.alex, "homelab")
                .unwrap()
                .is_some()
        );
    }
}
