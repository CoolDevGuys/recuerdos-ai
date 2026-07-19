//! Lists a user's API keys (metadata only — secrets are unrecoverable).

use crate::identity::domain::api_key::ApiKey;
use crate::identity::domain::api_key_repository::ApiKeyRepository;
use crate::identity::domain::user_repository::UserRepository;
use crate::shared::error::{RaError, Result};
use std::sync::Arc;

pub struct ApiKeyLister {
    users: Arc<dyn UserRepository>,
    keys: Arc<dyn ApiKeyRepository>,
}

impl ApiKeyLister {
    pub fn new(users: Arc<dyn UserRepository>, keys: Arc<dyn ApiKeyRepository>) -> Self {
        Self { users, keys }
    }

    /// Lists the keys belonging to `handle`, revoked ones included — an
    /// audit view is only useful if it shows what was withdrawn, not just
    /// what is live.
    pub fn execute(&self, handle: &str) -> Result<Vec<ApiKey>> {
        let user = self
            .users
            .find_by_handle(handle)?
            .ok_or_else(|| RaError::NotFound(format!("user {handle:?} not found")))?;

        self.keys.list_for_user(user.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::application::api_key_issuer::ApiKeyIssuer;
    use crate::identity::application::api_key_revoker::ApiKeyRevoker;
    use crate::identity::application::test_doubles::{
        FakeApiKeyHasher, InMemoryApiKeyRepository, InMemoryUserRepository, fixed_clock,
    };
    use crate::identity::domain::api_key_hasher::ApiKeyHasher;
    use crate::identity::domain::scope::Scope;
    use crate::identity::domain::user::User;

    struct Fixture {
        users: Arc<InMemoryUserRepository>,
        issuer: ApiKeyIssuer,
        revoker: ApiKeyRevoker,
        lister: ApiKeyLister,
    }

    fn fixture() -> Fixture {
        let users = Arc::new(InMemoryUserRepository::default());
        let keys = Arc::new(InMemoryApiKeyRepository::default());
        let hasher = Arc::new(FakeApiKeyHasher::default());

        Fixture {
            issuer: ApiKeyIssuer::new(
                Arc::clone(&users) as Arc<dyn UserRepository>,
                Arc::clone(&keys) as Arc<dyn ApiKeyRepository>,
                Arc::clone(&hasher) as Arc<dyn ApiKeyHasher>,
                fixed_clock(),
            ),
            revoker: ApiKeyRevoker::new(
                Arc::clone(&keys) as Arc<dyn ApiKeyRepository>,
                fixed_clock(),
            ),
            lister: ApiKeyLister::new(
                Arc::clone(&users) as Arc<dyn UserRepository>,
                Arc::clone(&keys) as Arc<dyn ApiKeyRepository>,
            ),
            users,
        }
    }

    fn with_user(fixture: &Fixture, handle: &str) {
        let user = User::create(handle, None, fixed_clock().now()).unwrap();
        fixture.users.insert(&user).unwrap();
    }

    #[test]
    fn lists_a_users_keys() {
        let fixture = fixture();
        with_user(&fixture, "alex");
        fixture
            .issuer
            .execute("alex", vec![Scope::Read], "laptop")
            .unwrap();
        fixture
            .issuer
            .execute("alex", vec![Scope::Write], "ci")
            .unwrap();

        let keys = fixture.lister.execute("alex").unwrap();

        let names: Vec<&str> = keys.iter().map(|k| k.name()).collect();
        assert_eq!(names, vec!["laptop", "ci"]);
    }

    #[test]
    fn never_lists_another_users_keys() {
        let fixture = fixture();
        with_user(&fixture, "alex");
        with_user(&fixture, "sam");
        fixture
            .issuer
            .execute("alex", vec![Scope::Read], "alex-laptop")
            .unwrap();
        let sam_key = fixture
            .issuer
            .execute("sam", vec![Scope::Read], "sam-laptop")
            .unwrap();

        let alex_keys = fixture.lister.execute("alex").unwrap();

        assert_eq!(alex_keys.len(), 1);
        assert!(
            alex_keys.iter().all(|k| k.prefix() != sam_key.key.prefix()),
            "another user's key leaked into the listing"
        );
    }

    #[test]
    fn includes_revoked_keys() {
        let fixture = fixture();
        with_user(&fixture, "alex");
        let issued = fixture
            .issuer
            .execute("alex", vec![Scope::Read], "laptop")
            .unwrap();
        fixture.revoker.execute(issued.key.prefix()).unwrap();

        let keys = fixture.lister.execute("alex").unwrap();

        assert_eq!(keys.len(), 1);
        assert!(keys[0].is_revoked());
    }

    #[test]
    fn a_user_with_no_keys_lists_empty() {
        let fixture = fixture();
        with_user(&fixture, "alex");

        assert!(fixture.lister.execute("alex").unwrap().is_empty());
    }

    #[test]
    fn reports_an_unknown_user() {
        let fixture = fixture();

        let err = fixture.lister.execute("nobody").unwrap_err();

        assert!(matches!(err, RaError::NotFound(_)), "got {err:?}");
    }
}
