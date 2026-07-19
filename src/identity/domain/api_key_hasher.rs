//! Hashing contract for key secrets.
//!
//! A contract rather than a concrete call because the algorithm is an
//! infrastructure choice (argon2id today), and because tests need a fast
//! substitute: argon2 is deliberately slow, and a test suite that hashes
//! honestly on every case would take minutes instead of milliseconds.

use crate::shared::error::Result;

pub trait ApiKeyHasher: Send + Sync {
    /// Hashes a key secret for storage. Includes its own random salt, so
    /// two identical secrets produce different hashes.
    fn hash(&self, secret: &str) -> Result<String>;

    /// Verifies a presented secret against a stored hash. Returns
    /// `Ok(false)` for a mismatch; `Err` only when the stored hash itself
    /// is unreadable (corruption, or an algorithm we no longer support).
    fn verify(&self, secret: &str, hash: &str) -> Result<bool>;
}
