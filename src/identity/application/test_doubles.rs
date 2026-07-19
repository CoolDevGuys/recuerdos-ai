//! In-memory doubles used by the use-case tests.
//!
//! Compiled only under `cfg(test)`. They exist so use-case tests assert
//! orchestration (what gets stored, what gets rejected) without a
//! database, and — for the hasher — without argon2's deliberate slowness
//! turning a millisecond suite into a minute-long one.
//!
//! The repository doubles mirror the real stores' *contracts*, including
//! the UNIQUE-violation-to-`Conflict` mapping. A double that is more
//! permissive than the real thing turns green tests into false comfort.

use crate::identity::domain::api_key::ApiKey;
use crate::identity::domain::api_key_hasher::ApiKeyHasher;
use crate::identity::domain::api_key_repository::ApiKeyRepository;
use crate::identity::domain::user::User;
use crate::identity::domain::user_repository::UserRepository;
use crate::shared::clock::{Clock, FixedClock};
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{ApiKeyId, UserId};
use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};

/// A clock pinned to a fixed instant, so timestamps are assertable.
pub fn fixed_clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock::at(
        DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
    ))
}

#[derive(Default)]
pub struct InMemoryUserRepository {
    users: Mutex<Vec<User>>,
}

impl UserRepository for InMemoryUserRepository {
    fn insert(&self, user: &User) -> Result<()> {
        let mut users = self.users.lock().unwrap();
        if users.iter().any(|u| u.handle() == user.handle()) {
            return Err(RaError::Conflict(format!(
                "user {:?} already exists",
                user.handle()
            )));
        }
        users.push(user.clone());
        Ok(())
    }

    fn find_by_handle(&self, handle: &str) -> Result<Option<User>> {
        let handle = handle.trim().to_ascii_lowercase();
        Ok(self
            .users
            .lock()
            .unwrap()
            .iter()
            .find(|u| u.handle() == handle)
            .cloned())
    }

    fn find_by_id(&self, id: UserId) -> Result<Option<User>> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .iter()
            .find(|u| u.id() == id)
            .cloned())
    }

    fn list(&self) -> Result<Vec<User>> {
        let mut users = self.users.lock().unwrap().clone();
        users.sort_by(|a, b| a.handle().cmp(b.handle()));
        Ok(users)
    }
}

#[derive(Default)]
pub struct InMemoryApiKeyRepository {
    keys: Mutex<Vec<ApiKey>>,
}

impl ApiKeyRepository for InMemoryApiKeyRepository {
    fn insert(&self, key: &ApiKey) -> Result<()> {
        let mut keys = self.keys.lock().unwrap();
        if keys.iter().any(|k| k.prefix() == key.prefix()) {
            return Err(RaError::Conflict("API key already exists".to_string()));
        }
        keys.push(key.clone());
        Ok(())
    }

    fn find_by_prefix(&self, prefix: &str) -> Result<Option<ApiKey>> {
        Ok(self
            .keys
            .lock()
            .unwrap()
            .iter()
            .find(|k| k.prefix() == prefix)
            .cloned())
    }

    fn list_for_user(&self, user_id: UserId) -> Result<Vec<ApiKey>> {
        Ok(self
            .keys
            .lock()
            .unwrap()
            .iter()
            .filter(|k| k.user_id() == user_id)
            .cloned()
            .collect())
    }

    fn revoke(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<()> {
        let mut keys = self.keys.lock().unwrap();
        let key = keys
            .iter_mut()
            .find(|k| k.id() == id)
            .ok_or_else(|| RaError::NotFound(format!("API key {id} not found")))?;

        // Matches the store's COALESCE: the first revocation time wins.
        if !key.is_revoked() {
            *key = rebuild(key, key.last_used_at(), Some(now));
        }
        Ok(())
    }

    fn touch_last_used(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<()> {
        let mut keys = self.keys.lock().unwrap();
        if let Some(key) = keys.iter_mut().find(|k| k.id() == id) {
            *key = rebuild(key, Some(now), key.revoked_at());
        }
        Ok(())
    }
}

/// `ApiKey` exposes no setters — mutation goes through the store — so the
/// double rebuilds the entity from its parts.
fn rebuild(
    key: &ApiKey,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
) -> ApiKey {
    ApiKey::from_stored(
        key.id(),
        key.user_id(),
        key.name().to_string(),
        key.prefix().to_string(),
        key.secret_hash().to_string(),
        key.scopes().to_vec(),
        key.created_at(),
        last_used_at,
        revoked_at,
    )
}

/// A hasher that is instant and still asymmetric: the stored value is not
/// the secret, so a test asserting "the secret is never stored" is a real
/// assertion. Obviously not secure — it is `cfg(test)` only.
pub struct FakeApiKeyHasher;

impl ApiKeyHasher for FakeApiKeyHasher {
    fn hash(&self, secret: &str) -> Result<String> {
        Ok(format!("fake-hash:{secret}"))
    }

    fn verify(&self, secret: &str, hash: &str) -> Result<bool> {
        if !hash.starts_with("fake-hash:") {
            return Err(RaError::Internal("stored key hash is unreadable".into()));
        }
        Ok(hash == format!("fake-hash:{secret}"))
    }
}
