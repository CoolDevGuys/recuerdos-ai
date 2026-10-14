//! Rebuilds the derived keyword index from the `memories` table.
//!
//! # Why this exists
//!
//! The tantivy index is derived state (see its module docs), but until
//! now it only ever grew through per-save `upsert` calls — there was no
//! bulk path. An analyzer change (reranking-plan.md Phase 1) moves the
//! index into a new versioned directory, so every *existing* store needs
//! its keyword index rebuilt once, or keyword recall silently returns
//! nothing after the upgrade. Two entry points, one implementation:
//!
//! * [`rebuild_missing`](TextIndexRebuilder::rebuild_missing) — the
//!   daemon's startup path. A user with memories but no index of the
//!   current version gets one before the daemon serves; a user who
//!   already has one is not touched, so the check is cheap every start.
//! * [`rebuild_all`](TextIndexRebuilder::rebuild_all) — the
//!   `recuerdos-ai reindex --text` path, which rebuilds unconditionally
//!   and doubles as the repair for a half-finished run.
//!
//! # Guarantees
//!
//! * `memories` is read only. Nothing here writes to the system of
//!   record; a failed or interrupted run is fixed by running again —
//!   `upsert` deletes by id first, so re-indexing is idempotent.
//! * Commits come in chunks (see the use of `upsert_many`), so a
//!   10 000-memory rebuild is dozens of commits, not 10 000.
//! * Users are walked the way every background job walks them — via
//!   `UserRepository::list` and `BackgroundUserResolver` — so each pass
//!   carries a real `&UserContext` and index isolation stays
//!   structural. A user that no longer resolves is skipped with a warn,
//!   not fatal: their rows are unreadable to the API anyway.
//! * Superseded memories are *not* re-indexed (`list(…, false)`): the
//!   recall path filters them anyway, and a rebuild doubles as the tidy
//!   that drops their documents, which the incremental path left behind.

use crate::identity::application::background_user_resolver::BackgroundUserResolver;
use crate::identity::domain::user_repository::UserRepository;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::text_index::TextIndex;
use crate::shared::error::Result;
use std::sync::Arc;

/// Memories per index commit. Personal-scale corpora rebuild in one or
/// two chunks; the cap exists so a pathological store still commits
/// progress rather than one giant in-memory batch.
const COMMIT_CHUNK: usize = 500;

/// One user's line of a rebuild report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRebuild {
    pub handle: String,
    pub memories_indexed: usize,
}

/// What a rebuild did, for the startup log and the CLI summary.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RebuildReport {
    /// Users looked at at all.
    pub users_examined: usize,
    /// Users whose index was rebuilt (for `rebuild_all`: skipped means
    /// they have no memories to index).
    pub users_rebuilt: usize,
    pub users_skipped: usize,
    pub memories_indexed: usize,
    /// The rebuilt users, in the order they were processed.
    pub rebuilt: Vec<UserRebuild>,
}

pub struct TextIndexRebuilder {
    users: Arc<dyn UserRepository>,
    resolver: Arc<BackgroundUserResolver>,
    memories: Arc<dyn MemoryRepository>,
    text: Arc<dyn TextIndex>,
}

impl TextIndexRebuilder {
    pub fn new(
        users: Arc<dyn UserRepository>,
        memories: Arc<dyn MemoryRepository>,
        text: Arc<dyn TextIndex>,
    ) -> Self {
        Self {
            resolver: Arc::new(BackgroundUserResolver::new(Arc::clone(&users))),
            users,
            memories,
            text,
        }
    }

    /// Rebuild only users whose keyword index is missing while their
    /// memories are not (the first start after an index-version bump).
    /// Users with an index are not read at all.
    pub fn rebuild_missing(&self) -> Result<RebuildReport> {
        self.rebuild(true)
    }

    /// Rebuild every user, whatever is on disk. `reindex --text`.
    pub fn rebuild_all(&self) -> Result<RebuildReport> {
        self.rebuild(false)
    }

    fn rebuild(&self, only_missing: bool) -> Result<RebuildReport> {
        let started = std::time::Instant::now();
        let mut report = RebuildReport::default();

        for user in self.users.list()? {
            report.users_examined += 1;
            let context = match self.resolver.execute(user.id()) {
                Ok(context) => context,
                Err(error) => {
                    tracing::warn!(user = %user.handle(), %error, "skipping a user during the text index rebuild");
                    report.users_skipped += 1;
                    continue;
                }
            };

            if only_missing && self.text.is_populated(&context) {
                report.users_skipped += 1;
                continue;
            }

            let memories = self.memories.list(&context, false)?;
            if memories.is_empty() {
                // Nothing to index: leave no directory behind, so the
                // startup check stays a single cheap stat.
                report.users_skipped += 1;
                continue;
            }

            for chunk in memories.chunks(COMMIT_CHUNK) {
                self.text.upsert_many(&context, chunk)?;
            }

            report.memories_indexed += memories.len();
            report.users_rebuilt += 1;
            report.rebuilt.push(UserRebuild {
                handle: context.handle().to_string(),
                memories_indexed: memories.len(),
            });
        }

        if report.users_rebuilt > 0 {
            tracing::info!(
                users_rebuilt = report.users_rebuilt,
                memories_indexed = report.memories_indexed,
                elapsed_ms = started.elapsed().as_millis(),
                "text index rebuilt"
            );
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::domain::user_context::UserContext;
    use crate::memories::application::test_doubles::{Fixture, new_memory, now};
    use crate::memories::domain::memory::Memory;
    use crate::shared::error::RaError;

    fn rebuilder(fixture: &Fixture) -> TextIndexRebuilder {
        TextIndexRebuilder::new(
            Arc::clone(&fixture.users),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&fixture.text) as Arc<dyn TextIndex>,
        )
    }

    /// Insert through the repository only — the point is a memory that
    /// exists in the table but not in the text index, which is exactly
    /// the state a rebuild has to notice.
    fn insert(fixture: &Fixture, context: &UserContext, content: &str) -> Memory {
        let memory = Memory::create(context.user_id(), new_memory(content), now()).unwrap();
        fixture.memories.insert(context, &memory, "test").unwrap();
        memory
    }

    #[test]
    fn rebuilds_each_users_memories_into_their_own_index() {
        let fixture = Fixture::new();
        // Both users' text shares the search word, so the isolation
        // assertion below is about *whose* document matched, not whether
        // the word exists at all.
        let alex_first = insert(&fixture, &fixture.alex, "alex prefers pnpm");
        let alex_second = insert(&fixture, &fixture.alex, "pnpm on the frontend too");
        let sam_memory = insert(&fixture, &fixture.sam, "sam prefers pnpm");

        let report = rebuilder(&fixture).rebuild_all().unwrap();

        assert_eq!(report.users_examined, 2);
        assert_eq!(report.users_rebuilt, 2);
        assert_eq!(report.memories_indexed, 3);
        assert_eq!(report.rebuilt.len(), 2);

        let alex_hits = fixture
            .text
            .search(&fixture.alex, "pnpm", 10)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        let sam_hits = fixture
            .text
            .search(&fixture.sam, "pnpm", 10)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();

        assert_eq!(alex_hits.len(), 2);
        assert!(alex_hits.contains(&alex_first.id()));
        assert!(alex_hits.contains(&alex_second.id()));
        assert!(!alex_hits.contains(&sam_memory.id()));
        assert_eq!(sam_hits, vec![sam_memory.id()]);
    }

    #[test]
    fn soft_deleted_memories_are_not_re_indexed() {
        let fixture = Fixture::new();
        let kept = insert(&fixture, &fixture.alex, "kept memory about pnpm");
        let gone = insert(&fixture, &fixture.alex, "forgotten memory about pnpm");
        fixture
            .memories
            .delete(&fixture.alex, gone.id(), "test", "no longer wanted")
            .unwrap();

        let report = rebuilder(&fixture).rebuild_all().unwrap();

        assert_eq!(report.memories_indexed, 1);
        assert!(fixture.text.contains(kept.id()));
        assert!(!fixture.text.contains(gone.id()));
    }

    #[test]
    fn a_failure_midway_leaves_a_state_the_retry_fixes() {
        let fixture = Fixture::new();
        let a = insert(&fixture, &fixture.alex, "first pnpm memory");
        let b = insert(&fixture, &fixture.alex, "second pnpm memory");

        // The doubles' `upsert_many` falls back to the trait default — a
        // loop over `upsert` — so one injected failure stops the run with
        // the index half-populated (or not at all).
        fixture.text.fail_next_upsert();
        let first = rebuilder(&fixture).rebuild_all();
        assert!(matches!(first, Err(RaError::Internal(_))));

        // Retry: the injected failure is spent, and re-indexing is
        // delete-first, so whatever half-state is on disk is overwritten.
        let report = rebuilder(&fixture).rebuild_all().unwrap();
        assert_eq!(report.memories_indexed, 2);
        let hits = fixture
            .text
            .search(&fixture.alex, "pnpm", 10)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        assert!(hits.contains(&a.id()) && hits.contains(&b.id()));
    }

    #[test]
    fn rebuild_missing_skips_users_whose_index_is_already_there() {
        // The double reports itself populated (the trait default: an
        // index that keeps no per-user on-disk state never needs a
        // rebuild), so `rebuild_missing` must touch nothing. That is the
        // guarantee the *real* adapter earns with its versioned
        // directory, pinned here at the contract's default.
        let fixture = Fixture::new();
        insert(&fixture, &fixture.alex, "anything at all");

        let report = rebuilder(&fixture).rebuild_missing().unwrap();

        assert_eq!(report.users_examined, 2);
        assert_eq!(report.users_rebuilt, 0);
        assert_eq!(report.users_skipped, 2);
    }

    #[test]
    fn a_user_who_saved_nothing_is_skipped_not_rebuilt() {
        let fixture = Fixture::new();

        let report = rebuilder(&fixture).rebuild_all().unwrap();

        assert_eq!(report.users_examined, 2);
        assert_eq!(report.users_rebuilt, 0);
        assert_eq!(report.users_skipped, 2);
    }
}
