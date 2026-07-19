//! Storage contract for users. Implemented by `SqliteUserRepository`
//! (infrastructure) and by an in-memory fake in tests.

use super::user::User;
use crate::shared::error::Result;
use crate::shared::ids::UserId;

pub trait UserRepository: Send + Sync {
    /// Persists a new user. Returns `RaError::Conflict` if the handle is
    /// already taken — the uniqueness rule is enforced by the store, not
    /// by a check-then-insert race in the use case.
    fn insert(&self, user: &User) -> Result<()>;

    fn find_by_handle(&self, handle: &str) -> Result<Option<User>>;

    fn find_by_id(&self, id: UserId) -> Result<Option<User>>;

    fn list(&self) -> Result<Vec<User>>;
}
