//! Remembers recently verified API keys, so argon2 runs once per key
//! rather than once per request.
//!
//! # Why this exists
//!
//! argon2id is deliberately expensive — that is the point of it for
//! password storage. Measured on this service it costs ~230 ms, which
//! made every authenticated request ~250 ms even though the work being
//! authorised (a hybrid recall) takes 8 ms. Auth was 96% of the response
//! time, and the sub-50 ms recall target was unreachable while every
//! request paid it.
//!
//! API keys differ from passwords in a way that makes caching sound:
//! they are 128-bit random secrets, not human-chosen. The slow hash
//! exists to protect a *stolen database*, and it still does — the stored
//! hash is unchanged. It was never load-bearing against online guessing,
//! where the entropy does the work.
//!
//! # What is cached, and what is not
//!
//! Only the expensive verification is cached, keyed by a SHA-256 digest
//! of the presented secret. Every request still reads the key row and
//! re-checks revocation and scopes, so:
//!
//! - revoking a key takes effect immediately, not after a TTL;
//! - the plaintext secret is never held (a digest is), so the cache adds
//!   no secret to memory that the request didn't already carry.
//!
//! The TTL bounds how long a *verification* is reused, which caps the
//! window in which a cache entry could outlive a change to the hashing
//! parameters themselves.

use crate::identity::domain::api_key::ApiKeyToken;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a verification may be reused. Short enough to bound staleness,
/// long enough that a busy agent session pays argon2 once.
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// Bounds memory use, and makes the cache useless as an amplification
/// target: a flood of distinct bad keys evicts rather than grows.
const MAX_ENTRIES: usize = 1_024;

type SecretDigest = [u8; 32];

struct Entry {
    digest: SecretDigest,
    verified_at: Instant,
}

pub struct VerifiedKeyCache {
    entries: Mutex<HashMap<String, Entry>>,
    ttl: Duration,
}

impl Default for VerifiedKeyCache {
    fn default() -> Self {
        Self::new(DEFAULT_TTL)
    }
}

impl VerifiedKeyCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Whether this exact secret was verified for this prefix recently.
    pub fn is_verified(&self, token: &ApiKeyToken) -> bool {
        let digest = digest_of(token);
        let entries = self.entries.lock().expect("verified key cache poisoned");

        entries.get(token.prefix()).is_some_and(|entry| {
            entry.verified_at.elapsed() < self.ttl
                    // Constant-time comparison: a timing side channel here
                    // would leak the digest a byte at a time.
                    && constant_time_eq(&entry.digest, &digest)
        })
    }

    /// Records a successful argon2 verification.
    pub fn remember(&self, token: &ApiKeyToken) {
        let mut entries = self.entries.lock().expect("verified key cache poisoned");

        if entries.len() >= MAX_ENTRIES {
            let now = Instant::now();
            entries.retain(|_, entry| now.duration_since(entry.verified_at) < self.ttl);
            // Still full of live entries: drop everything rather than
            // grow without bound. Costs one argon2 per key afterwards.
            if entries.len() >= MAX_ENTRIES {
                entries.clear();
            }
        }

        entries.insert(
            token.prefix().to_string(),
            Entry {
                digest: digest_of(token),
                verified_at: Instant::now(),
            },
        );
    }

    /// Drops a cached verification. Not needed for revocation (which is
    /// re-checked per request), but keeps the cache honest when a key is
    /// known to be gone.
    pub fn forget(&self, prefix: &str) {
        self.entries
            .lock()
            .expect("verified key cache poisoned")
            .remove(prefix);
    }
}

fn digest_of(token: &ApiKeyToken) -> SecretDigest {
    let mut hasher = Sha256::new();
    hasher.update(token.secret().as_bytes());
    hasher.finalize().into()
}

fn constant_time_eq(a: &SecretDigest, b: &SecretDigest) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> ApiKeyToken {
        ApiKeyToken::generate()
    }

    #[test]
    fn an_unseen_key_is_not_verified() {
        let cache = VerifiedKeyCache::default();
        assert!(!cache.is_verified(&token()));
    }

    #[test]
    fn a_remembered_key_is_verified() {
        let cache = VerifiedKeyCache::default();
        let token = token();

        cache.remember(&token);

        assert!(cache.is_verified(&token));
    }

    #[test]
    fn a_different_secret_under_the_same_prefix_is_not_verified() {
        // The attack this must stop: reusing a cached verification by
        // presenting the right prefix with a wrong secret.
        let cache = VerifiedKeyCache::default();
        let real = token();
        cache.remember(&real);

        let forged =
            ApiKeyToken::parse(&format!("ra_live_{}{}", real.prefix(), "0".repeat(32))).unwrap();

        assert!(!cache.is_verified(&forged));
    }

    #[test]
    fn an_expired_entry_is_not_verified() {
        let cache = VerifiedKeyCache::new(Duration::from_millis(0));
        let token = token();

        cache.remember(&token);

        assert!(
            !cache.is_verified(&token),
            "a zero TTL should expire at once"
        );
    }

    #[test]
    fn forgetting_a_prefix_drops_its_verification() {
        let cache = VerifiedKeyCache::default();
        let token = token();
        cache.remember(&token);

        cache.forget(token.prefix());

        assert!(!cache.is_verified(&token));
    }

    #[test]
    fn the_cache_does_not_grow_without_bound() {
        let cache = VerifiedKeyCache::default();

        for _ in 0..MAX_ENTRIES * 2 {
            cache.remember(&token());
        }

        let size = cache.entries.lock().unwrap().len();
        assert!(size <= MAX_ENTRIES, "cache grew to {size}");
    }

    #[test]
    fn entries_survive_eviction_pressure_for_the_most_recent_key() {
        let cache = VerifiedKeyCache::default();
        for _ in 0..MAX_ENTRIES * 2 {
            cache.remember(&token());
        }

        let latest = token();
        cache.remember(&latest);

        assert!(cache.is_verified(&latest));
    }
}
