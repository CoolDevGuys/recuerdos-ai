//! Revokes an API key by its prefix.

use crate::identity::domain::api_key_repository::ApiKeyRepository;
use crate::shared::clock::Clock;
use crate::shared::error::{RaError, Result};
use std::sync::Arc;

pub struct ApiKeyRevoker {
    keys: Arc<dyn ApiKeyRepository>,
    clock: Arc<dyn Clock>,
}

impl ApiKeyRevoker {
    pub fn new(keys: Arc<dyn ApiKeyRepository>, clock: Arc<dyn Clock>) -> Self {
        Self { keys, clock }
    }

    /// Revokes the key with this prefix.
    ///
    /// Keys are identified by prefix rather than id because the prefix is
    /// what a user can actually see — it is printed by `key list` and is
    /// the visible half of the key itself. Revoking is idempotent.
    pub fn execute(&self, prefix: &str) -> Result<()> {
        let prefix = prefix.trim().to_ascii_lowercase();

        let key = self
            .keys
            .find_by_prefix(&prefix)?
            .ok_or_else(|| RaError::NotFound(format!("no API key with prefix {prefix:?}")))?;

        self.keys.revoke(key.id(), self.clock.now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::application::test_doubles::{InMemoryApiKeyRepository, fixed_clock};
    use crate::identity::domain::api_key::ApiKey;
    use crate::identity::domain::scope::Scope;
    use crate::shared::ids::UserId;

    fn fixture() -> (Arc<InMemoryApiKeyRepository>, ApiKeyRevoker) {
        let keys = Arc::new(InMemoryApiKeyRepository::default());
        let revoker = ApiKeyRevoker::new(
            Arc::clone(&keys) as Arc<dyn ApiKeyRepository>,
            fixed_clock(),
        );
        (keys, revoker)
    }

    fn stored_key(keys: &Arc<InMemoryApiKeyRepository>, prefix: &str) -> ApiKey {
        let key = ApiKey::issue(
            UserId::new(),
            "laptop",
            prefix,
            "fake-hash:s".to_string(),
            vec![Scope::Read],
            fixed_clock().now(),
        )
        .unwrap();
        keys.insert(&key).unwrap();
        key
    }

    #[test]
    fn revokes_a_key_by_prefix() {
        let (keys, revoker) = fixture();
        stored_key(&keys, "1f4c8a20");

        revoker.execute("1f4c8a20").unwrap();

        assert!(
            keys.find_by_prefix("1f4c8a20")
                .unwrap()
                .unwrap()
                .is_revoked()
        );
    }

    #[test]
    fn revoking_is_idempotent() {
        let (keys, revoker) = fixture();
        stored_key(&keys, "1f4c8a20");

        revoker.execute("1f4c8a20").unwrap();
        revoker.execute("1f4c8a20").unwrap();

        assert!(
            keys.find_by_prefix("1f4c8a20")
                .unwrap()
                .unwrap()
                .is_revoked()
        );
    }

    #[test]
    fn reports_an_unknown_prefix() {
        let (_keys, revoker) = fixture();

        let err = revoker.execute("deadbeef").unwrap_err();

        assert!(matches!(err, RaError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn revoking_one_key_leaves_the_others_alone() {
        let (keys, revoker) = fixture();
        stored_key(&keys, "1f4c8a20");
        stored_key(&keys, "aabbccdd");

        revoker.execute("1f4c8a20").unwrap();

        assert!(
            !keys
                .find_by_prefix("aabbccdd")
                .unwrap()
                .unwrap()
                .is_revoked()
        );
    }
}
