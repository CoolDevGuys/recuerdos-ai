//! A user telling the store who they are: the names "the user" goes by
//! (`/v1/me`, the web UI's About page).
//!
//! Saving the names also looks for the memories they reveal as the user's
//! own that are filed as `general` — "Alejandro Romero is a backend
//! engineer…" was, until now, a fact about some person in the user's world —
//! and sends them to the review queue. It re-files nothing itself: a
//! mistaken match must cost a look, not a wrong memory.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::applies_to::Target;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::self_names::{SelfNameStore, SelfNames, self_subject};
use crate::shared::error::Result;
use std::sync::Arc;

/// Recorded in each flagged memory's audit entry.
const FLAG_DETAIL: &str = "it reads as being about the user, but it is filed as general";

/// What saving the names did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamesSaved {
    pub names: SelfNames,
    /// Memories newly sent to review because they are about the user.
    pub flagged: usize,
}

pub struct SelfNamesCurator {
    store: Arc<dyn SelfNameStore>,
    memories: Arc<dyn MemoryRepository>,
}

impl SelfNamesCurator {
    pub fn new(store: Arc<dyn SelfNameStore>, memories: Arc<dyn MemoryRepository>) -> Self {
        Self { store, memories }
    }

    pub fn get(&self, context: &UserContext) -> Result<SelfNames> {
        self.store.get(context)
    }

    /// Replaces the user's names, then flags what they show to be theirs.
    pub fn set(&self, context: &UserContext, raw: &[String], actor: &str) -> Result<NamesSaved> {
        let names = SelfNames::parse(raw)?;
        self.store.set(context, &names)?;
        let flagged = self.flag_misfiled(context, &names, actor)?;
        Ok(NamesSaved { names, flagged })
    }

    /// Sends every active `general` memory whose subject is the user to the
    /// review queue. Already-flagged ones are left as they are.
    fn flag_misfiled(
        &self,
        context: &UserContext,
        names: &SelfNames,
        actor: &str,
    ) -> Result<usize> {
        let misfiled: Vec<Memory> = self
            .memories
            .list(context, false)?
            .into_iter()
            .filter(|memory| {
                let structure = memory.structure();
                structure.applies_to().target() == Target::General
                    && structure.kind().allows(Target::Me)
                    && !structure.needs_review()
                    && self_subject(memory.content(), names).is_some()
            })
            .map(|memory| {
                let flagged = memory.structure().clone().flagged_for_review(true);
                memory.with_structure(flagged)
            })
            .collect();
        if !misfiled.is_empty() {
            self.memories
                .update_many(context, &misfiled, actor, FLAG_DETAIL)?;
        }
        Ok(misfiled.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::applies_to::AppliesTo;
    use crate::memories::domain::kind::Kind;
    use crate::memories::domain::memory_structure::{MemoryStructure, Origin};
    use crate::memories::domain::topic::Topic;

    fn filed(fixture: &Fixture, content: &str, applies_to: AppliesTo) -> Memory {
        let memory = fixture.save(&fixture.alex, content);
        let structure =
            MemoryStructure::new(Kind::Fact, Topic::Work, applies_to, Origin::UserSaid, None)
                .unwrap();
        let memory = memory.with_structure(structure);
        fixture
            .memories
            .update(&fixture.alex, &memory, "test")
            .unwrap();
        memory
    }

    #[test]
    fn saving_the_names_sends_the_users_own_general_memories_to_review() {
        let fixture = Fixture::new();
        let named = filed(
            &fixture,
            "Alejandro Romero is a Senior Backend Software Engineer in Berlin",
            AppliesTo::General,
        );
        let the_user = filed(&fixture, "The user is an expert in PHP", AppliesTo::General);
        let sister = filed(
            &fixture,
            "The user's sister Ana lives in Madrid",
            AppliesTo::General,
        );
        let already_mine = filed(&fixture, "The user works at Kau", AppliesTo::Me);
        let curator = SelfNamesCurator::new(
            fixture.self_names(),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
        );

        let saved = curator
            .set(
                &fixture.alex,
                &["Alejandro Romero".to_string(), "Alex".to_string()],
                "web-ui",
            )
            .unwrap();

        assert_eq!(saved.flagged, 2);
        assert_eq!(curator.get(&fixture.alex).unwrap(), saved.names);
        let review = |memory: &Memory| {
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .unwrap()
                .structure()
                .needs_review()
        };
        assert!(review(&named));
        assert!(review(&the_user));
        assert!(!review(&sister), "the sister's memory is about her");
        assert!(!review(&already_mine));

        // Saving again finds nothing new: they are in the queue already.
        let again = curator
            .set(&fixture.alex, &["Alejandro Romero".to_string()], "web-ui")
            .unwrap();
        assert_eq!(again.flagged, 0);
    }

    #[test]
    fn a_name_that_breaks_the_rules_is_refused_and_nothing_is_stored() {
        let fixture = Fixture::new();
        let curator = SelfNamesCurator::new(
            fixture.self_names(),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
        );
        let many: Vec<String> = (0..9).map(|i| format!("name {i}")).collect();

        assert!(curator.set(&fixture.alex, &many, "web-ui").is_err());
        assert!(curator.get(&fixture.alex).unwrap().is_empty());
    }
}
