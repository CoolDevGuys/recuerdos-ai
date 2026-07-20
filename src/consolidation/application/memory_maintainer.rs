//! The half of the nightly job that needs no language model: retire
//! what has expired, and rescore what is left.
//!
//! # Why it is separate from merging
//!
//! Merging is a judgement about meaning and cannot happen without a
//! model. Expiry is a comparison against a timestamp the user themselves
//! set, and decay is arithmetic over access bookkeeping. Neither needs a
//! provider, and tying them to one would mean a zero-egress installation
//! — the default — silently keeps expired memories forever.
//!
//! # Why expiry retires rather than deletes
//!
//! `expires_at` is a promise that a memory stops being *used*, not that
//! it stops existing. A user who set a three-month expiry on "on call
//! until March" is asking not to be told about it in April; they are not
//! asking for the record of having set it to be destroyed. So an expired
//! memory is soft-deleted exactly as `memory_forget` does it — gone from
//! recall, present in the audit trail, with a reason saying why it went.
//! Actually erasing bytes stays a governance operation
//! (project-plan.md §15).

use crate::consolidation::domain::decay::importance;
use crate::identity::domain::user_context::UserContext;
use crate::memories::application::memory_forgetter::MemoryForgetter;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Recorded as the actor on expiry, so the trail distinguishes a memory
/// the user deleted from one that reached the date they set for it.
pub const ACTOR: &str = "consolidation";

/// What one maintenance pass did for one user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaintenanceOutcome {
    pub expired: usize,
    pub rescored: usize,
}

pub struct MemoryMaintainer {
    memories: Arc<dyn MemoryRepository>,
    forgetter: Arc<MemoryForgetter>,
}

impl MemoryMaintainer {
    pub fn new(memories: Arc<dyn MemoryRepository>, forgetter: Arc<MemoryForgetter>) -> Self {
        Self {
            memories,
            forgetter,
        }
    }

    /// Synchronous: it is all database work, and the caller already runs
    /// it inside a blocking section.
    pub fn execute(&self, context: &UserContext, now: DateTime<Utc>) -> Result<MaintenanceOutcome> {
        let stored = self.memories.list(context, false)?;
        let mut outcome = MaintenanceOutcome::default();

        // Expiry first: a memory about to be retired should not have its
        // decay score recomputed, and rescoring it would be wasted work.
        let (expired, live): (Vec<Memory>, Vec<Memory>) = stored
            .into_iter()
            .partition(|memory| memory.is_expired_at(now));

        for memory in &expired {
            let reason = match memory.expires_at() {
                Some(at) => format!("expired on {}", at.format("%Y-%m-%d")),
                // Unreachable — `is_expired_at` is false without one —
                // but a memory disappearing with no stated reason is
                // exactly the outcome the trail exists to prevent.
                None => "expired".to_string(),
            };

            if let Err(error) = self.forgetter.execute(context, memory.id(), ACTOR, &reason) {
                tracing::warn!(memory_id = %memory.id(), %error, "could not retire an expired memory");
                continue;
            }
            outcome.expired += 1;
        }

        // Superseded memories are excluded: they are already out of
        // recall, so their score changes nothing and writing it would be
        // pure churn.
        let scores: Vec<(MemoryId, f32)> = live
            .iter()
            .filter(|memory| !memory.is_superseded())
            .map(|memory| {
                (
                    memory.id(),
                    importance(
                        memory.created_at(),
                        memory.last_accessed_at(),
                        memory.access_count(),
                        now,
                    ),
                )
            })
            .collect();

        outcome.rescored = scores.len();
        self.memories.set_importance(context, &scores)?;

        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consolidation::domain::decay::MIN_IMPORTANCE;
    use crate::memories::application::test_doubles::{Fixture, new_memory, now};
    use crate::memories::domain::memory_repository::AuditOperation;
    use crate::memories::domain::recall_query::RecallQuery;
    use chrono::Duration;

    fn maintainer(fixture: &Fixture) -> MemoryMaintainer {
        MemoryMaintainer::new(
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::new(fixture.forgetter()),
        )
    }

    fn save_expiring(fixture: &Fixture, content: &str, at: DateTime<Utc>) -> Memory {
        let mut new = new_memory(content);
        new.expires_at = Some(at);
        fixture
            .saver()
            .execute(&fixture.alex, new, "test")
            .expect("save should succeed")
    }

    fn recall(fixture: &Fixture, query: &str) -> Vec<String> {
        fixture
            .recaller()
            .execute(&fixture.alex, &RecallQuery::new(query, 20).unwrap())
            .unwrap()
            .into_iter()
            .map(|scored| scored.memory.content().to_string())
            .collect()
    }

    #[test]
    fn a_memory_that_has_not_reached_its_expiry_is_left_alone() {
        let fixture = Fixture::new();
        save_expiring(&fixture, "on call until March", now() + Duration::days(30));

        let outcome = maintainer(&fixture).execute(&fixture.alex, now()).unwrap();

        assert_eq!(outcome.expired, 0);
        assert_eq!(recall(&fixture, "on call").len(), 1);
    }

    #[test]
    fn an_expired_memory_is_retired_once_its_date_passes() {
        // Time travel: the same store, looked at from a month later.
        let fixture = Fixture::new();
        let expiring = save_expiring(&fixture, "on call until March", now() + Duration::days(30));
        fixture.save(&fixture.alex, "a memory with no expiry");

        let outcome = maintainer(&fixture)
            .execute(&fixture.alex, now() + Duration::days(31))
            .unwrap();

        assert_eq!(outcome.expired, 1);
        assert!(
            !recall(&fixture, "on call").contains(&"on call until March".to_string()),
            "an expired memory is still being recalled"
        );
        assert!(
            fixture
                .memories
                .find(&fixture.alex, expiring.id())
                .unwrap()
                .is_none(),
            "expiry should soft-delete, which hides the row from find"
        );
        assert_eq!(
            recall(&fixture, "no expiry").len(),
            1,
            "an unrelated memory was caught up in expiry"
        );
    }

    #[test]
    fn an_expired_memory_leaves_a_trail_saying_why_it_went() {
        // "Why did my memory disappear?" is the question the trail
        // exists to answer, and expiry is the case where nobody was
        // there to see it happen.
        let fixture = Fixture::new();
        save_expiring(&fixture, "on call until March", now() + Duration::days(30));

        maintainer(&fixture)
            .execute(&fixture.alex, now() + Duration::days(31))
            .unwrap();

        let trail = fixture.memories.audit_trail(&fixture.alex, 20).unwrap();
        let entry = trail
            .iter()
            .find(|entry| entry.operation == AuditOperation::Delete)
            .expect("a delete entry");

        assert_eq!(entry.actor, ACTOR);
        assert!(
            entry.detail.contains("expired on"),
            "detail was {:?}",
            entry.detail
        );
    }

    #[test]
    fn a_memory_nobody_reads_decays_and_a_used_one_does_not() {
        // The ranking consequence is asserted in the ranker's own tests;
        // this pins that the job actually writes the scores.
        let fixture = Fixture::new();
        let ignored = fixture.save(&fixture.alex, "a memory nobody reads");
        let used = fixture.save(&fixture.alex, "a memory in daily use");

        // Recalling bumps last_accessed_at and access_count.
        for _ in 0..10 {
            fixture
                .memories
                .touch_accessed(&fixture.alex, &[used.id()], now() + Duration::days(364))
                .unwrap();
        }

        let outcome = maintainer(&fixture)
            .execute(&fixture.alex, now() + Duration::days(365))
            .unwrap();

        assert_eq!(outcome.rescored, 2);
        let scored = |id| {
            fixture
                .memories
                .find(&fixture.alex, id)
                .unwrap()
                .unwrap()
                .importance()
        };
        assert!(
            scored(used.id()) > scored(ignored.id()),
            "used {} did not outscore ignored {}",
            scored(used.id()),
            scored(ignored.id())
        );
    }

    #[test]
    fn decay_never_takes_a_memory_below_the_floor() {
        // An unread memory must lose ties, not disappear. This is the
        // guarantee that lets the job run unattended.
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "an architecture decision from years ago");

        maintainer(&fixture)
            .execute(&fixture.alex, now() + Duration::days(5_000))
            .unwrap();

        let importance = fixture
            .memories
            .find(&fixture.alex, old.id())
            .unwrap()
            .unwrap()
            .importance();

        assert!(importance >= MIN_IMPORTANCE, "got {importance}");
        assert_eq!(
            recall(&fixture, "architecture decision").len(),
            1,
            "a decayed memory must still be findable"
        );
    }

    #[test]
    fn rescoring_writes_nothing_to_the_audit_trail() {
        // Otherwise a nightly job over a few thousand memories buries
        // every change the user actually made.
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "a memory");
        let before = fixture
            .memories
            .audit_trail(&fixture.alex, 100)
            .unwrap()
            .len();

        maintainer(&fixture).execute(&fixture.alex, now()).unwrap();

        assert_eq!(
            fixture
                .memories
                .audit_trail(&fixture.alex, 100)
                .unwrap()
                .len(),
            before,
            "rescoring added audit entries"
        );
    }

    #[test]
    fn maintenance_touches_only_the_given_users_memories() {
        let fixture = Fixture::new();
        let mut theirs = new_memory("sam's expiring memory");
        theirs.expires_at = Some(now() + Duration::days(1));
        fixture
            .saver()
            .execute(&fixture.sam, theirs, "test")
            .unwrap();

        let outcome = maintainer(&fixture)
            .execute(&fixture.alex, now() + Duration::days(2))
            .unwrap();

        assert_eq!(outcome.expired, 0, "another user's memory was expired");
        assert_eq!(outcome.rescored, 0);
    }

    #[test]
    fn an_empty_store_is_a_no_op_rather_than_an_error() {
        let fixture = Fixture::new();

        let outcome = maintainer(&fixture).execute(&fixture.alex, now()).unwrap();

        assert_eq!(outcome, MaintenanceOutcome::default());
    }
}
