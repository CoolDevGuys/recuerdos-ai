//! Turns a presented API key into a `UserContext`.
//!
//! This is the gate every authenticated request passes through, and the
//! only place outside the domain that produces a `UserContext`.

use crate::identity::application::verified_key_cache::VerifiedKeyCache;
use crate::identity::domain::api_key::ApiKeyToken;
use crate::identity::domain::api_key_hasher::ApiKeyHasher;
use crate::identity::domain::api_key_repository::ApiKeyRepository;
use crate::identity::domain::user_context::UserContext;
use crate::identity::domain::user_repository::UserRepository;
use crate::shared::error::{RaError, Result};
use std::sync::Arc;

pub struct KeyAuthenticator {
    users: Arc<dyn UserRepository>,
    keys: Arc<dyn ApiKeyRepository>,
    hasher: Arc<dyn ApiKeyHasher>,
    verified: VerifiedKeyCache,
}

impl KeyAuthenticator {
    pub fn new(
        users: Arc<dyn UserRepository>,
        keys: Arc<dyn ApiKeyRepository>,
        hasher: Arc<dyn ApiKeyHasher>,
    ) -> Self {
        Self {
            users,
            keys,
            hasher,
            verified: VerifiedKeyCache::default(),
        }
    }

    /// Authenticates a raw key.
    ///
    /// Every failure — malformed, unknown prefix, wrong secret, revoked,
    /// orphaned — returns the same `Unauthorized("invalid API key")`.
    /// Distinguishing "no such key" from "wrong secret" would tell a
    /// caller which half of a guess was right.
    ///
    /// Blocking: the argon2 verify costs tens of milliseconds. Async
    /// callers must run this on a blocking thread.
    pub fn execute(&self, raw_key: &str) -> Result<UserContext> {
        let token = ApiKeyToken::parse(raw_key)?;

        let key = self
            .keys
            .find_by_prefix(token.prefix())?
            .ok_or_else(invalid_key)?;

        // A recent verification of this exact secret lets us skip argon2,
        // which costs ~230 ms and dominated every request. Revocation and
        // scopes are still read from the row below, so revoking a key
        // takes effect immediately rather than after the cache TTL.
        let matches = if self.verified.is_verified(&token) {
            true
        } else {
            // Verify before checking revocation, so a revoked key and an
            // unknown one cost the same. Skipping the hash for revoked
            // keys would make revocation detectable by timing alone.
            let verified = self.hasher.verify(token.secret(), key.secret_hash())?;
            if verified {
                self.verified.remember(&token);
            }
            verified
        };

        if !matches || key.is_revoked() {
            if key.is_revoked() {
                self.verified.forget(token.prefix());
            }
            return Err(invalid_key());
        }

        let user = self.users.find_by_id(key.user_id())?.ok_or_else(|| {
            // The FK makes this unreachable in practice; if it ever
            // happens the key is unusable, and saying why would leak.
            tracing::error!(key_id = %key.id(), "API key references a missing user");
            invalid_key()
        })?;

        Ok(UserContext::authenticated(
            user.id(),
            user.handle().to_string(),
            key.id(),
            key.scopes().to_vec(),
        ))
    }
}

fn invalid_key() -> RaError {
    RaError::Unauthorized("invalid API key".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::application::api_key_issuer::{ApiKeyIssuer, IssuedApiKey};
    use crate::identity::application::test_doubles::{
        FakeApiKeyHasher, InMemoryApiKeyRepository, InMemoryUserRepository, fixed_clock,
    };
    use crate::identity::domain::scope::Scope;
    use crate::identity::domain::user::User;

    struct Fixture {
        users: Arc<InMemoryUserRepository>,
        keys: Arc<InMemoryApiKeyRepository>,
        hasher: Arc<FakeApiKeyHasher>,
        issuer: ApiKeyIssuer,
        authenticator: KeyAuthenticator,
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
            authenticator: KeyAuthenticator::new(
                Arc::clone(&users) as Arc<dyn UserRepository>,
                Arc::clone(&keys) as Arc<dyn ApiKeyRepository>,
                Arc::clone(&hasher) as Arc<dyn ApiKeyHasher>,
            ),
            users,
            keys,
            hasher,
        }
    }

    fn issue(fixture: &Fixture, handle: &str, scopes: Vec<Scope>) -> IssuedApiKey {
        let user = User::create(handle, None, fixed_clock().now()).unwrap();
        fixture.users.insert(&user).unwrap();
        fixture.issuer.execute(handle, scopes, "laptop").unwrap()
    }

    fn assert_rejected(result: Result<UserContext>) {
        match result {
            Err(RaError::Unauthorized(message)) => assert_eq!(message, "invalid API key"),
            other => panic!("expected an opaque rejection, got {other:?}"),
        }
    }

    #[test]
    fn authenticates_a_valid_key() {
        let fixture = fixture();
        let issued = issue(&fixture, "alex", vec![Scope::Read, Scope::Write]);

        let ctx = fixture
            .authenticator
            .execute(&issued.token.render())
            .unwrap();

        assert_eq!(ctx.user_id(), issued.key.user_id());
        assert_eq!(ctx.handle(), "alex");
        assert_eq!(ctx.key_id(), Some(issued.key.id()));
        assert!(ctx.allows(Scope::Read));
        assert!(ctx.allows(Scope::Write));
    }

    #[test]
    fn the_context_carries_only_the_scopes_the_key_was_issued_with() {
        let fixture = fixture();
        let issued = issue(&fixture, "alex", vec![Scope::Read]);

        let ctx = fixture
            .authenticator
            .execute(&issued.token.render())
            .unwrap();

        assert!(ctx.allows(Scope::Read));
        assert!(!ctx.allows(Scope::Write));
    }

    #[test]
    fn rejects_a_malformed_key() {
        let fixture = fixture();
        assert_rejected(fixture.authenticator.execute("not-a-key"));
        assert_rejected(fixture.authenticator.execute(""));
    }

    #[test]
    fn rejects_an_unknown_prefix() {
        let fixture = fixture();
        issue(&fixture, "alex", vec![Scope::Read]);

        let unknown = format!("ra_live_deadbeef{}", "0".repeat(32));
        assert_rejected(fixture.authenticator.execute(&unknown));
    }

    #[test]
    fn rejects_a_valid_prefix_with_the_wrong_secret() {
        let fixture = fixture();
        let issued = issue(&fixture, "alex", vec![Scope::Read]);

        let forged = format!("ra_live_{}{}", issued.token.prefix(), "0".repeat(32));
        assert_rejected(fixture.authenticator.execute(&forged));
    }

    #[test]
    fn rejects_a_revoked_key() {
        let fixture = fixture();
        let issued = issue(&fixture, "alex", vec![Scope::Read]);
        let raw = issued.token.render();
        assert!(fixture.authenticator.execute(&raw).is_ok());

        fixture
            .keys
            .revoke(issued.key.id(), fixed_clock().now())
            .unwrap();

        assert_rejected(fixture.authenticator.execute(&raw));
    }

    #[test]
    fn rejects_every_failure_mode_identically() {
        let fixture = fixture();
        let issued = issue(&fixture, "alex", vec![Scope::Read]);
        fixture
            .keys
            .revoke(issued.key.id(), fixed_clock().now())
            .unwrap();

        // Malformed, unknown, wrong-secret and revoked must be
        // indistinguishable to a caller probing for valid keys.
        let messages: Vec<String> = [
            "garbage".to_string(),
            format!("ra_live_deadbeef{}", "0".repeat(32)),
            format!("ra_live_{}{}", issued.token.prefix(), "1".repeat(32)),
            issued.token.render(),
        ]
        .iter()
        .map(|raw| fixture.authenticator.execute(raw).unwrap_err().to_string())
        .collect();

        assert!(
            messages.windows(2).all(|pair| pair[0] == pair[1]),
            "rejection messages differ between failure modes: {messages:?}"
        );
    }

    #[test]
    fn keys_of_different_users_resolve_to_their_own_owner() {
        let fixture = fixture();
        let alex = issue(&fixture, "alex", vec![Scope::Read]);
        let sam = issue(&fixture, "sam", vec![Scope::Read]);

        let alex_ctx = fixture.authenticator.execute(&alex.token.render()).unwrap();
        let sam_ctx = fixture.authenticator.execute(&sam.token.render()).unwrap();

        assert_eq!(alex_ctx.handle(), "alex");
        assert_eq!(sam_ctx.handle(), "sam");
        assert_ne!(alex_ctx.user_id(), sam_ctx.user_id());
    }

    #[test]
    fn a_revoked_key_is_rejected_even_after_a_successful_verification() {
        // The cache must not outlive revocation. This is the test that
        // makes skipping argon2 safe: the key is verified (and cached),
        // then revoked, then presented again.
        let fixture = fixture();
        let issued = issue(&fixture, "alex", vec![Scope::Read]);
        let raw = issued.token.render();

        assert!(
            fixture.authenticator.execute(&raw).is_ok(),
            "first call primes the cache"
        );

        fixture
            .keys
            .revoke(issued.key.id(), fixed_clock().now())
            .unwrap();

        assert_rejected(fixture.authenticator.execute(&raw));
    }

    #[test]
    fn a_cached_prefix_does_not_admit_a_different_secret() {
        // Presenting a valid prefix with the wrong secret must still fail
        // after that prefix has been verified once.
        let fixture = fixture();
        let issued = issue(&fixture, "alex", vec![Scope::Read]);
        assert!(
            fixture
                .authenticator
                .execute(&issued.token.render())
                .is_ok()
        );

        let forged = format!("ra_live_{}{}", issued.token.prefix(), "0".repeat(32));

        assert_rejected(fixture.authenticator.execute(&forged));
    }

    #[test]
    fn scope_changes_are_not_masked_by_the_cache() {
        // Scopes come from the row on every call, not from the cache.
        let fixture = fixture();
        let issued = issue(&fixture, "alex", vec![Scope::Read]);
        let raw = issued.token.render();

        let first = fixture.authenticator.execute(&raw).unwrap();
        assert!(first.allows(Scope::Read) && !first.allows(Scope::Write));

        let second = fixture.authenticator.execute(&raw).unwrap();
        assert!(second.allows(Scope::Read) && !second.allows(Scope::Write));
    }

    #[test]
    fn the_expensive_hash_runs_once_per_key_not_once_per_request() {
        let fixture = fixture();
        let issued = issue(&fixture, "alex", vec![Scope::Read]);
        let raw = issued.token.render();
        let before = fixture.hasher.verify_calls();

        for _ in 0..5 {
            fixture.authenticator.execute(&raw).unwrap();
        }

        assert_eq!(
            fixture.hasher.verify_calls() - before,
            1,
            "argon2 should run once, then be served from the cache"
        );
    }
}
