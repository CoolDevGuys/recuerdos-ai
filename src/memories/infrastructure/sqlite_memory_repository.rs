//! `MemoryRepository` backed by SQLite.
//!
//! # Isolation
//!
//! Every statement here carries `user_id = ?` — not because the caller
//! passed an id, but because it came from an authenticated
//! [`UserContext`]. Update and delete use it in the `WHERE` clause too,
//! so naming another user's memory id affects zero rows rather than
//! theirs. `tests/identity_isolation.rs` asserts this from the outside.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::category::Category;
use crate::memories::domain::memory::{Entity, Memory, MemorySource};
use crate::memories::domain::memory_repository::{AuditEntry, AuditOperation, MemoryRepository};
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{MemoryId, UserId};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error, optional};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row};
use std::str::FromStr;
use std::sync::Arc;

const COLUMNS: &str = "id, user_id, content, category, tags, entities, confidence, \
                       source_client, source_session_id, created_at, updated_at, \
                       last_accessed_at, expires_at, superseded_by";

/// Phase 2 gives each user a single collection. It exists so the
/// embedding model can be pinned per collection (see V2__memories.sql);
/// user-visible collections are a later phase.
const DEFAULT_COLLECTION: &str = "main";

pub struct SqliteMemoryRepository {
    database: Arc<SqliteDatabase>,
    embedding_model: String,
    dimensions: usize,
}

impl SqliteMemoryRepository {
    pub fn new(database: Arc<SqliteDatabase>, embedding_model: &str, dimensions: usize) -> Self {
        Self {
            database,
            embedding_model: embedding_model.to_string(),
            dimensions,
        }
    }

    /// Returns this user's default collection, creating it on first use.
    ///
    /// Also the guard against a silently-changed embedding model: vectors
    /// built by different models are not comparable, so a mismatch fails
    /// loudly here rather than quietly returning nonsense rankings.
    fn collection_id(&self, connection: &Connection, user_id: UserId) -> Result<String> {
        let existing: Option<(String, String, i64)> = optional(connection.query_row(
            "SELECT id, embedding_model, dimensions FROM collections
             WHERE user_id = ?1 AND name = ?2",
            rusqlite::params![user_id.to_string(), DEFAULT_COLLECTION],
            |row| {
                Ok(Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                )))
            },
        ))?;

        if let Some((id, model, dimensions)) = existing {
            if model != self.embedding_model || dimensions as usize != self.dimensions {
                return Err(RaError::Validation(format!(
                    "this collection was built with embedding model {model:?} ({dimensions} \
                     dimensions) but the service is configured for {:?} ({}). Vectors from \
                     different models are not comparable — either restore the previous \
                     [embeddings] settings or re-index.",
                    self.embedding_model, self.dimensions
                )));
            }
            return Ok(id);
        }

        let id = uuid::Uuid::now_v7().to_string();
        connection
            .execute(
                "INSERT INTO collections (id, user_id, name, embedding_model, dimensions, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    id,
                    user_id.to_string(),
                    DEFAULT_COLLECTION,
                    self.embedding_model,
                    self.dimensions as i64,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(|e| map_sqlite_error(e, "collection already exists"))?;

        Ok(id)
    }

    fn write_audit(
        connection: &Connection,
        context: &UserContext,
        memory_id: MemoryId,
        operation: AuditOperation,
        actor: &str,
        detail: &str,
    ) -> Result<()> {
        connection
            .execute(
                "INSERT INTO memory_audit (id, memory_id, user_id, operation, actor, detail, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    uuid::Uuid::now_v7().to_string(),
                    memory_id.to_string(),
                    context.user_id().to_string(),
                    operation.as_str(),
                    actor,
                    detail,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(|e| map_sqlite_error(e, "audit write conflict"))?;
        Ok(())
    }
}

impl MemoryRepository for SqliteMemoryRepository {
    fn insert(&self, context: &UserContext, memory: &Memory, actor: &str) -> Result<()> {
        self.database.with_connection(|connection| {
            let collection_id = self.collection_id(connection, context.user_id())?;

            connection
                .execute(
                    "INSERT INTO memories (
                        id, user_id, collection_id, content, category, tags, entities,
                        confidence, source_client, source_session_id,
                        created_at, updated_at, last_accessed_at, expires_at, superseded_by
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    rusqlite::params![
                        memory.id().to_string(),
                        context.user_id().to_string(),
                        collection_id,
                        memory.content(),
                        memory.category().as_str(),
                        encode_tags(memory.tags()),
                        encode_entities(memory.entities()),
                        memory.confidence(),
                        memory.source().client,
                        memory.source().session_id,
                        memory.created_at().to_rfc3339(),
                        memory.updated_at().to_rfc3339(),
                        memory.last_accessed_at().map(|at| at.to_rfc3339()),
                        memory.expires_at().map(|at| at.to_rfc3339()),
                        memory.superseded_by().map(|id| id.to_string()),
                    ],
                )
                .map_err(|e| map_sqlite_error(e, "memory already exists"))?;

            Self::write_audit(
                connection,
                context,
                memory.id(),
                AuditOperation::Add,
                actor,
                memory.category().as_str(),
            )
        })
    }

    fn update(&self, context: &UserContext, memory: &Memory, actor: &str) -> Result<()> {
        self.database.with_connection(|connection| {
            let affected = connection
                .execute(
                    "UPDATE memories SET
                        content = ?3, category = ?4, tags = ?5, entities = ?6,
                        confidence = ?7, updated_at = ?8, expires_at = ?9, superseded_by = ?10
                     WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL",
                    rusqlite::params![
                        memory.id().to_string(),
                        context.user_id().to_string(),
                        memory.content(),
                        memory.category().as_str(),
                        encode_tags(memory.tags()),
                        encode_entities(memory.entities()),
                        memory.confidence(),
                        memory.updated_at().to_rfc3339(),
                        memory.expires_at().map(|at| at.to_rfc3339()),
                        memory.superseded_by().map(|id| id.to_string()),
                    ],
                )
                .map_err(|e| map_sqlite_error(e, "memory update conflict"))?;

            if affected == 0 {
                return Err(not_found(memory.id()));
            }

            let operation = if memory.is_superseded() {
                AuditOperation::Supersede
            } else {
                AuditOperation::Update
            };
            Self::write_audit(connection, context, memory.id(), operation, actor, "")
        })
    }

    fn delete(&self, context: &UserContext, id: MemoryId, actor: &str) -> Result<()> {
        self.database.with_connection(|connection| {
            // COALESCE: deleting twice keeps the first deletion time, for
            // the same reason revoking a key twice does.
            let affected = connection
                .execute(
                    "UPDATE memories SET deleted_at = COALESCE(deleted_at, ?3)
                     WHERE id = ?1 AND user_id = ?2",
                    rusqlite::params![
                        id.to_string(),
                        context.user_id().to_string(),
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(|e| map_sqlite_error(e, "memory delete conflict"))?;

            if affected == 0 {
                return Err(not_found(id));
            }

            Self::write_audit(connection, context, id, AuditOperation::Delete, actor, "")
        })
    }

    fn find(&self, context: &UserContext, id: MemoryId) -> Result<Option<Memory>> {
        self.database.with_connection(|connection| {
            optional(connection.query_row(
                &format!(
                    "SELECT {COLUMNS} FROM memories
                     WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL"
                ),
                rusqlite::params![id.to_string(), context.user_id().to_string()],
                row_to_memory,
            ))
        })
    }

    fn find_many(&self, context: &UserContext, ids: &[MemoryId]) -> Result<Vec<Memory>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        self.database.with_connection(|connection| {
            // Ids come from our own indexes, never from user input, but
            // they are still bound as parameters rather than interpolated.
            let placeholders = std::iter::repeat_n("?", ids.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT {COLUMNS} FROM memories
                 WHERE user_id = ?1 AND deleted_at IS NULL AND id IN ({placeholders})"
            );

            let mut parameters: Vec<String> = Vec::with_capacity(ids.len() + 1);
            parameters.push(context.user_id().to_string());
            parameters.extend(ids.iter().map(|id| id.to_string()));

            let mut statement = connection
                .prepare(&sql)
                .map_err(|e| map_sqlite_error(e, "memory fetch conflict"))?;

            let rows = statement
                .query_map(rusqlite::params_from_iter(parameters), row_to_memory)
                .map_err(|e| map_sqlite_error(e, "memory fetch conflict"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| map_sqlite_error(e, "memory fetch conflict"))?;

            rows.into_iter().collect::<Result<Vec<_>>>()
        })
    }

    fn list(&self, context: &UserContext, include_inactive: bool) -> Result<Vec<Memory>> {
        self.database.with_connection(|connection| {
            let sql = format!(
                "SELECT {COLUMNS} FROM memories
                 WHERE user_id = ?1 AND deleted_at IS NULL {}
                 ORDER BY created_at DESC",
                if include_inactive {
                    ""
                } else {
                    "AND superseded_by IS NULL"
                }
            );

            let mut statement = connection
                .prepare(&sql)
                .map_err(|e| map_sqlite_error(e, "memory list conflict"))?;

            let rows = statement
                .query_map([context.user_id().to_string()], row_to_memory)
                .map_err(|e| map_sqlite_error(e, "memory list conflict"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| map_sqlite_error(e, "memory list conflict"))?;

            rows.into_iter().collect::<Result<Vec<_>>>()
        })
    }

    fn audit_trail(&self, context: &UserContext, limit: usize) -> Result<Vec<AuditEntry>> {
        self.database.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT memory_id, operation, actor, detail, at FROM memory_audit
                     WHERE user_id = ?1 ORDER BY at DESC, rowid DESC LIMIT ?2",
                )
                .map_err(|e| map_sqlite_error(e, "audit read conflict"))?;

            let rows = statement
                .query_map(
                    rusqlite::params![context.user_id().to_string(), limit as i64],
                    |row| {
                        Ok(build_audit_entry(
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .map_err(|e| map_sqlite_error(e, "audit read conflict"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| map_sqlite_error(e, "audit read conflict"))?;

            rows.into_iter().collect::<Result<Vec<_>>>()
        })
    }

    fn touch_accessed(
        &self,
        context: &UserContext,
        ids: &[MemoryId],
        now: DateTime<Utc>,
    ) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        self.database.with_connection(|connection| {
            let placeholders = std::iter::repeat_n("?", ids.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "UPDATE memories SET last_accessed_at = ?1
                 WHERE user_id = ?2 AND id IN ({placeholders})"
            );

            let mut parameters: Vec<String> = Vec::with_capacity(ids.len() + 2);
            parameters.push(now.to_rfc3339());
            parameters.push(context.user_id().to_string());
            parameters.extend(ids.iter().map(|id| id.to_string()));

            connection
                .execute(&sql, rusqlite::params_from_iter(parameters))
                .map_err(|e| map_sqlite_error(e, "memory touch conflict"))?;
            Ok(())
        })
    }
}

fn not_found(id: MemoryId) -> RaError {
    // Deliberately the same error whether the memory belongs to someone
    // else or doesn't exist: probing for another user's ids must not be
    // distinguishable from probing for nothing.
    RaError::NotFound(format!("memory {id} not found"))
}

fn encode_tags(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string())
}

fn decode_tags(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn encode_entities(entities: &[Entity]) -> String {
    let as_json: Vec<serde_json::Value> = entities
        .iter()
        .map(|entity| serde_json::json!({"name": entity.name, "kind": entity.kind}))
        .collect();
    serde_json::to_string(&as_json).unwrap_or_else(|_| "[]".to_string())
}

fn decode_entities(raw: &str) -> Vec<Entity> {
    serde_json::from_str::<Vec<serde_json::Value>>(raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| {
            Some(Entity {
                name: value.get("name")?.as_str()?.to_string(),
                kind: value.get("kind")?.as_str()?.to_string(),
            })
        })
        .collect()
}

fn row_to_memory(row: &Row<'_>) -> rusqlite::Result<Result<Memory>> {
    Ok(build_memory(
        row.get::<_, String>(0)?,
        row.get::<_, String>(1)?,
        row.get::<_, String>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, String>(4)?,
        row.get::<_, String>(5)?,
        row.get::<_, f32>(6)?,
        row.get::<_, Option<String>>(7)?,
        row.get::<_, Option<String>>(8)?,
        row.get::<_, String>(9)?,
        row.get::<_, String>(10)?,
        row.get::<_, Option<String>>(11)?,
        row.get::<_, Option<String>>(12)?,
        row.get::<_, Option<String>>(13)?,
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_memory(
    id: String,
    user_id: String,
    content: String,
    category: String,
    tags: String,
    entities: String,
    confidence: f32,
    source_client: Option<String>,
    source_session_id: Option<String>,
    created_at: String,
    updated_at: String,
    last_accessed_at: Option<String>,
    expires_at: Option<String>,
    superseded_by: Option<String>,
) -> Result<Memory> {
    Ok(Memory::from_stored(
        MemoryId::from_str(&id)
            .map_err(|e| RaError::Internal(format!("stored memory id {id:?}: {e}")))?,
        UserId::from_str(&user_id)
            .map_err(|e| RaError::Internal(format!("stored user id {user_id:?}: {e}")))?,
        content,
        Category::from_stored(&category),
        decode_tags(&tags),
        decode_entities(&entities),
        confidence,
        MemorySource {
            client: source_client,
            session_id: source_session_id,
        },
        parse_timestamp(&created_at)?,
        parse_timestamp(&updated_at)?,
        last_accessed_at
            .as_deref()
            .map(parse_timestamp)
            .transpose()?,
        expires_at.as_deref().map(parse_timestamp).transpose()?,
        superseded_by
            .map(|id| {
                MemoryId::from_str(&id)
                    .map_err(|e| RaError::Internal(format!("stored superseded_by {id:?}: {e}")))
            })
            .transpose()?,
    ))
}

fn build_audit_entry(
    memory_id: String,
    operation: String,
    actor: String,
    detail: String,
    at: String,
) -> Result<AuditEntry> {
    Ok(AuditEntry {
        memory_id: MemoryId::from_str(&memory_id)
            .map_err(|e| RaError::Internal(format!("stored audit memory id: {e}")))?,
        operation: match operation.as_str() {
            "add" => AuditOperation::Add,
            "update" => AuditOperation::Update,
            "delete" => AuditOperation::Delete,
            "supersede" => AuditOperation::Supersede,
            other => {
                return Err(RaError::Internal(format!(
                    "stored audit operation {other:?} is unknown"
                )));
            }
        },
        actor,
        detail,
        at: parse_timestamp(&at)?,
    })
}

fn parse_timestamp(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|e| RaError::Internal(format!("stored timestamp {raw:?} is not rfc3339: {e}")))
}
