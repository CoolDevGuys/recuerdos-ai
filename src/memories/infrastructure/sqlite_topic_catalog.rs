//! `TopicCatalog` over the V11 `topics` table.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::topic::Topic;
use crate::memories::domain::topic_catalog::TopicCatalog;
use crate::shared::error::Result;
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error};
use std::sync::Arc;

pub struct SqliteTopicCatalog {
    database: Arc<SqliteDatabase>,
}

impl SqliteTopicCatalog {
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }
}

impl TopicCatalog for SqliteTopicCatalog {
    fn custom(&self, context: &UserContext) -> Result<Vec<Topic>> {
        self.database.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT id FROM topics WHERE user_id = ?1 AND built_in = 0 ORDER BY id")
                .map_err(|e| map_sqlite_error(e, "reading topics"))?;
            let ids = statement
                .query_map([context.user_id().to_string()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| map_sqlite_error(e, "reading topics"))?
                .collect::<std::result::Result<Vec<String>, _>>()
                .map_err(|e| map_sqlite_error(e, "reading topics"))?;
            Ok(ids.iter().map(|id| Topic::from_stored(id)).collect())
        })
    }
}
