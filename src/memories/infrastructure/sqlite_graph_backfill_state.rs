//! `GraphBackfillState` backed by SQLite.
//!
//! Every statement carries `user_id = ?` from an authenticated
//! [`UserContext`]: one person's backfill progress must never move another
//! person's, exactly like the consolidation watermark beside it.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::graph_backfill_state::GraphBackfillState;
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error, optional};
use chrono::{DateTime, Utc};
use std::sync::Arc;

pub struct SqliteGraphBackfillState {
    database: Arc<SqliteDatabase>,
}

impl SqliteGraphBackfillState {
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }
}

impl GraphBackfillState for SqliteGraphBackfillState {
    fn relations_cursor(&self, context: &UserContext) -> Result<Option<DateTime<Utc>>> {
        self.database.with_connection(|connection| {
            optional(connection.query_row(
                "SELECT relations_cursor FROM graph_backfill_state WHERE user_id = ?1",
                rusqlite::params![context.user_id().to_string()],
                |row| {
                    let raw: String = row.get(0)?;
                    Ok(DateTime::parse_from_rfc3339(&raw)
                        .map(|value| value.with_timezone(&Utc))
                        .map_err(|e| {
                            RaError::Internal(format!("stored backfill cursor {raw:?}: {e}"))
                        }))
                },
            ))
        })
    }

    fn set_relations_cursor(&self, context: &UserContext, cursor: DateTime<Utc>) -> Result<()> {
        self.database.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO graph_backfill_state (user_id, relations_cursor)
                     VALUES (?1, ?2)
                     ON CONFLICT (user_id) DO UPDATE SET relations_cursor = excluded.relations_cursor",
                    rusqlite::params![context.user_id().to_string(), cursor.to_rfc3339()],
                )
                .map_err(|e| map_sqlite_error(e, "backfill state write conflict"))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::wiring::Identity;
    use crate::identity::domain::scope::Scope;

    struct Fixture {
        store: SqliteGraphBackfillState,
        alex: UserContext,
        sam: UserContext,
    }

    fn fixture() -> Fixture {
        let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
        let identity = Identity::from_database(Arc::clone(&database)).unwrap();
        let authenticate = |handle: &str| {
            identity.user_creator.execute(handle, None).unwrap();
            let issued = identity
                .api_key_issuer
                .execute(handle, vec![Scope::Admin], "test")
                .unwrap();
            identity
                .key_authenticator
                .execute(&issued.token.render())
                .unwrap()
        };
        Fixture {
            store: SqliteGraphBackfillState::new(database),
            alex: authenticate("alex"),
            sam: authenticate("sam"),
        }
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn a_missing_cursor_reads_as_none() {
        let fixture = fixture();
        assert_eq!(fixture.store.relations_cursor(&fixture.alex).unwrap(), None);
    }

    #[test]
    fn a_cursor_round_trips_and_is_overwritten_on_the_next_write() {
        let fixture = fixture();
        fixture
            .store
            .set_relations_cursor(&fixture.alex, at(1_000))
            .unwrap();
        assert_eq!(
            fixture.store.relations_cursor(&fixture.alex).unwrap(),
            Some(at(1_000))
        );

        fixture
            .store
            .set_relations_cursor(&fixture.alex, at(2_000))
            .unwrap();
        assert_eq!(
            fixture.store.relations_cursor(&fixture.alex).unwrap(),
            Some(at(2_000))
        );
    }

    #[test]
    fn one_users_cursor_is_invisible_to_another() {
        let fixture = fixture();
        fixture
            .store
            .set_relations_cursor(&fixture.alex, at(1_000))
            .unwrap();

        assert_eq!(fixture.store.relations_cursor(&fixture.sam).unwrap(), None);
    }
}
