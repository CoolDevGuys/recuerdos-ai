//! Resolves the built-in user for `[auth].mode = "none"`.
//!
//! Opting out of authentication does not opt out of *ownership*: memories
//! still belong to a user, every store is still scoped by `user_id`, and
//! turning auth back on later must not orphan the data written while it
//! was off. So an unauthenticated deployment gets a real, persisted user
//! named `default` — it simply doesn't have to prove it is that user.

use crate::identity::domain::user::User;
use crate::identity::domain::user_context::UserContext;
use crate::identity::domain::user_repository::UserRepository;
use crate::shared::clock::Clock;
use crate::shared::error::Result;
use std::sync::Arc;

pub const DEFAULT_USER_HANDLE: &str = "default";

pub struct DefaultUserResolver {
    users: Arc<dyn UserRepository>,
    clock: Arc<dyn Clock>,
}

impl DefaultUserResolver {
    pub fn new(users: Arc<dyn UserRepository>, clock: Arc<dyn Clock>) -> Self {
        Self { users, clock }
    }

    /// Returns a context for the `default` user, creating that user on
    /// first use. Idempotent, and safe if two requests race: the loser of
    /// the insert re-reads the winner's row rather than failing.
    pub fn execute(&self) -> Result<UserContext> {
        if let Some(user) = self.users.find_by_handle(DEFAULT_USER_HANDLE)? {
            return Ok(context_for(&user));
        }

        let user = User::create(DEFAULT_USER_HANDLE, None, self.clock.now())?;
        match self.users.insert(&user) {
            Ok(()) => Ok(context_for(&user)),
            Err(_conflict) => {
                // Another request created it between our read and write.
                let user = self
                    .users
                    .find_by_handle(DEFAULT_USER_HANDLE)?
                    .ok_or_else(|| {
                        crate::shared::error::RaError::Internal(
                            "default user vanished after a conflicting insert".to_string(),
                        )
                    })?;
                Ok(context_for(&user))
            }
        }
    }
}

fn context_for(user: &User) -> UserContext {
    UserContext::unauthenticated(user.id(), user.handle().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::application::test_doubles::{InMemoryUserRepository, fixed_clock};
    use crate::identity::domain::scope::Scope;

    fn resolver(users: &Arc<InMemoryUserRepository>) -> DefaultUserResolver {
        DefaultUserResolver::new(Arc::clone(users) as Arc<dyn UserRepository>, fixed_clock())
    }

    #[test]
    fn creates_the_default_user_on_first_use() {
        let users = Arc::new(InMemoryUserRepository::default());

        let ctx = resolver(&users).execute().unwrap();

        assert_eq!(ctx.handle(), DEFAULT_USER_HANDLE);
        assert!(users.find_by_handle(DEFAULT_USER_HANDLE).unwrap().is_some());
    }

    #[test]
    fn reuses_the_same_user_across_calls() {
        let users = Arc::new(InMemoryUserRepository::default());

        let first = resolver(&users).execute().unwrap();
        let second = resolver(&users).execute().unwrap();

        assert_eq!(
            first.user_id(),
            second.user_id(),
            "each call minted a new user — data written earlier would be orphaned"
        );
        assert_eq!(users.list().unwrap().len(), 1);
    }

    #[test]
    fn the_context_carries_no_key_but_full_access() {
        let users = Arc::new(InMemoryUserRepository::default());

        let ctx = resolver(&users).execute().unwrap();

        assert_eq!(ctx.key_id(), None);
        assert!(ctx.allows(Scope::Read));
        assert!(ctx.allows(Scope::Write));
    }

    #[test]
    fn does_not_disturb_existing_users() {
        let users = Arc::new(InMemoryUserRepository::default());
        let alex = User::create("alex", None, fixed_clock().now()).unwrap();
        users.insert(&alex).unwrap();

        let ctx = resolver(&users).execute().unwrap();

        assert_ne!(ctx.user_id(), alex.id());
        assert_eq!(users.list().unwrap().len(), 2);
    }
}
