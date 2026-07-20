//! Shared kernel: ids, error type, clock. Tiny by design — every other
//! context may depend on `shared`, but `shared` depends on nothing in the
//! crate. See `domain` boundary rule 1 in `docs/architecture.md`.

pub mod api_error;
pub mod blocking;
pub mod clock;
pub mod error;
pub mod ids;
pub mod sqlite;

// Re-exported for a nicer `crate::shared::X` surface. Not yet used outside
// this module's own tests — consumers arrive with the identity/memories
// domain in Phase 1+.
#[allow(unused_imports)]
pub use clock::{Clock, SystemClock};
#[allow(unused_imports)]
pub use error::{RaError, Result};
#[allow(unused_imports)]
pub use ids::{ApiKeyId, MemoryId, UserId};
