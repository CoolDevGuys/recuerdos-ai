//! Resolves the context a background job runs under.
//!
//! The ingestion worker faces a problem the request path does not: by the
//! time a job runs, the key that enqueued it is long gone. Something has
//! to say "this work belongs to alex" without a credential to check.
//!
//! This is that something, and it is deliberately the *only* one. The
//! user id it takes comes from a persisted job row — written by a handler
//! that had already authenticated as that user — never from anything a
//! caller supplies. And it resolves against the user table rather than
//! trusting the id blindly: a job whose user was deleted resolves to
//! nothing and fails, instead of writing memories owned by a ghost.

use crate::identity::domain::user_context::UserContext;
use crate::identity::domain::user_repository::UserRepository;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::UserId;
use std::sync::Arc;

pub struct BackgroundUserResolver {
    users: Arc<dyn UserRepository>,
}

impl BackgroundUserResolver {
    pub fn new(users: Arc<dyn UserRepository>) -> Self {
        Self { users }
    }

    /// The read/write context for `user_id`, or `NotFound` if that user
    /// no longer exists.
    pub fn execute(&self, user_id: UserId) -> Result<UserContext> {
        let user = self
            .users
            .find_by_id(user_id)?
            .ok_or_else(|| RaError::NotFound(format!("user {user_id} no longer exists")))?;

        Ok(UserContext::background(
            user.id(),
            user.handle().to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::application::test_doubles::{InMemoryUserRepository, fixed_clock};
    use crate::identity::domain::scope::Scope;
    use crate::identity::domain::user::User;

    #[test]
    fn resolves_a_stored_user_to_a_read_write_context() {
        let users = Arc::new(InMemoryUserRepository::default());
        let alex = User::create("alex", None, fixed_clock().now()).unwrap();
        users.insert(&alex).unwrap();

        let context = BackgroundUserResolver::new(users)
            .execute(alex.id())
            .unwrap();

        assert_eq!(context.user_id(), alex.id());
        assert_eq!(context.handle(), "alex");
        assert!(context.allows(Scope::Write));
        assert!(!context.allows(Scope::Admin));
    }

    #[test]
    fn a_job_for_a_deleted_user_resolves_to_nothing() {
        // Otherwise the worker would happily write memories owned by an
        // id with no user behind it — invisible to every API, and a
        // surprise for whoever takes that id next.
        let users = Arc::new(InMemoryUserRepository::default());

        let error = BackgroundUserResolver::new(users)
            .execute(UserId::new())
            .unwrap_err();

        assert!(matches!(error, RaError::NotFound(_)), "got {error:?}");
    }
}
