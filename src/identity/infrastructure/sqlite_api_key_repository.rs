//! `ApiKeyRepository` backed by SQLite.

use crate::identity::domain::api_key::ApiKey;
use crate::identity::domain::api_key_repository::ApiKeyRepository;
use crate::identity::domain::scope::Scope;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{ApiKeyId, UserId};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error, optional};
use chrono::{DateTime, Utc};
use rusqlite::Row;
use std::str::FromStr;
use std::sync::Arc;

const COLUMNS: &str = "id, user_id, name, prefix, secret_hash, scopes, \
                       created_at, last_used_at, revoked_at";

pub struct SqliteApiKeyRepository {
    database: Arc<SqliteDatabase>,
}

impl SqliteApiKeyRepository {
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }
}

impl ApiKeyRepository for SqliteApiKeyRepository {
    fn insert(&self, key: &ApiKey) -> Result<()> {
        self.database.with_connection(|conn| {
            conn.execute(
                "INSERT INTO api_keys (id, user_id, name, prefix, secret_hash, scopes, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    key.id().to_string(),
                    key.user_id().to_string(),
                    key.name(),
                    key.prefix(),
                    key.secret_hash(),
                    Scope::join(key.scopes()),
                    key.created_at().to_rfc3339(),
                ],
            )
            .map_err(|e| map_sqlite_error(e, "API key already exists"))?;
            Ok(())
        })
    }

    fn find_by_prefix(&self, prefix: &str) -> Result<Option<ApiKey>> {
        self.database.with_connection(|conn| {
            optional(conn.query_row(
                &format!("SELECT {COLUMNS} FROM api_keys WHERE prefix = ?1"),
                [prefix],
                row_to_api_key,
            ))
        })
    }

    fn list_for_user(&self, user_id: UserId) -> Result<Vec<ApiKey>> {
        self.database.with_connection(|conn| {
            let mut statement = conn
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM api_keys WHERE user_id = ?1 ORDER BY created_at"
                ))
                .map_err(|e| map_sqlite_error(e, "key list conflict"))?;

            let keys = statement
                .query_map([user_id.to_string()], row_to_api_key)
                .map_err(|e| map_sqlite_error(e, "key list conflict"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| map_sqlite_error(e, "key list conflict"))?;

            keys.into_iter().collect::<Result<Vec<_>>>()
        })
    }

    fn revoke(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<()> {
        self.database.with_connection(|conn| {
            // COALESCE keeps the original revocation time: revoking twice
            // is a no-op, not a way to rewrite when access actually ended.
            let affected = conn
                .execute(
                    "UPDATE api_keys SET revoked_at = COALESCE(revoked_at, ?2) WHERE id = ?1",
                    rusqlite::params![id.to_string(), now.to_rfc3339()],
                )
                .map_err(|e| map_sqlite_error(e, "key revoke conflict"))?;

            if affected == 0 {
                return Err(RaError::NotFound(format!("API key {id} not found")));
            }
            Ok(())
        })
    }

    fn touch_last_used(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<()> {
        self.database.with_connection(|conn| {
            conn.execute(
                "UPDATE api_keys SET last_used_at = ?2 WHERE id = ?1",
                rusqlite::params![id.to_string(), now.to_rfc3339()],
            )
            .map_err(|e| map_sqlite_error(e, "key touch conflict"))?;
            Ok(())
        })
    }
}

fn row_to_api_key(row: &Row<'_>) -> rusqlite::Result<Result<ApiKey>> {
    let id: String = row.get(0)?;
    let user_id: String = row.get(1)?;
    let name: String = row.get(2)?;
    let prefix: String = row.get(3)?;
    let secret_hash: String = row.get(4)?;
    let scopes: String = row.get(5)?;
    let created_at: String = row.get(6)?;
    let last_used_at: Option<String> = row.get(7)?;
    let revoked_at: Option<String> = row.get(8)?;

    Ok(build_api_key(
        id,
        user_id,
        name,
        prefix,
        secret_hash,
        scopes,
        created_at,
        last_used_at,
        revoked_at,
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_api_key(
    id: String,
    user_id: String,
    name: String,
    prefix: String,
    secret_hash: String,
    scopes: String,
    created_at: String,
    last_used_at: Option<String>,
    revoked_at: Option<String>,
) -> Result<ApiKey> {
    let id = ApiKeyId::from_str(&id)
        .map_err(|e| RaError::Internal(format!("stored key id {id:?} is not a uuid: {e}")))?;
    let user_id = UserId::from_str(&user_id)
        .map_err(|e| RaError::Internal(format!("stored user id {user_id:?} is not a uuid: {e}")))?;
    let scopes = Scope::parse_list(&scopes)
        .map_err(|e| RaError::Internal(format!("stored scopes {scopes:?} are unreadable: {e}")))?;

    Ok(ApiKey::from_stored(
        id,
        user_id,
        name,
        prefix,
        secret_hash,
        scopes,
        parse_timestamp(&created_at)?,
        last_used_at.as_deref().map(parse_timestamp).transpose()?,
        revoked_at.as_deref().map(parse_timestamp).transpose()?,
    ))
}

fn parse_timestamp(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|e| RaError::Internal(format!("stored timestamp {raw:?} is not rfc3339: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::domain::user::User;
    use crate::identity::domain::user_repository::UserRepository;
    use crate::identity::infrastructure::sqlite_user_repository::SqliteUserRepository;

    struct Fixture {
        users: SqliteUserRepository,
        keys: SqliteApiKeyRepository,
    }

    fn fixture() -> Fixture {
        let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
        Fixture {
            users: SqliteUserRepository::new(Arc::clone(&database)),
            keys: SqliteApiKeyRepository::new(database),
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn user(fixture: &Fixture, handle: &str) -> User {
        let user = User::create(handle, None, now()).unwrap();
        fixture.users.insert(&user).unwrap();
        user
    }

    fn key_for(user: &User, prefix: &str, scopes: Vec<Scope>) -> ApiKey {
        ApiKey::issue(
            user.id(),
            "laptop",
            prefix,
            "argon2-hash".to_string(),
            scopes,
            now(),
        )
        .unwrap()
    }

    #[test]
    fn round_trips_a_key() {
        let fixture = fixture();
        let user = user(&fixture, "alex");
        let key = key_for(&user, "1f4c8a20", vec![Scope::Read, Scope::Write]);
        fixture.keys.insert(&key).unwrap();

        let found = fixture.keys.find_by_prefix("1f4c8a20").unwrap().unwrap();
        assert_eq!(found.id(), key.id());
        assert_eq!(found.user_id(), user.id());
        assert_eq!(found.name(), "laptop");
        assert_eq!(found.secret_hash(), "argon2-hash");
        assert_eq!(found.scopes(), &[Scope::Read, Scope::Write]);
        assert_eq!(found.created_at(), now());
        assert_eq!(found.last_used_at(), None);
        assert!(!found.is_revoked());
    }

    #[test]
    fn missing_prefixes_are_none_not_errors() {
        let fixture = fixture();
        assert!(fixture.keys.find_by_prefix("deadbeef").unwrap().is_none());
    }

    #[test]
    fn revoking_marks_the_key_and_survives_a_reload() {
        let fixture = fixture();
        let user = user(&fixture, "alex");
        let key = key_for(&user, "1f4c8a20", vec![Scope::Read]);
        fixture.keys.insert(&key).unwrap();

        fixture.keys.revoke(key.id(), now()).unwrap();

        let found = fixture.keys.find_by_prefix("1f4c8a20").unwrap().unwrap();
        assert!(found.is_revoked());
        assert_eq!(found.revoked_at(), Some(now()));
    }

    #[test]
    fn revoking_twice_keeps_the_original_revocation_time() {
        let fixture = fixture();
        let user = user(&fixture, "alex");
        let key = key_for(&user, "1f4c8a20", vec![Scope::Read]);
        fixture.keys.insert(&key).unwrap();

        fixture.keys.revoke(key.id(), now()).unwrap();
        let later = now() + chrono::Duration::days(1);
        fixture.keys.revoke(key.id(), later).unwrap();

        let found = fixture.keys.find_by_prefix("1f4c8a20").unwrap().unwrap();
        assert_eq!(
            found.revoked_at(),
            Some(now()),
            "a second revoke must not rewrite when access actually ended"
        );
    }

    #[test]
    fn revoking_an_unknown_key_is_not_found() {
        let fixture = fixture();
        let err = fixture.keys.revoke(ApiKeyId::new(), now()).unwrap_err();
        assert!(matches!(err, RaError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn touch_records_last_used() {
        let fixture = fixture();
        let user = user(&fixture, "alex");
        let key = key_for(&user, "1f4c8a20", vec![Scope::Read]);
        fixture.keys.insert(&key).unwrap();

        let used_at = now() + chrono::Duration::hours(3);
        fixture.keys.touch_last_used(key.id(), used_at).unwrap();

        let found = fixture.keys.find_by_prefix("1f4c8a20").unwrap().unwrap();
        assert_eq!(found.last_used_at(), Some(used_at));
    }

    #[test]
    fn duplicate_prefixes_are_rejected() {
        let fixture = fixture();
        let user = user(&fixture, "alex");
        fixture
            .keys
            .insert(&key_for(&user, "1f4c8a20", vec![Scope::Read]))
            .unwrap();

        let err = fixture
            .keys
            .insert(&key_for(&user, "1f4c8a20", vec![Scope::Read]))
            .unwrap_err();
        assert!(matches!(err, RaError::Conflict(_)), "got {err:?}");
    }

    #[test]
    fn lists_only_the_requested_users_keys() {
        let fixture = fixture();
        let alex = user(&fixture, "alex");
        let sam = user(&fixture, "sam");
        fixture
            .keys
            .insert(&key_for(&alex, "aaaaaaaa", vec![Scope::Read]))
            .unwrap();
        fixture
            .keys
            .insert(&key_for(&alex, "bbbbbbbb", vec![Scope::Write]))
            .unwrap();
        fixture
            .keys
            .insert(&key_for(&sam, "cccccccc", vec![Scope::Read]))
            .unwrap();

        let alex_keys = fixture.keys.list_for_user(alex.id()).unwrap();
        assert_eq!(alex_keys.len(), 2);
        assert!(
            alex_keys.iter().all(|k| k.user_id() == alex.id()),
            "another user's key leaked into the list"
        );

        let sam_keys = fixture.keys.list_for_user(sam.id()).unwrap();
        assert_eq!(sam_keys.len(), 1);
        assert_eq!(sam_keys[0].prefix(), "cccccccc");
    }
}
