//! The skip-unchanged watermark, as a contract.
//!
//! # What it is for
//!
//! Consolidation clusters near-duplicates within a `(category,
//! subcategory)` group. The expensive part — re-embedding the group and
//! asking a model whether clusters are the same thing — is wasted on a
//! group that has not changed since it was last consolidated. This store
//! remembers, per group, the maximum `updated_at` seen at the last
//! successful pass; the runner skips a group whose current maximum still
//! matches.
//!
//! # Why `updated_at` is the right signal
//!
//! Any create, edit or supersede bumps a memory's `updated_at`. Nightly
//! rescoring (importance) and recall bookkeeping
//! (`last_accessed_at`/`access_count`) deliberately do not — see
//! `sqlite_memory_repository`. So the maximum `updated_at` over a group's
//! active memories rises exactly when something a merge would care about
//! changed, and stays put otherwise. Comparing it against a stored
//! watermark can never skip a group that gained or lost a memory.
//!
//! # Why the watermark is only ever written after a successful pass
//!
//! Recording it before a merge, or for a group whose merge failed, would
//! mark unmerged duplicates as "done" and skip them forever. So the
//! runner records a group only once its clusters have been handled without
//! error, and never when the run stopped early on a budget limit.

use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use chrono::{DateTime, Utc};

/// Per-user, per-`(category, subcategory)` record of the last
/// consolidation watermark. `subcategory` is `None` for memories without
/// a sub-label; implementations map that to whatever their storage needs.
pub trait ConsolidationStateStore: Send + Sync {
    /// The maximum `updated_at` recorded at the last successful
    /// consolidation of this group, or `None` if it was never
    /// consolidated.
    fn last_max_updated_at(
        &self,
        context: &UserContext,
        category: &str,
        subcategory: Option<&str>,
    ) -> Result<Option<DateTime<Utc>>>;

    /// Record the watermark for a group that was just consolidated.
    fn record(
        &self,
        context: &UserContext,
        category: &str,
        subcategory: Option<&str>,
        max_updated_at: DateTime<Utc>,
    ) -> Result<()>;
}
