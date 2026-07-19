//! Issues an API key for a user.

use crate::identity::domain::api_key::{ApiKey, ApiKeyToken};
use crate::identity::domain::api_key_hasher::ApiKeyHasher;
use crate::identity::domain::api_key_repository::ApiKeyRepository;
use crate::identity::domain::scope::Scope;
use crate::identity::domain::user_repository::UserRepository;
use crate::shared::clock::Clock;
use crate::shared::error::{RaError, Result};
use std::sync::Arc;

/// A freshly issued key plus its plaintext token.
///
/// The token exists only in this value, and only until the caller drops
/// it: the store keeps a hash. This is the one and only moment it can be
/// shown to a user.
///
/// `Debug` is safe to derive: `ApiKeyToken`'s own `Debug` redacts the
/// secret, so printing this value cannot leak it.
#[derive(Debug)]
pub struct IssuedApiKey {
    pub key: ApiKey,
    pub token: ApiKeyToken,
}

pub struct ApiKeyIssuer {
    users: Arc<dyn UserRepository>,
    keys: Arc<dyn ApiKeyRepository>,
    hasher: Arc<dyn ApiKeyHasher>,
    clock: Arc<dyn Clock>,
}

impl ApiKeyIssuer {
    pub fn new(
        users: Arc<dyn UserRepository>,
        keys: Arc<dyn ApiKeyRepository>,
        hasher: Arc<dyn ApiKeyHasher>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            users,
            keys,
            hasher,
            clock,
        }
    }

    pub fn execute(&self, handle: &str, scopes: Vec<Scope>, name: &str) -> Result<IssuedApiKey> {
        let user = self
            .users
            .find_by_handle(handle)?
            .ok_or_else(|| RaError::NotFound(format!("user {handle:?} not found")))?;

        let token = ApiKeyToken::generate();
        let secret_hash = self.hasher.hash(token.secret())?;

        let key = ApiKey::issue(
            user.id(),
            name,
            token.prefix(),
            secret_hash,
            scopes,
            self.clock.now(),
        )?;
        self.keys.insert(&key)?;

        Ok(IssuedApiKey { key, token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::application::test_doubles::{
        FakeApiKeyHasher, InMemoryApiKeyRepository, InMemoryUserRepository, fixed_clock,
    };
    use crate::identity::domain::user::User;

    struct Fixture {
        users: Arc<InMemoryUserRepository>,
        keys: Arc<InMemoryApiKeyRepository>,
        hasher: Arc<FakeApiKeyHasher>,
        issuer: ApiKeyIssuer,
    }

    fn fixture() -> Fixture {
        let users = Arc::new(InMemoryUserRepository::default());
        let keys = Arc::new(InMemoryApiKeyRepository::default());
        let hasher = Arc::new(FakeApiKeyHasher::default());

        let issuer = ApiKeyIssuer::new(
            Arc::clone(&users) as Arc<dyn UserRepository>,
            Arc::clone(&keys) as Arc<dyn ApiKeyRepository>,
            Arc::clone(&hasher) as Arc<dyn ApiKeyHasher>,
            fixed_clock(),
        );

        Fixture {
            users,
            keys,
            hasher,
            issuer,
        }
    }

    fn with_user(fixture: &Fixture, handle: &str) -> User {
        let user = User::create(handle, None, fixed_clock().now()).unwrap();
        fixture.users.insert(&user).unwrap();
        user
    }

    #[test]
    fn issues_a_key_bound_to_the_user() {
        let fixture = fixture();
        let user = with_user(&fixture, "alex");

        let issued = fixture
            .issuer
            .execute("alex", vec![Scope::Read, Scope::Write], "laptop")
            .unwrap();

        assert_eq!(issued.key.user_id(), user.id());
        assert_eq!(issued.key.name(), "laptop");
        assert_eq!(issued.key.scopes(), &[Scope::Read, Scope::Write]);
        assert_eq!(issued.key.prefix(), issued.token.prefix());
    }

    #[test]
    fn stores_only_a_hash_never_the_secret() {
        let fixture = fixture();
        with_user(&fixture, "alex");

        let issued = fixture
            .issuer
            .execute("alex", vec![Scope::Read], "laptop")
            .unwrap();

        let stored = fixture
            .keys
            .find_by_prefix(issued.token.prefix())
            .unwrap()
            .unwrap();
        assert_ne!(stored.secret_hash(), issued.token.secret());
        assert!(
            fixture
                .hasher
                .verify(issued.token.secret(), stored.secret_hash())
                .unwrap()
        );
    }

    #[test]
    fn every_key_gets_a_distinct_token() {
        let fixture = fixture();
        with_user(&fixture, "alex");

        let first = fixture
            .issuer
            .execute("alex", vec![Scope::Read], "laptop")
            .unwrap();
        let second = fixture
            .issuer
            .execute("alex", vec![Scope::Read], "desktop")
            .unwrap();

        assert_ne!(first.token.render(), second.token.render());
        assert_ne!(first.key.prefix(), second.key.prefix());
    }

    #[test]
    fn refuses_to_issue_for_an_unknown_user() {
        let fixture = fixture();

        let err = fixture
            .issuer
            .execute("nobody", vec![Scope::Read], "laptop")
            .unwrap_err();

        assert!(matches!(err, RaError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn rejects_an_empty_key_name() {
        let fixture = fixture();
        with_user(&fixture, "alex");

        let err = fixture
            .issuer
            .execute("alex", vec![Scope::Read], "  ")
            .unwrap_err();

        assert!(matches!(err, RaError::Validation(_)), "got {err:?}");
    }
}
