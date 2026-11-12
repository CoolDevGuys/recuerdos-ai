//! `TopicCatalog` over the V11 `topics` table.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::topic::{BUILT_IN_TOPICS, Topic};
use crate::memories::domain::topic_catalog::{TopicCatalog, TopicEntry};
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error};
use chrono::Utc;
use std::collections::HashMap;
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

    fn entries(&self, context: &UserContext) -> Result<Vec<TopicEntry>> {
        let labels: HashMap<String, Option<String>> =
            self.database.with_connection(|connection| {
                let mut statement = connection
                    .prepare("SELECT id, label FROM topics WHERE user_id = ?1")
                    .map_err(|e| map_sqlite_error(e, "reading topics"))?;
                statement
                    .query_map([context.user_id().to_string()], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                    })
                    .map_err(|e| map_sqlite_error(e, "reading topics"))?
                    .collect::<std::result::Result<HashMap<_, _>, _>>()
                    .map_err(|e| map_sqlite_error(e, "reading topics"))
            })?;
        let mut entries: Vec<TopicEntry> = BUILT_IN_TOPICS
            .iter()
            .map(|topic| TopicEntry {
                label: labels.get(topic.as_str()).cloned().flatten(),
                topic: topic.clone(),
            })
            .collect();
        for topic in self.custom(context)? {
            entries.push(TopicEntry {
                label: labels.get(topic.as_str()).cloned().flatten(),
                topic,
            });
        }
        Ok(entries)
    }

    fn create(&self, context: &UserContext, topic: &Topic, label: Option<&str>) -> Result<()> {
        if topic.is_built_in() {
            return Err(RaError::Conflict(format!(
                "{topic} is a built-in topic; relabel it instead"
            )));
        }
        self.database.with_connection(|connection| {
            let now = Utc::now().to_rfc3339();
            connection
                .execute(
                    "INSERT INTO topics (user_id, id, label, built_in, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 0, ?4, ?4)",
                    rusqlite::params![context.user_id().to_string(), topic.as_str(), label, now],
                )
                .map_err(|e| map_sqlite_error(e, &format!("you already have the topic {topic}")))?;
            Ok(())
        })
    }

    fn relabel(&self, context: &UserContext, topic: &Topic, label: Option<&str>) -> Result<()> {
        let built_in = topic.is_built_in();
        self.database.with_connection(|connection| {
            let now = Utc::now().to_rfc3339();
            let changed = if built_in {
                // A built-in has a row only once it is relabelled.
                connection
                    .execute(
                        "INSERT INTO topics (user_id, id, label, built_in, created_at, updated_at)
                         VALUES (?1, ?2, ?3, 1, ?4, ?4)
                         ON CONFLICT (user_id, id) DO UPDATE SET label = excluded.label,
                                                                 updated_at = excluded.updated_at",
                        rusqlite::params![
                            context.user_id().to_string(),
                            topic.as_str(),
                            label,
                            now
                        ],
                    )
                    .map_err(|e| map_sqlite_error(e, "topic conflict"))?
            } else {
                connection
                    .execute(
                        "UPDATE topics SET label = ?3, updated_at = ?4
                         WHERE user_id = ?1 AND id = ?2 AND built_in = 0",
                        rusqlite::params![
                            context.user_id().to_string(),
                            topic.as_str(),
                            label,
                            now
                        ],
                    )
                    .map_err(|e| map_sqlite_error(e, "topic conflict"))?
            };
            if changed == 0 {
                return Err(RaError::NotFound(format!("topic {topic}")));
            }
            Ok(())
        })
    }
}
