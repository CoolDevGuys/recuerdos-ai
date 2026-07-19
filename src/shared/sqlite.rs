//! SQLite connection handling and schema migration.
//!
//! # Concurrency
//!
//! One connection behind a mutex, in WAL mode. SQLite allows only one
//! writer regardless, and the read side is not the bottleneck for the
//! workload this guards: authentication does one indexed lookup (tens of
//! microseconds) and then an argon2 verify (tens of *milli*seconds) that
//! happens outside the lock. A reader pool would optimize the cheap half.
//!
//! Phase 2 adds memory recall — genuinely concurrent, genuinely read-heavy
//! — and that is when a reader pool earns its complexity. See
//! implementation-plan.md §2.2.
//!
//! Every call here blocks. Async callers must wrap them in
//! `tokio::task::spawn_blocking` rather than stalling a runtime worker.

use crate::shared::error::{RaError, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Mutex, Once};

mod embedded {
    refinery::embed_migrations!("migrations");
}

static REGISTER_EXTENSIONS: Once = Once::new();

/// Registers sqlite-vec as an auto-extension, so every connection opened
/// afterwards has `vec0` available.
///
/// `sqlite3_auto_extension` is process-global and must run before the
/// connections that need it; `Once` makes that idempotent and
/// thread-safe. The `transmute` is how the C entry point is handed to
/// SQLite — the signature is fixed by the C API and checked by nothing,
/// which is why it is confined to this one place.
fn register_extensions() {
    REGISTER_EXTENSIONS.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            // `c_char` rather than a hardcoded i8: it is unsigned on
            // aarch64 Linux, so spelling it out breaks that build.
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::os::raw::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> i32,
        >(
            sqlite_vec::sqlite3_vec_init as *const ()
        )));
    });
}

pub struct SqliteDatabase {
    connection: Mutex<Connection>,
}

impl SqliteDatabase {
    /// Opens (creating if absent) the database at `path`, applies pragmas
    /// and runs any pending migrations. Parent directories are created.
    pub fn open(path: &Path) -> Result<Self> {
        register_extensions();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                RaError::Internal(format!(
                    "failed to create data directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let connection = Connection::open(path).map_err(|e| {
            RaError::Internal(format!("failed to open database {}: {e}", path.display()))
        })?;

        Self::from_connection(connection)
    }

    /// An ephemeral database for tests. Same pragmas, same migrations —
    /// tests exercise the real schema, never a hand-written stand-in.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        register_extensions();

        let connection = Connection::open_in_memory()
            .map_err(|e| RaError::Internal(format!("failed to open in-memory database: {e}")))?;
        Self::from_connection(connection)
    }

    fn from_connection(mut connection: Connection) -> Result<Self> {
        // WAL lets readers proceed during a write. busy_timeout makes a
        // contended write wait rather than immediately returning SQLITE_BUSY.
        // foreign_keys is off by default in SQLite and must be enabled per
        // connection — without it the api_keys -> users reference is
        // decorative.
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| RaError::Internal(format!("failed to enable WAL: {e}")))?;
        connection
            .pragma_update(None, "busy_timeout", 5_000)
            .map_err(|e| RaError::Internal(format!("failed to set busy_timeout: {e}")))?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|e| RaError::Internal(format!("failed to enable foreign_keys: {e}")))?;

        embedded::migrations::runner()
            .run(&mut connection)
            .map_err(|e| RaError::Internal(format!("failed to run migrations: {e}")))?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Runs `f` with the connection held. Keep the closure short: it
    /// serializes every other database user.
    pub fn with_connection<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RaError::Internal("database mutex poisoned".to_string()))?;
        f(&connection)
    }
}

/// Translates a rusqlite error into the domain error a caller can act on.
///
/// A UNIQUE violation is the only one with a genuine domain meaning
/// (`Conflict`); everything else is a failure of the storage layer itself
/// and stays `Internal`.
pub fn map_sqlite_error(error: rusqlite::Error, conflict_message: &str) -> RaError {
    use rusqlite::ErrorCode;

    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == ErrorCode::ConstraintViolation =>
        {
            // Covers UNIQUE, PRIMARY KEY and FOREIGN KEY violations. All
            // three mean "the caller asked for something the data model
            // forbids", which is a conflict, not an outage.
            RaError::Conflict(conflict_message.to_string())
        }
        _ => RaError::Internal(format!("database error: {error}")),
    }
}

/// Collapses a single-row query into an `Option`.
///
/// rusqlite reports "no such row" as an error; for a `find_*` that
/// returns `Option`, absence is an ordinary answer, not a failure. The
/// nested `Result` is the row mapper's own — mapping a stored row can
/// fail independently of the query.
pub fn optional<T>(outcome: rusqlite::Result<Result<T>>) -> Result<Option<T>> {
    match outcome {
        Ok(mapped) => mapped.map(Some),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(map_sqlite_error(error, "unexpected conflict during read")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_maps_no_rows_to_none() {
        let db = SqliteDatabase::open_in_memory().unwrap();
        let found: Option<String> = db
            .with_connection(|conn| {
                optional(conn.query_row(
                    "SELECT handle FROM users WHERE handle = 'nobody'",
                    [],
                    |row| {
                        Ok(row
                            .get::<_, String>(0)
                            .map_err(|e| RaError::Internal(format!("column read failed: {e}"))))
                    },
                ))
            })
            .unwrap();

        assert!(found.is_none());
    }

    #[test]
    fn open_in_memory_applies_migrations() {
        let db = SqliteDatabase::open_in_memory().unwrap();

        let tables: Vec<String> = db
            .with_connection(|conn| {
                let mut stmt = conn
                    .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                    .unwrap();
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .unwrap()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap();
                Ok(rows)
            })
            .unwrap();

        assert!(tables.contains(&"users".to_string()), "got {tables:?}");
        assert!(tables.contains(&"api_keys".to_string()), "got {tables:?}");
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let db = SqliteDatabase::open_in_memory().unwrap();

        let result = db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO api_keys (id, user_id, name, prefix, secret_hash, scopes, created_at)
                 VALUES ('k1', 'no-such-user', 'n', 'p', 'h', 'read', '2026-01-01T00:00:00Z')",
                [],
            )
            .map_err(|e| map_sqlite_error(e, "orphan key"))
        });

        assert!(
            matches!(result, Err(RaError::Conflict(_))),
            "inserting a key for a missing user must fail, got {result:?}"
        );
    }

    #[test]
    fn open_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/recordagent.db");

        SqliteDatabase::open(&path).unwrap();

        assert!(path.exists(), "database file was not created");
    }

    #[test]
    fn migrations_are_idempotent_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recordagent.db");

        SqliteDatabase::open(&path).unwrap();
        // Re-opening runs the migration runner again against a schema
        // that is already current: it must be a no-op, not an error.
        SqliteDatabase::open(&path).unwrap();
    }

    #[test]
    fn unique_violations_map_to_conflict() {
        let db = SqliteDatabase::open_in_memory().unwrap();

        db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO users (id, handle, created_at)
                 VALUES ('u1', 'alex', '2026-01-01T00:00:00Z')",
                [],
            )
            .map_err(|e| map_sqlite_error(e, "handle taken"))?;
            Ok(())
        })
        .unwrap();

        let result = db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO users (id, handle, created_at)
                 VALUES ('u2', 'alex', '2026-01-01T00:00:00Z')",
                [],
            )
            .map_err(|e| map_sqlite_error(e, "handle taken"))
        });

        match result {
            Err(RaError::Conflict(message)) => assert_eq!(message, "handle taken"),
            other => panic!("expected Conflict, got {other:?}"),
        }
    }
}
