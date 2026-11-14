//! `SelfNameStore` over the V13 `self_names` table.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::self_names::{SelfNameStore, SelfNames};
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error};
use chrono::Utc;
use rusqlite::OptionalExtension;
use std::sync::Arc;

pub struct SqliteSelfNameStore {
    database: Arc<SqliteDatabase>,
}

impl SqliteSelfNameStore {
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }
}

impl SelfNameStore for SqliteSelfNameStore {
    fn get(&self, context: &UserContext) -> Result<SelfNames> {
        let stored: Option<String> = self.database.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT names FROM self_names WHERE user_id = ?1",
                    [context.user_id().to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| map_sqlite_error(e, "reading the user's names"))
        })?;
        match stored {
            None => Ok(SelfNames::default()),
            Some(json) => serde_json::from_str::<Vec<String>>(&json)
                .map(SelfNames::from_stored)
                .map_err(|e| RaError::Internal(format!("stored names are not a list: {e}"))),
        }
    }

    fn set(&self, context: &UserContext, names: &SelfNames) -> Result<()> {
        let json = serde_json::to_string(names.names())
            .map_err(|e| RaError::Internal(format!("could not encode names: {e}")))?;
        self.database.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO self_names (user_id, names, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT (user_id) DO UPDATE SET names = ?2, updated_at = ?3",
                    (context.user_id().to_string(), json, Utc::now().to_rfc3339()),
                )
                .map_err(|e| map_sqlite_error(e, "saving the user's names"))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;

    #[test]
    fn names_round_trip_and_start_empty() {
        let fixture = Fixture::new();
        let store = SqliteSelfNameStore::new(fixture.database.clone());
        let alex = fixture.alex.clone();
        assert!(store.get(&alex).unwrap().is_empty());

        let names = SelfNames::parse(&["Alejandro Romero".into(), "Alex".into()]).unwrap();
        store.set(&alex, &names).unwrap();
        assert_eq!(store.get(&alex).unwrap(), names);

        store.set(&alex, &SelfNames::default()).unwrap();
        assert!(store.get(&alex).unwrap().is_empty());
        assert!(store.get(&fixture.sam).unwrap().is_empty());
    }
}
