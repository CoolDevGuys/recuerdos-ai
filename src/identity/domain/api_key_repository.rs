//! Storage contract for API keys. Implemented by
//! `SqliteApiKeyRepository` (infrastructure) and by an in-memory fake in
//! tests.

use super::api_key::ApiKey;
use crate::shared::error::Result;
use crate::shared::ids::{ApiKeyId, UserId};
use chrono::{DateTime, Utc};

pub trait ApiKeyRepository: Send + Sync {
    fn insert(&self, key: &ApiKey) -> Result<()>;

    /// The authentication lookup: one indexed hit on the non-secret
    /// prefix. Returns revoked keys too — deciding what a revoked key
    /// means is the use case's job, not the store's.
    fn find_by_prefix(&self, prefix: &str) -> Result<Option<ApiKey>>;

    fn list_for_user(&self, user_id: UserId) -> Result<Vec<ApiKey>>;

    /// Marks a key revoked. Returns `RaError::NotFound` if no such key,
    /// and is idempotent for an already-revoked one.
    fn revoke(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<()>;

    /// Records that a key was just used. Called off the request's hot
    /// path, so a failure here must never fail the request.
    fn touch_last_used(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<()>;
}
