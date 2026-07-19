//! Fetches one memory by id.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use std::sync::Arc;

pub struct MemoryFinder {
    memories: Arc<dyn MemoryRepository>,
}

impl MemoryFinder {
    pub fn new(memories: Arc<dyn MemoryRepository>) -> Self {
        Self { memories }
    }

    /// Returns `NotFound` both when the memory does not exist and when it
    /// belongs to someone else — the two must be indistinguishable, or
    /// the API becomes an oracle for other users' ids.
    pub fn execute(&self, context: &UserContext, id: MemoryId) -> Result<Memory> {
        self.memories
            .find(context, id)?
            .ok_or_else(|| RaError::NotFound(format!("memory {id} not found")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;

    #[test]
    fn finds_the_callers_own_memory() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "a note");

        let found = fixture
            .finder()
            .execute(&fixture.alex, memory.id())
            .unwrap();

        assert_eq!(found.id(), memory.id());
    }

    #[test]
    fn a_missing_memory_is_not_found() {
        let fixture = Fixture::new();
        assert!(matches!(
            fixture.finder().execute(&fixture.alex, MemoryId::new()),
            Err(RaError::NotFound(_))
        ));
    }

    #[test]
    fn another_users_memory_is_indistinguishable_from_a_missing_one() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "alex's note");

        let existing_other_user = fixture.finder().execute(&fixture.sam, memory.id());
        let genuinely_missing = fixture.finder().execute(&fixture.sam, MemoryId::new());

        // Same variant *and* same shape of message: a caller must not be
        // able to tell "exists but not yours" from "does not exist".
        assert!(matches!(existing_other_user, Err(RaError::NotFound(_))));
        assert!(matches!(genuinely_missing, Err(RaError::NotFound(_))));
    }
}
