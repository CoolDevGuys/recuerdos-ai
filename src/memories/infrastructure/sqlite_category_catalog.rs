//! `CategoryCatalog` backed by SQLite (migration V10).
//!
//! # Lazy seeding
//!
//! The first read of a user's catalog seeds it, in one transaction, with
//! the configured `[understanding.taxonomy].extra_categories` and with
//! every custom category already stored on one of their memories, then
//! records in `category_seeds` that it did. That is how existing users keep
//! every category they had when the taxonomy moved out of the config file,
//! and how an operator can still pre-provision categories for new users.
//! The marker makes it happen once: a user who deletes a seeded category
//! does not see it come back on the next read.
//!
//! Every statement carries `user_id = ?` from the [`UserContext`].

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::category::{Category, DEFAULT_CATEGORIES};
use crate::memories::domain::category_catalog::{CatalogEntry, CategoryCatalog};
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error};
use chrono::Utc;
use rusqlite::Connection;
use std::sync::Arc;

pub struct SqliteCategoryCatalog {
    database: Arc<SqliteDatabase>,
    /// The config extras, normalised: what a new user starts with.
    seed: Vec<String>,
}

impl SqliteCategoryCatalog {
    pub fn new(database: Arc<SqliteDatabase>, extra_categories: &[String]) -> Self {
        let mut seed: Vec<String> = extra_categories
            .iter()
            .map(|extra| extra.trim().to_ascii_lowercase())
            .filter(|extra| !extra.is_empty())
            .filter(|extra| !DEFAULT_CATEGORIES.iter().any(|c| c.as_str() == extra))
            .collect();
        seed.sort();
        seed.dedup();
        Self { database, seed }
    }

    fn ensure_seeded(&self, connection: &Connection, context: &UserContext) -> Result<()> {
        let user = context.user_id().to_string();
        let seeded: bool = connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM category_seeds WHERE user_id = ?1)",
                rusqlite::params![user],
                |row| row.get(0),
            )
            .map_err(|e| map_sqlite_error(e, "category seed read"))?;
        if seeded {
            return Ok(());
        }

        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| map_sqlite_error(e, "could not begin a category seed"))?;
        let now = Utc::now().to_rfc3339();
        for id in &self.seed {
            transaction
                .execute(
                    "INSERT INTO categories (user_id, id, label, built_in, created_at, updated_at)
                     VALUES (?1, ?2, NULL, 0, ?3, ?3)
                     ON CONFLICT (user_id, id) DO NOTHING",
                    rusqlite::params![user, id, now],
                )
                .map_err(|e| map_sqlite_error(e, "category seed conflict"))?;
        }
        // Forgotten and superseded memories too: they can come back (undo,
        // `include_superseded`), and must not come back to a category their
        // owner cannot see.
        let built_ins: Vec<String> = DEFAULT_CATEGORIES
            .iter()
            .map(|c| format!("'{}'", c.as_str()))
            .collect();
        transaction
            .execute(
                &format!(
                    "INSERT INTO categories (user_id, id, label, built_in, created_at, updated_at)
                     SELECT DISTINCT ?1, category, NULL, 0, ?2, ?2 FROM memories
                      WHERE user_id = ?1 AND category NOT IN ({})
                     ON CONFLICT (user_id, id) DO NOTHING",
                    built_ins.join(", ")
                ),
                rusqlite::params![user, now],
            )
            .map_err(|e| map_sqlite_error(e, "category seed conflict"))?;
        transaction
            .execute(
                "INSERT INTO category_seeds (user_id, seeded_at) VALUES (?1, ?2)",
                rusqlite::params![user, now],
            )
            .map_err(|e| map_sqlite_error(e, "category seed conflict"))?;
        transaction
            .commit()
            .map_err(|e| map_sqlite_error(e, "could not commit a category seed"))
    }
}

impl CategoryCatalog for SqliteCategoryCatalog {
    fn entries(&self, context: &UserContext) -> Result<Vec<CatalogEntry>> {
        self.database.with_connection(|connection| {
            self.ensure_seeded(connection, context)?;
            let mut statement = connection
                .prepare(
                    "SELECT id, label FROM categories WHERE user_id = ?1
                     ORDER BY built_in DESC, id",
                )
                .map_err(|e| map_sqlite_error(e, "category read"))?;
            let rows = statement
                .query_map(rusqlite::params![context.user_id().to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })
                .map_err(|e| map_sqlite_error(e, "category read"))?;
            rows.map(|row| {
                let (id, label) = row.map_err(|e| map_sqlite_error(e, "category read"))?;
                Ok(CatalogEntry {
                    category: Category::from_stored(&id),
                    label,
                })
            })
            .collect()
        })
    }

    fn create(&self, context: &UserContext, entry: &CatalogEntry) -> Result<()> {
        if entry.category.is_built_in() {
            return Err(RaError::Conflict(format!(
                "{} is a built-in category",
                entry.category
            )));
        }
        self.database.with_connection(|connection| {
            self.ensure_seeded(connection, context)?;
            let now = Utc::now().to_rfc3339();
            connection
                .execute(
                    "INSERT INTO categories (user_id, id, label, built_in, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 0, ?4, ?4)",
                    rusqlite::params![
                        context.user_id().to_string(),
                        entry.category.as_str(),
                        entry.label,
                        now,
                    ],
                )
                .map_err(|e| {
                    map_sqlite_error(e, &format!("category {} already exists", entry.category))
                })?;
            Ok(())
        })
    }

    fn set_label(
        &self,
        context: &UserContext,
        category: &Category,
        label: Option<&str>,
    ) -> Result<()> {
        self.database.with_connection(|connection| {
            self.ensure_seeded(connection, context)?;
            let user = context.user_id().to_string();
            let now = Utc::now().to_rfc3339();
            let affected = if category.is_built_in() {
                connection.execute(
                    "INSERT INTO categories (user_id, id, label, built_in, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 1, ?4, ?4)
                     ON CONFLICT (user_id, id) DO UPDATE
                        SET label = excluded.label, updated_at = excluded.updated_at",
                    rusqlite::params![user, category.as_str(), label, now],
                )
            } else {
                connection.execute(
                    "UPDATE categories SET label = ?3, updated_at = ?4
                      WHERE user_id = ?1 AND id = ?2",
                    rusqlite::params![user, category.as_str(), label, now],
                )
            }
            .map_err(|e| map_sqlite_error(e, "category label conflict"))?;
            if affected == 0 {
                return Err(RaError::NotFound(format!("category {category} not found")));
            }
            Ok(())
        })
    }

    fn remove(&self, context: &UserContext, category: &Category) -> Result<()> {
        if category.is_built_in() {
            return Err(RaError::Conflict(format!(
                "{category} is a built-in category and cannot be deleted"
            )));
        }
        self.database.with_connection(|connection| {
            self.ensure_seeded(connection, context)?;
            let affected = connection
                .execute(
                    "DELETE FROM categories WHERE user_id = ?1 AND id = ?2",
                    rusqlite::params![context.user_id().to_string(), category.as_str()],
                )
                .map_err(|e| map_sqlite_error(e, "category delete conflict"))?;
            if affected == 0 {
                return Err(RaError::NotFound(format!("category {category} not found")));
            }
            Ok(())
        })
    }
}
