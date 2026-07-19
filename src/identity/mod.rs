//! Identity context: users, API keys, authentication, `UserContext` — the
//! capability token every other context requires to touch user data.

// The domain and its adapters land (Tasks 1.1/1.2) before the use cases
// and CLI that call them (Task 1.3) and the middleware that authenticates
// with them (Task 1.4). Until then the compiler sees an unreachable
// island. Removed at the end of Phase 1, once `bootstrap` wires this
// context into the binary — if anything here is still dead then, it is
// genuinely dead and should be deleted rather than re-silenced.
#![allow(dead_code)]

pub mod application;
pub mod domain;
pub mod infrastructure;
