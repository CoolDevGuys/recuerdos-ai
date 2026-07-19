//! Memories context: storing, indexing, searching, exporting memories.
//! Owns `Memory`, `Category`, `Tag`, `Recall`.

// The domain (Task 2.1) lands before the adapters and use cases that
// call it, so until `bootstrap` wires this context into the router
// (Task 2.6) the compiler sees an unreachable island. Removed at the end
// of Phase 2 — anything still dead then is genuinely dead and should be
// deleted rather than re-silenced. (Same contract as Phase 1's identity
// island, which ended with two deletions.)
#![allow(dead_code)]

pub mod application;
pub mod domain;
pub mod infrastructure;
