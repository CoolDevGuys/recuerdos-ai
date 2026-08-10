//! A port for "a user's active memory set may have changed".
//!
//! It lives in the memories context because that is the one both
//! understanding (which writes memories through ingest) and consolidation
//! (which merges and expires them, and which builds the derived profile)
//! already depend on. Defining it here lets the ingest worker and the
//! consolidation runner call it without either taking a dependency on the
//! other, and lets consolidation supply the single implementation.
//!
//! # Why it is a side effect, not a result
//!
//! The only subscriber is the profile-digest refresh, and its whole point
//! is to move the model call *off* the read path (`GET /v1/profile`) so a
//! slow provider can no longer time the read out. That means the observer
//! is best-effort by construction: a refresh that fails, or is dropped on
//! shutdown, costs a slightly stale profile until the next change or the
//! nightly sweep — never a failed ingest or a failed consolidation run. So
//! the method cannot fail its caller; an implementation logs its own
//! trouble and returns.

use crate::identity::domain::user_context::UserContext;

#[async_trait::async_trait]
pub trait MemoryChangeObserver: Send + Sync {
    /// Notify that `context`'s active memories may have changed. Called
    /// after an ingest job stores memories and after a consolidation pass
    /// touches a user, so a subscriber can bring anything derived from
    /// those memories back up to date without the read path having to.
    async fn memories_changed(&self, context: &UserContext);
}
