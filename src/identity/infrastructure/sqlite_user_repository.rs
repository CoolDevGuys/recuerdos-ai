//! `UserRepository` backed by SQLite.

use crate::identity::domain::user::User;
use crate::identity::domain::user_repository::UserRepository;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::UserId;
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error, optional};
use chrono::{DateTime, Utc};
use rusqlite::Row;
use std::str::FromStr;
use std::sync::Arc;

pub struct SqliteUserRepository {
    database: Arc<SqliteDatabase>,
}

impl SqliteUserRepository {
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }
}

impl UserRepository for SqliteUserRepository {
    fn insert(&self, user: &User) -> Result<()> {
        self.database.with_connection(|conn| {
            conn.execute(
                "INSERT INTO users (id, handle, email, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    user.id().to_string(),
                    user.handle(),
                    user.email(),
                    user.created_at().to_rfc3339(),
                ],
            )
            .map_err(|e| {
                map_sqlite_error(e, &format!("user {:?} already exists", user.handle()))
            })?;
            Ok(())
        })
    }

    fn find_by_handle(&self, handle: &str) -> Result<Option<User>> {
        let handle = handle.trim().to_ascii_lowercase();
        self.database.with_connection(|conn| {
            optional(conn.query_row(
                "SELECT id, handle, email, created_at FROM users WHERE handle = ?1",
                [&handle],
                row_to_user,
            ))
        })
    }

    fn find_by_id(&self, id: UserId) -> Result<Option<User>> {
        self.database.with_connection(|conn| {
            optional(conn.query_row(
                "SELECT id, handle, email, created_at FROM users WHERE id = ?1",
                [id.to_string()],
                row_to_user,
            ))
        })
    }

    fn list(&self) -> Result<Vec<User>> {
        self.database.with_connection(|conn| {
            let mut statement = conn
                .prepare("SELECT id, handle, email, created_at FROM users ORDER BY handle")
                .map_err(|e| map_sqlite_error(e, "user list conflict"))?;

            let users = statement
                .query_map([], row_to_user)
                .map_err(|e| map_sqlite_error(e, "user list conflict"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| map_sqlite_error(e, "user list conflict"))?;

            users.into_iter().collect::<Result<Vec<_>>>()
        })
    }
}

fn row_to_user(row: &Row<'_>) -> rusqlite::Result<Result<User>> {
    let id: String = row.get(0)?;
    let handle: String = row.get(1)?;
    let email: Option<String> = row.get(2)?;
    let created_at: String = row.get(3)?;

    Ok(build_user(id, handle, email, created_at))
}

fn build_user(
    id: String,
    handle: String,
    email: Option<String>,
    created_at: String,
) -> Result<User> {
    let id = UserId::from_str(&id)
        .map_err(|e| RaError::Internal(format!("stored user id {id:?} is not a uuid: {e}")))?;
    let created_at = parse_timestamp(&created_at)?;

    Ok(User::from_stored(id, handle, email, created_at))
}

fn parse_timestamp(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|e| RaError::Internal(format!("stored timestamp {raw:?} is not rfc3339: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> SqliteUserRepository {
        SqliteUserRepository::new(Arc::new(SqliteDatabase::open_in_memory().unwrap()))
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    #[test]
    fn round_trips_a_user() {
        let repository = repository();
        let user = User::create("alex", Some("alex@example.com"), now()).unwrap();
        repository.insert(&user).unwrap();

        let found = repository.find_by_handle("alex").unwrap().unwrap();
        assert_eq!(found.id(), user.id());
        assert_eq!(found.handle(), "alex");
        assert_eq!(found.email(), Some("alex@example.com"));
        assert_eq!(found.created_at(), now());
    }

    #[test]
    fn finds_by_id() {
        let repository = repository();
        let user = User::create("alex", None, now()).unwrap();
        repository.insert(&user).unwrap();

        assert_eq!(
            repository.find_by_id(user.id()).unwrap().unwrap().handle(),
            "alex"
        );
    }

    #[test]
    fn missing_users_are_none_not_errors() {
        let repository = repository();
        assert!(repository.find_by_handle("nobody").unwrap().is_none());
        assert!(repository.find_by_id(UserId::new()).unwrap().is_none());
    }

    #[test]
    fn lookup_is_case_insensitive_like_creation() {
        let repository = repository();
        repository
            .insert(&User::create("Alex", None, now()).unwrap())
            .unwrap();

        assert!(repository.find_by_handle("ALEX").unwrap().is_some());
    }

    #[test]
    fn duplicate_handles_are_rejected_by_the_store() {
        let repository = repository();
        repository
            .insert(&User::create("alex", None, now()).unwrap())
            .unwrap();

        // A second User value with the same handle is a different entity
        // with a different id — only the store's UNIQUE constraint can
        // catch it, which is exactly why the use case doesn't check first.
        let err = repository
            .insert(&User::create("alex", None, now()).unwrap())
            .unwrap_err();

        assert!(matches!(err, RaError::Conflict(_)), "got {err:?}");
        assert!(err.to_string().contains("alex"));
    }

    #[test]
    fn lists_users_alphabetically() {
        let repository = repository();
        for handle in ["sam", "alex", "kim"] {
            repository
                .insert(&User::create(handle, None, now()).unwrap())
                .unwrap();
        }

        let handles: Vec<String> = repository
            .list()
            .unwrap()
            .iter()
            .map(|u| u.handle().to_string())
            .collect();
        assert_eq!(handles, vec!["alex", "kim", "sam"]);
    }
}
