//! `VectorIndex` backed by sqlite-vec.
//!
//! # Why the table isn't in a migration
//!
//! A `vec0` table's dimensionality is fixed at creation (`float[384]`),
//! and the right number depends on the configured embedding model. A
//! migration would have to hardcode one. Instead the table is created on
//! first use from the embedder's own `dimensions()`, and the pin recorded
//! in `collections` is what stops the two drifting apart.
//!
//! # Isolation
//!
//! `user_id` is a vec0 `PARTITION KEY`, so a query for one user does not
//! merely filter another user's vectors out afterwards — it never scans
//! them. Isolation is a property of the index, not of remembering to add
//! a `WHERE`.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error};
use rusqlite::Connection;
use std::str::FromStr;
use std::sync::Arc;

pub struct SqliteVectorIndex {
    database: Arc<SqliteDatabase>,
    dimensions: usize,
}

/// The vec0 table name and its schema, in one place: both the normal
/// open path and the reindexer create it, and a drift between the two
/// would silently corrupt vectors.
pub(crate) const VEC_TABLE: &str = "vec_memories";

/// Creates the `vec0` table at `dimensions` if it is absent. The width is
/// fixed at creation, which is why changing the embedding model needs a
/// drop-and-recreate (see [`SqliteReindexer`](super::sqlite_reindexer)).
pub(crate) fn create_vec_table(connection: &Connection, dimensions: usize) -> Result<()> {
    connection
        .execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {VEC_TABLE} USING vec0(
                memory_id TEXT PRIMARY KEY,
                user_id TEXT PARTITION KEY,
                embedding float[{dimensions}]
            );"
        ))
        .map_err(|e| {
            RaError::Internal(format!(
                "failed to create the vector index (is the sqlite-vec extension \
                 available?): {e}"
            ))
        })
}

impl SqliteVectorIndex {
    /// Opens the index, creating the `vec0` table if absent.
    pub fn open(database: Arc<SqliteDatabase>, dimensions: usize) -> Result<Self> {
        database.with_connection(|connection| create_vec_table(connection, dimensions))?;

        Ok(Self {
            database,
            dimensions,
        })
    }

    fn check_dimensions(&self, embedding: &[f32]) -> Result<()> {
        if embedding.len() != self.dimensions {
            return Err(RaError::Internal(format!(
                "embedding has {} dimensions but the index expects {}",
                embedding.len(),
                self.dimensions
            )));
        }
        Ok(())
    }

    fn delete_row(connection: &Connection, context: &UserContext, id: MemoryId) -> Result<()> {
        connection
            .execute(
                "DELETE FROM vec_memories WHERE memory_id = ?1 AND user_id = ?2",
                rusqlite::params![id.to_string(), context.user_id().to_string()],
            )
            .map_err(|e| map_sqlite_error(e, "vector delete conflict"))?;
        Ok(())
    }
}

impl VectorIndex for SqliteVectorIndex {
    fn upsert(&self, context: &UserContext, id: MemoryId, embedding: &[f32]) -> Result<()> {
        self.check_dimensions(embedding)?;

        self.database.with_connection(|connection| {
            // vec0 has no UPSERT; delete-then-insert is the documented
            // idiom and is atomic enough under the single writer lock.
            Self::delete_row(connection, context, id)?;

            connection
                .execute(
                    "INSERT INTO vec_memories (memory_id, user_id, embedding)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![
                        id.to_string(),
                        context.user_id().to_string(),
                        to_bytes(embedding),
                    ],
                )
                .map_err(|e| map_sqlite_error(e, "vector insert conflict"))?;
            Ok(())
        })
    }

    fn remove(&self, context: &UserContext, id: MemoryId) -> Result<()> {
        self.database
            .with_connection(|connection| Self::delete_row(connection, context, id))
    }

    fn search(
        &self,
        context: &UserContext,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<MemoryId>> {
        self.check_dimensions(embedding)?;
        if limit == 0 {
            return Ok(Vec::new());
        }

        self.database.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT memory_id FROM vec_memories
                     WHERE embedding MATCH ?1 AND user_id = ?2 AND k = ?3
                     ORDER BY distance",
                )
                .map_err(|e| map_sqlite_error(e, "vector search conflict"))?;

            let rows = statement
                .query_map(
                    rusqlite::params![
                        to_bytes(embedding),
                        context.user_id().to_string(),
                        limit as i64,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|e| map_sqlite_error(e, "vector search conflict"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| map_sqlite_error(e, "vector search conflict"))?;

            rows.into_iter()
                .map(|id| {
                    MemoryId::from_str(&id).map_err(|e| {
                        RaError::Internal(format!("indexed memory id {id:?} is not a uuid: {e}"))
                    })
                })
                .collect()
        })
    }
}

/// sqlite-vec takes a vector as a little-endian f32 blob. `pub(crate)`
/// so the reindexer writes vectors in exactly this encoding rather than
/// its own copy that could drift.
pub(crate) fn to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
