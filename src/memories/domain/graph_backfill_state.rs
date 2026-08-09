//! Where the relation backfill left off, as a contract.
//!
//! `graph backfill --relations` (Task 7.3.5) walks a user's memories oldest
//! first, re-extracting relations for the ones that have none. Each is a
//! model call, and the run is bounded by `[graph].backfill_budget`, so it
//! routinely stops partway. This store is how the next run resumes: it
//! remembers the `created_at` of the newest memory already attempted, and
//! the runner considers only memories created after it.
//!
//! # Why `created_at`, and why "attempted" rather than "written"
//!
//! A memory that yields no relations leaves nothing on the graph to mark it
//! done, so keying resumption on "has edges" would re-extract — and re-pay
//! for — every relationless memory on every run. The watermark marks how
//! far the backfill has *looked*, not how far it has *written*, so those
//! memories are attempted once. `created_at` is the cursor because it is
//! assigned at ingest and does not move afterwards, so ordering by it is
//! stable across runs.
//!
//! # Why the cursor is only advanced after a memory is handled
//!
//! Recording it before extracting, or for a memory whose extraction failed,
//! would skip that memory forever. So the runner advances it only once a
//! memory has been handled without error, and never past a memory the
//! budget stopped it before reaching — exactly the discipline the
//! consolidation watermark keeps (`consolidation::domain::consolidation_state`).

use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use chrono::{DateTime, Utc};

pub trait GraphBackfillState: Send + Sync {
    /// The `created_at` of the newest memory whose relations the backfill
    /// has attempted for this user, or `None` if it never ran.
    fn relations_cursor(&self, context: &UserContext) -> Result<Option<DateTime<Utc>>>;

    /// Record how far relation backfill has reached for this user. The
    /// runner only ever calls this with a value at or beyond the memory it
    /// just finished, walking oldest first, so the cursor only moves
    /// forward.
    fn set_relations_cursor(&self, context: &UserContext, cursor: DateTime<Utc>) -> Result<()>;
}
