//! `ConsolidationStateStore` backed by SQLite.
//!
//! Every statement carries `user_id = ?` from an authenticated
//! [`UserContext`]: this table records which of a person's memory groups
//! were consolidated and when, and a missing scope would let one user's
//! watermark skip another user's memories.
//!
//! `subcategory` is stored as an empty string for "no sub-label", so the
//! `(user_id, category, subcategory)` primary key stays a real key — a
//! `NULL` there would let SQLite treat every no-subcategory row as
//! distinct and break the upsert.

use crate::consolidation::domain::consolidation_state::ConsolidationStateStore;
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error, optional};
use chrono::{DateTime, Utc};
use std::sync::Arc;

pub struct SqliteConsolidationStateStore {
    database: Arc<SqliteDatabase>,
}

impl SqliteConsolidationStateStore {
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }
}

impl ConsolidationStateStore for SqliteConsolidationStateStore {
    fn last_max_updated_at(
        &self,
        context: &UserContext,
        category: &str,
        subcategory: Option<&str>,
    ) -> Result<Option<DateTime<Utc>>> {
        self.database.with_connection(|connection| {
            optional(connection.query_row(
                "SELECT max_updated_at FROM consolidation_state
                 WHERE user_id = ?1 AND category = ?2 AND subcategory = ?3",
                rusqlite::params![
                    context.user_id().to_string(),
                    category,
                    subcategory.unwrap_or(""),
                ],
                |row| {
                    let raw: String = row.get(0)?;
                    Ok(DateTime::parse_from_rfc3339(&raw)
                        .map(|value| value.with_timezone(&Utc))
                        .map_err(|e| {
                            RaError::Internal(format!(
                                "stored consolidation watermark {raw:?}: {e}"
                            ))
                        }))
                },
            ))
        })
    }

    fn record(
        &self,
        context: &UserContext,
        category: &str,
        subcategory: Option<&str>,
        max_updated_at: DateTime<Utc>,
    ) -> Result<()> {
        self.database.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO consolidation_state
                        (user_id, category, subcategory, max_updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (user_id, category, subcategory) DO UPDATE SET
                        max_updated_at = excluded.max_updated_at",
                    rusqlite::params![
                        context.user_id().to_string(),
                        category,
                        subcategory.unwrap_or(""),
                        max_updated_at.to_rfc3339(),
                    ],
                )
                .map_err(|e| map_sqlite_error(e, "consolidation state write conflict"))?;
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
        store: SqliteConsolidationStateStore,
        alex: UserContext,
        sam: UserContext,
    }

    fn fixture() -> Fixture {
        let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
        let identity = Identity::from_database(Arc::clone(&database)).unwrap();

        Fixture {
            store: SqliteConsolidationStateStore::new(database),
            alex: authenticate(&identity, "alex"),
            sam: authenticate(&identity, "sam"),
        }
    }

    fn authenticate(identity: &Identity, handle: &str) -> UserContext {
        identity.user_creator.execute(handle, None).unwrap();
        let issued = identity
            .api_key_issuer
            .execute(handle, vec![Scope::Admin], "test")
            .unwrap();
        identity
            .key_authenticator
            .execute(&issued.token.render())
            .unwrap()
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn a_group_that_was_never_consolidated_has_no_watermark() {
        let f = fixture();
        assert_eq!(
            f.store
                .last_max_updated_at(&f.alex, "preference.coding", None)
                .unwrap(),
            None
        );
    }

    #[test]
    fn a_recorded_watermark_reads_back() {
        let f = fixture();
        f.store
            .record(&f.alex, "preference.coding", None, at(1_000))
            .unwrap();

        assert_eq!(
            f.store
                .last_max_updated_at(&f.alex, "preference.coding", None)
                .unwrap(),
            Some(at(1_000))
        );
    }

    #[test]
    fn recording_again_overwrites_rather_than_duplicates() {
        let f = fixture();
        f.store
            .record(&f.alex, "preference.coding", None, at(1_000))
            .unwrap();
        f.store
            .record(&f.alex, "preference.coding", None, at(2_000))
            .unwrap();

        assert_eq!(
            f.store
                .last_max_updated_at(&f.alex, "preference.coding", None)
                .unwrap(),
            Some(at(2_000))
        );
    }

    #[test]
    fn a_subcategory_is_tracked_separately_from_its_bare_category() {
        let f = fixture();
        f.store
            .record(&f.alex, "preference.coding", None, at(1_000))
            .unwrap();
        f.store
            .record(&f.alex, "preference.coding", Some("testing"), at(2_000))
            .unwrap();

        assert_eq!(
            f.store
                .last_max_updated_at(&f.alex, "preference.coding", None)
                .unwrap(),
            Some(at(1_000))
        );
        assert_eq!(
            f.store
                .last_max_updated_at(&f.alex, "preference.coding", Some("testing"))
                .unwrap(),
            Some(at(2_000))
        );
    }

    #[test]
    fn one_users_watermark_is_never_visible_to_another() {
        let f = fixture();
        f.store
            .record(&f.alex, "preference.coding", None, at(1_000))
            .unwrap();

        assert_eq!(
            f.store
                .last_max_updated_at(&f.sam, "preference.coding", None)
                .unwrap(),
            None,
            "one user's consolidation watermark leaked to another"
        );
    }
}
