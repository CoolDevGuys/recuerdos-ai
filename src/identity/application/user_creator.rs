//! Creates a user.

use crate::identity::domain::user::User;
use crate::identity::domain::user_repository::UserRepository;
use crate::shared::clock::Clock;
use crate::shared::error::Result;
use std::sync::Arc;

pub struct UserCreator {
    users: Arc<dyn UserRepository>,
    clock: Arc<dyn Clock>,
}

impl UserCreator {
    pub fn new(users: Arc<dyn UserRepository>, clock: Arc<dyn Clock>) -> Self {
        Self { users, clock }
    }

    /// Creates and persists a user.
    ///
    /// There is deliberately no "does this handle exist?" check first:
    /// between the check and the insert another process could take the
    /// handle. The UNIQUE constraint is the single source of truth, and
    /// its violation surfaces as `RaError::Conflict`.
    pub fn execute(&self, handle: &str, email: Option<&str>) -> Result<User> {
        let user = User::create(handle, email, self.clock.now())?;
        self.users.insert(&user)?;
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::application::test_doubles::{InMemoryUserRepository, fixed_clock};
    use crate::shared::error::RaError;

    fn creator(users: &Arc<InMemoryUserRepository>) -> UserCreator {
        UserCreator::new(Arc::clone(users) as Arc<dyn UserRepository>, fixed_clock())
    }

    #[test]
    fn creates_and_persists_a_user() {
        let users = Arc::new(InMemoryUserRepository::default());

        let user = creator(&users)
            .execute("alex", Some("alex@example.com"))
            .unwrap();

        assert_eq!(user.handle(), "alex");
        assert_eq!(user.created_at(), fixed_clock().now());
        assert_eq!(
            users.find_by_handle("alex").unwrap().unwrap().id(),
            user.id()
        );
    }

    #[test]
    fn rejects_a_duplicate_handle() {
        let users = Arc::new(InMemoryUserRepository::default());
        creator(&users).execute("alex", None).unwrap();

        let err = creator(&users).execute("alex", None).unwrap_err();

        assert!(matches!(err, RaError::Conflict(_)), "got {err:?}");
    }

    #[test]
    fn rejects_an_invalid_handle_before_touching_storage() {
        let users = Arc::new(InMemoryUserRepository::default());

        let err = creator(&users).execute("bad handle!", None).unwrap_err();

        assert!(matches!(err, RaError::Validation(_)), "got {err:?}");
        assert!(users.list().unwrap().is_empty(), "nothing should be stored");
    }
}
