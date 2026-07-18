//! Shared kernel: ids, error type, clock. Tiny by design — every other
//! context may depend on `shared`, but `shared` depends on nothing in the
//! crate. See `domain` boundary rule 1 in `docs/architecture.md`.

pub mod clock;
pub mod error;
pub mod ids;

pub use clock::{Clock, SystemClock};
pub use error::{RaError, Result};
pub use ids::{MemoryId, UserId};
