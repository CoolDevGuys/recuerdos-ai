//! Pure domain: `User`, `ApiKey`, `Scope`, `UserContext` and their
//! invariants. No tokio, no sqlite, no http — see boundary rule 1.

pub mod api_key;
pub mod api_key_hasher;
pub mod api_key_repository;
pub mod scope;
pub mod user;
pub mod user_context;
pub mod user_repository;
