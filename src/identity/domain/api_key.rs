//! API keys: the credential that resolves a request to exactly one user.
//!
//! # Key format
//!
//! ```text
//! ra_live_1f4c8a20 9d4e5f6a7b8c9d0e1f2a3b4c5d6e7f80
//! └──┬───┘└───┬──┘ └──────────────┬──────────────┘
//!  scheme  prefix (8 hex)     secret (32 hex)
//! ```
//!
//! The **prefix** is stored in plaintext and indexed: it turns
//! authentication into one indexed lookup instead of an argon2 verify
//! against every key in the table. It carries no authority on its own.
//!
//! The **secret** is never stored — only its argon2 hash. A leaked
//! database therefore does not yield usable keys, and a key can only ever
//! be displayed once, at issue time.
//!
//! Hashing and verification are *not* methods on `ApiKey`: argon2 is an
//! infrastructure concern, so it lives behind [`ApiKeyHasher`] and is
//! applied by the `KeyAuthenticator` use case.
//!
//! [`ApiKeyHasher`]: super::api_key_hasher::ApiKeyHasher

use super::scope::Scope;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{ApiKeyId, UserId};
use chrono::{DateTime, Utc};
use rand::Rng;
use std::fmt;

const KEY_SCHEME: &str = "ra_live_";
const PREFIX_BYTES: usize = 4; // 8 hex chars
const SECRET_BYTES: usize = 16; // 32 hex chars
const PREFIX_HEX_LEN: usize = PREFIX_BYTES * 2;
const SECRET_HEX_LEN: usize = SECRET_BYTES * 2;

/// A parsed or freshly generated key, held in plaintext.
///
/// Only ever exists in memory: at issue time (to show the user once) and
/// during authentication (to verify against a stored hash). `Debug`
/// redacts the secret so it cannot reach a log line by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKeyToken {
    prefix: String,
    secret: String,
}

impl ApiKeyToken {
    /// Generates a new random token from the OS CSPRNG.
    pub fn generate() -> Self {
        let mut prefix_bytes = [0u8; PREFIX_BYTES];
        let mut secret_bytes = [0u8; SECRET_BYTES];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut prefix_bytes);
        rng.fill_bytes(&mut secret_bytes);

        Self {
            prefix: hex::encode(prefix_bytes),
            secret: hex::encode(secret_bytes),
        }
    }

    /// Parses a key as presented by a client.
    ///
    /// Every rejection returns the same opaque error: telling a caller
    /// *why* their key is malformed is free information for someone
    /// probing the format.
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        let body = raw.strip_prefix(KEY_SCHEME).ok_or_else(malformed)?;

        if body.len() != PREFIX_HEX_LEN + SECRET_HEX_LEN {
            return Err(malformed());
        }
        if !body.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(malformed());
        }

        let (prefix, secret) = body.split_at(PREFIX_HEX_LEN);
        Ok(Self {
            prefix: prefix.to_ascii_lowercase(),
            secret: secret.to_ascii_lowercase(),
        })
    }

    /// The indexed, non-secret lookup key.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The part that must be hashed and verified. Keep it out of logs.
    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// The full key as the user must store it. Displayed exactly once.
    pub fn render(&self) -> String {
        format!("{KEY_SCHEME}{}{}", self.prefix, self.secret)
    }
}

fn malformed() -> RaError {
    RaError::Unauthorized("invalid API key".to_string())
}

impl fmt::Debug for ApiKeyToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Prefix is safe to show and is what appears in `key list`, so it
        // stays useful for debugging; the secret never does.
        write!(f, "ApiKeyToken({}{}…redacted)", KEY_SCHEME, self.prefix)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    id: ApiKeyId,
    user_id: UserId,
    name: String,
    prefix: String,
    secret_hash: String,
    scopes: Vec<Scope>,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

impl ApiKey {
    /// Records a newly issued key. `secret_hash` comes from the
    /// `ApiKeyHasher`; this constructor never sees the plaintext.
    pub fn issue(
        user_id: UserId,
        name: &str,
        prefix: &str,
        secret_hash: String,
        scopes: Vec<Scope>,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RaError::Validation("key name is empty".to_string()));
        }
        if scopes.is_empty() {
            return Err(RaError::Validation(
                "a key needs at least one scope".to_string(),
            ));
        }

        Ok(Self {
            id: ApiKeyId::new(),
            user_id,
            name: name.to_string(),
            prefix: prefix.to_string(),
            secret_hash,
            scopes,
            created_at: now,
            last_used_at: None,
            revoked_at: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_stored(
        id: ApiKeyId,
        user_id: UserId,
        name: String,
        prefix: String,
        secret_hash: String,
        scopes: Vec<Scope>,
        created_at: DateTime<Utc>,
        last_used_at: Option<DateTime<Utc>>,
        revoked_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id,
            user_id,
            name,
            prefix,
            secret_hash,
            scopes,
            created_at,
            last_used_at,
            revoked_at,
        }
    }

    pub fn id(&self) -> ApiKeyId {
        self.id
    }

    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn secret_hash(&self) -> &str {
        &self.secret_hash
    }

    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    pub fn last_used_at(&self) -> Option<DateTime<Utc>> {
        self.last_used_at
    }

    pub fn revoked_at(&self) -> Option<DateTime<Utc>> {
        self.revoked_at
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Whether this key carries `required`. `Admin` grants everything;
    /// otherwise membership is exact (see [`Scope`]).
    pub fn allows(&self, required: Scope) -> bool {
        self.scopes.contains(&Scope::Admin) || self.scopes.contains(&required)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn issued(scopes: Vec<Scope>) -> ApiKey {
        ApiKey::issue(
            UserId::new(),
            "laptop",
            "1f4c8a20",
            "argon2-hash".to_string(),
            scopes,
            now(),
        )
        .unwrap()
    }

    #[test]
    fn generated_tokens_round_trip_through_render_and_parse() {
        let token = ApiKeyToken::generate();
        let parsed = ApiKeyToken::parse(&token.render()).unwrap();
        assert_eq!(parsed, token);
    }

    #[test]
    fn generated_tokens_have_the_documented_shape() {
        let rendered = ApiKeyToken::generate().render();
        assert!(rendered.starts_with(KEY_SCHEME), "got {rendered}");
        assert_eq!(rendered.len(), KEY_SCHEME.len() + 8 + 32);
    }

    #[test]
    fn generated_tokens_are_unique() {
        let a = ApiKeyToken::generate();
        let b = ApiKeyToken::generate();
        assert_ne!(a.secret(), b.secret());
        assert_ne!(a.prefix(), b.prefix());
    }

    #[test]
    fn parse_splits_prefix_from_secret() {
        let raw = format!("{KEY_SCHEME}1f4c8a20{}", "0".repeat(32));
        let token = ApiKeyToken::parse(&raw).unwrap();
        assert_eq!(token.prefix(), "1f4c8a20");
        assert_eq!(token.secret(), "0".repeat(32));
    }

    #[test]
    fn parse_rejects_malformed_keys_opaquely() {
        let body = "0".repeat(40);
        let cases = [
            String::new(),
            "not-a-key".to_string(),
            format!("ra_test_{body}"),                 // wrong scheme
            format!("{KEY_SCHEME}{}", "0"),            // too short
            format!("{KEY_SCHEME}{body}0"),            // too long
            format!("{KEY_SCHEME}{}", "z".repeat(40)), // not hex
        ];

        for raw in cases {
            let err = ApiKeyToken::parse(&raw).unwrap_err();
            assert!(
                matches!(err, RaError::Unauthorized(_)),
                "{raw:?} -> {err:?}"
            );
            assert_eq!(err.to_string(), "unauthorized: invalid API key");
        }
    }

    #[test]
    fn parse_tolerates_surrounding_whitespace() {
        let raw = format!("  {KEY_SCHEME}1f4c8a20{}  ", "a".repeat(32));
        assert!(ApiKeyToken::parse(&raw).is_ok());
    }

    #[test]
    fn debug_never_reveals_the_secret() {
        let token = ApiKeyToken::generate();
        let rendered = format!("{token:?}");
        assert!(
            !rendered.contains(token.secret()),
            "Debug leaked the secret: {rendered}"
        );
        assert!(rendered.contains(token.prefix()));
    }

    #[test]
    fn a_fresh_key_is_not_revoked() {
        let key = issued(vec![Scope::Read]);
        assert!(!key.is_revoked());
        assert_eq!(key.last_used_at(), None);
    }

    #[test]
    fn allows_matches_exact_scopes() {
        let key = issued(vec![Scope::Read]);
        assert!(key.allows(Scope::Read));
        assert!(!key.allows(Scope::Write));
        assert!(!key.allows(Scope::Admin));
    }

    #[test]
    fn write_does_not_imply_read() {
        let key = issued(vec![Scope::Write]);
        assert!(key.allows(Scope::Write));
        assert!(!key.allows(Scope::Read));
    }

    #[test]
    fn admin_implies_every_scope() {
        let key = issued(vec![Scope::Admin]);
        assert!(key.allows(Scope::Read));
        assert!(key.allows(Scope::Write));
        assert!(key.allows(Scope::Admin));
    }

    #[test]
    fn issue_rejects_an_empty_name_or_no_scopes() {
        let user = UserId::new();
        assert!(
            ApiKey::issue(user, "  ", "1f4c8a20", "h".into(), vec![Scope::Read], now()).is_err()
        );
        assert!(ApiKey::issue(user, "laptop", "1f4c8a20", "h".into(), vec![], now()).is_err());
    }
}
