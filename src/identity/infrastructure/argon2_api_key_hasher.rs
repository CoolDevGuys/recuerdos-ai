//! `ApiKeyHasher` using argon2id — the production implementation.
//!
//! Argon2id is memory-hard: the cost of a brute-force attempt against a
//! leaked hash is bounded by RAM, not just CPU. That same property makes
//! it slow on purpose (tens of milliseconds), which is why callers run it
//! off the async runtime's worker threads.

use crate::identity::domain::api_key_hasher::ApiKeyHasher;
use crate::shared::error::{RaError, Result};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::Rng;

/// 16 bytes is the argon2 reference implementation's recommended salt
/// length, and comfortably above `password_hash`'s 4-byte minimum.
const SALT_BYTES: usize = 16;

#[derive(Default)]
pub struct Argon2ApiKeyHasher {
    argon2: Argon2<'static>,
}

impl Argon2ApiKeyHasher {
    pub fn new() -> Self {
        // Default parameters are argon2id v19 with the OWASP-recommended
        // cost settings the crate ships; tuning them is a Phase 6 exercise
        // with a benchmark, not a guess made here.
        Self::default()
    }
}

impl ApiKeyHasher for Argon2ApiKeyHasher {
    fn hash(&self, secret: &str) -> Result<String> {
        // Salt comes from our own `rand` (OS CSPRNG) rather than argon2's
        // optional OsRng re-export, so the two crates' rand_core versions
        // can drift without breaking this.
        let mut salt_bytes = [0u8; SALT_BYTES];
        rand::rng().fill_bytes(&mut salt_bytes);
        let salt = SaltString::encode_b64(&salt_bytes)
            .map_err(|e| RaError::Internal(format!("failed to encode key salt: {e}")))?;

        self.argon2
            .hash_password(secret.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|e| RaError::Internal(format!("failed to hash API key: {e}")))
    }

    fn verify(&self, secret: &str, hash: &str) -> Result<bool> {
        let parsed = PasswordHash::new(hash)
            .map_err(|e| RaError::Internal(format!("stored key hash is unreadable: {e}")))?;

        match self.argon2.verify_password(secret.as_bytes(), &parsed) {
            Ok(()) => Ok(true),
            // A mismatch is an ordinary answer, not a failure: it is the
            // expected result whenever someone presents a wrong key.
            Err(argon2::password_hash::Error::Password) => Ok(false),
            Err(e) => Err(RaError::Internal(format!("failed to verify API key: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_a_secret_against_its_own_hash() {
        let hasher = Argon2ApiKeyHasher::new();
        let hash = hasher.hash("s3cret").unwrap();

        assert!(hasher.verify("s3cret", &hash).unwrap());
    }

    #[test]
    fn rejects_a_wrong_secret_without_erroring() {
        let hasher = Argon2ApiKeyHasher::new();
        let hash = hasher.hash("s3cret").unwrap();

        assert!(!hasher.verify("wrong", &hash).unwrap());
    }

    #[test]
    fn the_same_secret_hashes_differently_every_time() {
        let hasher = Argon2ApiKeyHasher::new();

        let first = hasher.hash("s3cret").unwrap();
        let second = hasher.hash("s3cret").unwrap();

        assert_ne!(first, second, "salt is not being applied");
        assert!(hasher.verify("s3cret", &first).unwrap());
        assert!(hasher.verify("s3cret", &second).unwrap());
    }

    #[test]
    fn the_hash_never_contains_the_secret() {
        let hasher = Argon2ApiKeyHasher::new();
        let hash = hasher.hash("s3cret").unwrap();

        assert!(!hash.contains("s3cret"), "got {hash}");
        assert!(hash.starts_with("$argon2id$"), "got {hash}");
    }

    #[test]
    fn a_corrupt_stored_hash_is_an_error_not_a_silent_pass() {
        let hasher = Argon2ApiKeyHasher::new();

        let result = hasher.verify("s3cret", "not-a-real-hash");

        assert!(
            matches!(result, Err(RaError::Internal(_))),
            "expected an error, got {result:?}"
        );
    }
}
