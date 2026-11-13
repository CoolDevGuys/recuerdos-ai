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
use crate::memories::domain::applies_to::{AppliesTo, ProjectRef, Target};
use crate::memories::domain::category::Category;
use crate::memories::domain::kind::Kind;
use crate::memories::domain::legacy_structure::legacy_structure;
use crate::memories::domain::memory::{Entity, Memory, MemorySource};
use crate::memories::domain::memory_repository::{AuditEntry, AuditOperation, MemoryRepository};
use crate::memories::domain::memory_structure::{GoalStatus, MemoryStructure, Origin};
use crate::memories::domain::topic::Topic;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{MemoryId, ProjectId, UserId};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error, optional};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row};
use std::str::FromStr;
use std::sync::Arc;

const COLUMNS: &str = "id, user_id, content, category, subcategory, tags, entities, confidence, \
                        source_client, source_session_id, created_at, updated_at, \
                        last_accessed_at, access_count, importance, expires_at, \
                        superseded_by, kind, topic, target, project_id, \
                        (SELECT name FROM projects WHERE projects.id = memories.project_id), \
                        origin, goal_status, needs_review";

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
                return Err(self.pin_mismatch(&model, dimensions));
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

    /// The actionable error for a store whose embedding pin no longer
    /// matches the configured model — shared by the lazy write-path guard
    /// ([`collection_id`](Self::collection_id)) and the startup check
    /// ([`verify_pin`](Self::verify_pin)) so both say the same thing.
    fn pin_mismatch(&self, stored_model: &str, stored_dimensions: i64) -> RaError {
        RaError::Validation(format!(
            "this store was built with embedding model {stored_model:?} ({stored_dimensions} \
             dimensions) but the service is configured for {:?} ({}). Vectors from different \
             models — or built from different [embeddings].document_text — are not comparable. \
             Either restore the previous [embeddings] settings, or run `recuerdos-ai reindex` \
             (with the daemon stopped) to re-embed every memory under the new settings in \
             place.",
            self.embedding_model, self.dimensions
        ))
    }

    /// Fails fast at startup if any collection in the store was built with
    /// a different embedding model or dimensionality than the service is
    /// now configured for.
    ///
    /// The per-collection guard in [`collection_id`](Self::collection_id)
    /// only runs on the write path, so without this a provider change
    /// surfaces much later as a raw dimension-mismatch from sqlite-vec on
    /// the first recall. Checked here, the daemon refuses to start and
    /// names the fix instead.
    pub fn verify_pin(&self) -> Result<()> {
        self.database.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT DISTINCT embedding_model, dimensions FROM collections")
                .map_err(|e| map_sqlite_error(e, "reading the embedding pin"))?;

            let pins = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|e| map_sqlite_error(e, "reading the embedding pin"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| map_sqlite_error(e, "reading the embedding pin"))?;

            for (model, dimensions) in pins {
                if model != self.embedding_model || dimensions as usize != self.dimensions {
                    return Err(self.pin_mismatch(&model, dimensions));
                }
            }
            Ok(())
        })
    }

    /// Gives every memory stored before V11 its structure, derived from its
    /// category (ai-specs/memory-structure-plan.md §8), and puts any user
    /// topic that mapping produces in its owner's list. Returns how many
    /// memories it filled.
    ///
    /// Run at startup. Idempotent and cheap once done: it only reads rows
    /// whose `kind` is still NULL, through a partial index. Every row is
    /// filled — superseded and forgotten ones too, so history reads the same
    /// way — in one transaction, so a crash leaves all of them or none.
    /// `updated_at` is not touched: deriving a structure is not an edit, and
    /// the web UI's `If-Match` must not start refusing edits to memories the
    /// user never changed.
    pub fn backfill_structure(&self) -> Result<usize> {
        self.database.with_connection(|connection| {
            let rows: Vec<(String, String, String, String)> = {
                let mut statement = connection
                    .prepare("SELECT id, user_id, category, tags FROM memories WHERE kind IS NULL")
                    .map_err(|e| map_sqlite_error(e, "reading memories to backfill"))?;
                statement
                    .query_map([], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })
                    .map_err(|e| map_sqlite_error(e, "reading memories to backfill"))?
                    .collect::<std::result::Result<_, _>>()
                    .map_err(|e| map_sqlite_error(e, "reading memories to backfill"))?
            };
            if rows.is_empty() {
                return Ok(0);
            }

            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin the structure backfill"))?;
            for (id, user_id, category, tags) in &rows {
                let structure =
                    legacy_structure(&Category::from_stored(category), &decode_tags(tags));
                transaction
                    .execute(
                        "UPDATE memories SET kind = ?2, topic = ?3, target = ?4, origin = ?5,
                                goal_status = ?6, needs_review = ?7
                          WHERE id = ?1 AND kind IS NULL",
                        rusqlite::params![
                            id,
                            structure.kind().as_str(),
                            structure.topic().as_str(),
                            structure.applies_to().target().as_str(),
                            structure.origin().as_str(),
                            structure.goal_status().map(|status| status.as_str()),
                            structure.needs_review(),
                        ],
                    )
                    .map_err(|e| map_sqlite_error(e, "structure backfill conflict"))?;
                let owner = UserId::from_str(user_id)
                    .map_err(|e| RaError::Internal(format!("stored user id {user_id:?}: {e}")))?;
                record_topic(&transaction, owner, structure.topic())?;
            }
            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit the structure backfill"))?;
            Ok(rows.len())
        })
    }

    fn write_update(
        connection: &Connection,
        context: &UserContext,
        memory: &Memory,
        operation: AuditOperation,
        actor: &str,
        detail: &str,
    ) -> Result<()> {
        let affected = connection
            .execute(
                "UPDATE memories SET
                    content = ?3, category = ?4, subcategory = ?5, tags = ?6, entities = ?7,
                    confidence = ?8, updated_at = ?9, expires_at = ?10, superseded_by = ?11,
                    kind = ?12, topic = ?13, target = ?14, project_id = ?15, origin = ?16,
                    goal_status = ?17, needs_review = ?18
                  WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL",
                rusqlite::params![
                    memory.id().to_string(),
                    context.user_id().to_string(),
                    memory.content(),
                    memory.category().as_str(),
                    memory.subcategory(),
                    encode_tags(memory.tags()),
                    encode_entities(memory.entities()),
                    memory.confidence(),
                    memory.updated_at().to_rfc3339(),
                    memory.expires_at().map(|at| at.to_rfc3339()),
                    memory.superseded_by().map(|id| id.to_string()),
                    memory.structure().kind().as_str(),
                    memory.structure().topic().as_str(),
                    memory.structure().applies_to().target().as_str(),
                    project_id(memory.structure()),
                    memory.structure().origin().as_str(),
                    memory
                        .structure()
                        .goal_status()
                        .map(|status| status.as_str()),
                    memory.structure().needs_review(),
                ],
            )
            .map_err(|e| map_sqlite_error(e, "memory update conflict"))?;

        if affected == 0 {
            return Err(not_found(memory.id()));
        }
        record_topic(connection, context.user_id(), memory.structure().topic())?;

        Self::write_audit(connection, context, memory.id(), operation, actor, detail)
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
                        id, user_id, collection_id, content, category, subcategory, tags, entities,
                        confidence, source_client, source_session_id,
                        created_at, updated_at, last_accessed_at, expires_at, superseded_by,
                        kind, topic, target, project_id, origin, goal_status, needs_review
                      ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                                ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
                    rusqlite::params![
                        memory.id().to_string(),
                        context.user_id().to_string(),
                        collection_id,
                        memory.content(),
                        memory.category().as_str(),
                        memory.subcategory(),
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
                        memory.structure().kind().as_str(),
                        memory.structure().topic().as_str(),
                        memory.structure().applies_to().target().as_str(),
                        project_id(memory.structure()),
                        memory.structure().origin().as_str(),
                        memory.structure().goal_status().map(|status| status.as_str()),
                        memory.structure().needs_review(),
                    ],
                )
                .map_err(|e| map_sqlite_error(e, "memory already exists"))?;
            record_topic(connection, context.user_id(), memory.structure().topic())?;

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
            let operation = if memory.is_superseded() {
                AuditOperation::Supersede
            } else {
                AuditOperation::Update
            };
            Self::write_update(connection, context, memory, operation, actor, "")
        })
    }

    fn update_many(
        &self,
        context: &UserContext,
        memories: &[Memory],
        actor: &str,
        detail: &str,
    ) -> Result<()> {
        self.database.with_connection(|connection| {
            // All or nothing: a bulk move that stopped halfway would leave a
            // category half-merged, which is neither what was asked nor what
            // was there.
            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin a bulk update"))?;
            for memory in memories {
                // Always `Update`: a bulk edit re-files a memory, superseded or
                // not, and says so in `detail`; it never supersedes one.
                Self::write_update(
                    &transaction,
                    context,
                    memory,
                    AuditOperation::Update,
                    actor,
                    detail,
                )?;
            }
            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit a bulk update"))
        })
    }

    fn delete(&self, context: &UserContext, id: MemoryId, actor: &str, reason: &str) -> Result<()> {
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

            Self::write_audit(
                connection,
                context,
                id,
                AuditOperation::Delete,
                actor,
                reason,
            )
        })
    }

    fn restore(&self, context: &UserContext, id: MemoryId, actor: &str) -> Result<Memory> {
        self.database.with_connection(|connection| {
            let affected = connection
                .execute(
                    "UPDATE memories SET deleted_at = NULL
                     WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NOT NULL",
                    rusqlite::params![id.to_string(), context.user_id().to_string()],
                )
                .map_err(|e| map_sqlite_error(e, "memory restore conflict"))?;

            if affected == 0 {
                return Err(not_found(id));
            }

            Self::write_audit(connection, context, id, AuditOperation::Restore, actor, "")?;

            connection
                .query_row(
                    &format!(
                        "SELECT {COLUMNS} FROM memories
                         WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL"
                    ),
                    rusqlite::params![id.to_string(), context.user_id().to_string()],
                    row_to_memory,
                )
                .map_err(|e| map_sqlite_error(e, "memory restore conflict"))?
        })
    }

    fn supersede(
        &self,
        context: &UserContext,
        superseded: MemoryId,
        replacement: MemoryId,
        actor: &str,
        reason: &str,
    ) -> Result<()> {
        self.database.with_connection(|connection| {
            let affected = connection
                .execute(
                    // `deleted_at IS NULL`: a memory the user already
                    // deleted must not be quietly resurrected into a
                    // supersede chain.
                    "UPDATE memories SET superseded_by = ?3, updated_at = ?4
                     WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL",
                    rusqlite::params![
                        superseded.to_string(),
                        context.user_id().to_string(),
                        replacement.to_string(),
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(|e| map_sqlite_error(e, "memory supersede conflict"))?;

            if affected == 0 {
                return Err(not_found(superseded));
            }

            Self::write_audit(
                connection,
                context,
                superseded,
                AuditOperation::Supersede,
                actor,
                reason,
            )
        })
    }

    fn merge(
        &self,
        context: &UserContext,
        superseded: &[MemoryId],
        replacement: MemoryId,
        actor: &str,
        reason: &str,
    ) -> Result<usize> {
        self.database.with_connection(|connection| {
            // An explicit transaction, unlike the single-statement
            // mutations above: half a merge is a worse state than either
            // end of it.
            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin a merge"))?;

            let mut retired = 0usize;
            for id in superseded {
                // `superseded_by IS NULL` as well as `deleted_at IS NULL`:
                // a cluster is a snapshot, and re-pointing a memory that
                // something else already retired would rewrite history.
                let affected = transaction
                    .execute(
                        "UPDATE memories SET superseded_by = ?3, updated_at = ?4
                         WHERE id = ?1 AND user_id = ?2
                           AND deleted_at IS NULL AND superseded_by IS NULL",
                        rusqlite::params![
                            id.to_string(),
                            context.user_id().to_string(),
                            replacement.to_string(),
                            Utc::now().to_rfc3339(),
                        ],
                    )
                    .map_err(|e| map_sqlite_error(e, "memory merge conflict"))?;

                if affected == 0 {
                    tracing::debug!(
                        memory_id = %id,
                        "skipping a cluster member that is already gone or already retired"
                    );
                    continue;
                }

                Self::write_audit(
                    &transaction,
                    context,
                    *id,
                    AuditOperation::Merge,
                    actor,
                    reason,
                )?;
                retired += 1;
            }

            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit a merge"))?;

            Ok(retired)
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

    fn generation(&self, context: &UserContext) -> Result<i64> {
        self.database.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COALESCE(MAX(rowid), 0) FROM memory_audit WHERE user_id = ?1",
                    rusqlite::params![context.user_id().to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|e| map_sqlite_error(e, "generation read conflict"))
        })
    }

    fn set_importance(&self, context: &UserContext, scores: &[(MemoryId, f32)]) -> Result<()> {
        if scores.is_empty() {
            return Ok(());
        }

        self.database.with_connection(|connection| {
            // One transaction for the whole batch. Not for atomicity —
            // half-updated decay scores would be harmless — but because
            // SQLite commits each statement separately otherwise, and a
            // nightly job over a few thousand memories would mean a few
            // thousand fsyncs.
            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin an importance update"))?;

            {
                let mut statement = transaction
                    .prepare(
                        "UPDATE memories SET importance = ?3
                         WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL",
                    )
                    .map_err(|e| map_sqlite_error(e, "importance update conflict"))?;

                for (id, importance) in scores {
                    statement
                        .execute(rusqlite::params![
                            id.to_string(),
                            context.user_id().to_string(),
                            importance,
                        ])
                        .map_err(|e| map_sqlite_error(e, "importance update conflict"))?;
                }
            }

            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit an importance update"))
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
                // The count is incremented in SQL rather than read,
                // bumped and written back: two concurrent recalls of the
                // same memory must both be counted, and a read-modify-write
                // would lose one of them.
                "UPDATE memories SET last_accessed_at = ?1, access_count = access_count + 1
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
        row.get::<_, Option<String>>(4)?,
        row.get::<_, String>(5)?,
        row.get::<_, String>(6)?,
        row.get::<_, f32>(7)?,
        row.get::<_, Option<String>>(8)?,
        row.get::<_, Option<String>>(9)?,
        row.get::<_, String>(10)?,
        row.get::<_, String>(11)?,
        row.get::<_, Option<String>>(12)?,
        row.get::<_, u32>(13)?,
        row.get::<_, f32>(14)?,
        row.get::<_, Option<String>>(15)?,
        row.get::<_, Option<String>>(16)?,
        StoredStructure {
            kind: row.get(17)?,
            topic: row.get(18)?,
            target: row.get(19)?,
            project_id: row.get(20)?,
            project_name: row.get(21)?,
            origin: row.get(22)?,
            goal_status: row.get(23)?,
            needs_review: row.get(24)?,
        },
    ))
}

/// The structure columns as stored. All `None` on a row the V11 backfill has
/// not reached yet.
struct StoredStructure {
    kind: Option<String>,
    topic: Option<String>,
    target: Option<String>,
    project_id: Option<String>,
    project_name: Option<String>,
    origin: Option<String>,
    goal_status: Option<String>,
    needs_review: bool,
}

impl StoredStructure {
    /// The memory's structure. A row without one (not backfilled yet), or
    /// with one this build cannot read, is read through the same mapping the
    /// backfill applies, so no reader ever sees a memory without one.
    fn into_structure(self, category: &Category, tags: &[String]) -> MemoryStructure {
        self.parse()
            .unwrap_or_else(|| legacy_structure(category, tags))
    }

    fn parse(self) -> Option<MemoryStructure> {
        let kind: Kind = self.kind?.parse().ok()?;
        let topic = Topic::from_stored(&self.topic?);
        let applies_to = match self.target?.parse::<Target>().ok()? {
            Target::Me => AppliesTo::Me,
            Target::General => AppliesTo::General,
            Target::Project => {
                let id = ProjectId::from_str(&self.project_id?).ok()?;
                AppliesTo::Project(ProjectRef {
                    id,
                    name: self.project_name.unwrap_or_else(|| id.to_string()),
                })
            }
        };
        let origin: Origin = self.origin?.parse().ok()?;
        let goal_status = match self.goal_status {
            Some(raw) => Some(raw.parse::<GoalStatus>().ok()?),
            None => None,
        };
        Some(MemoryStructure::from_stored(
            kind,
            topic,
            applies_to,
            origin,
            goal_status,
            self.needs_review,
        ))
    }
}

fn project_id(structure: &MemoryStructure) -> Option<String> {
    structure
        .applies_to()
        .project()
        .map(|project| project.id.to_string())
}

/// Puts a user's own topic in their list the first time a memory uses it, so
/// no memory names a topic its owner cannot see or move.
fn record_topic(connection: &Connection, user_id: UserId, topic: &Topic) -> Result<()> {
    if topic.is_built_in() {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    connection
        .execute(
            "INSERT OR IGNORE INTO topics (user_id, id, label, built_in, created_at, updated_at)
             VALUES (?1, ?2, NULL, 0, ?3, ?3)",
            rusqlite::params![user_id.to_string(), topic.as_str(), now],
        )
        .map_err(|e| map_sqlite_error(e, "topic already exists"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_memory(
    id: String,
    user_id: String,
    content: String,
    category: String,
    subcategory: Option<String>,
    tags: String,
    entities: String,
    confidence: f32,
    source_client: Option<String>,
    source_session_id: Option<String>,
    created_at: String,
    updated_at: String,
    last_accessed_at: Option<String>,
    access_count: u32,
    importance: f32,
    expires_at: Option<String>,
    superseded_by: Option<String>,
    structure: StoredStructure,
) -> Result<Memory> {
    let category = Category::from_stored(&category);
    let tags = decode_tags(&tags);
    let structure = structure.into_structure(&category, &tags);
    Ok(Memory::from_stored(
        MemoryId::from_str(&id)
            .map_err(|e| RaError::Internal(format!("stored memory id {id:?}: {e}")))?,
        UserId::from_str(&user_id)
            .map_err(|e| RaError::Internal(format!("stored user id {user_id:?}: {e}")))?,
        content,
        category,
        subcategory,
        structure,
        tags,
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
        access_count,
        importance,
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
            "merge" => AuditOperation::Merge,
            "restore" => AuditOperation::Restore,
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
