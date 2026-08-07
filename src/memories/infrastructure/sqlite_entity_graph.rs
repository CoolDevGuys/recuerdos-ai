//! `EntityGraph` backed by SQLite edge tables (migration V8).
//!
//! # Why app-side traversal, for now
//!
//! `neighbours` walks the graph a hop at a time in Rust rather than in one
//! recursive CTE. At personal scale — a hop count of two, a corpus bounded
//! by `MAX_MEMORIES_PER_CATEGORY` — the frontier stays tiny and the extra
//! round trips are noise, and a straight-line loop is far easier to read
//! and to prove correct than a self-joining CTE. Task 7.3.4, which puts
//! this on the recall hot path, is where the CTE earns its complexity.
//!
//! # Isolation
//!
//! Every statement carries `user_id = ?` from the [`UserContext`], exactly
//! like the memory repository: a hop cannot reach, seed from, or invalidate
//! another user's edges. `storage_tests` asserts it even when two users
//! store the identical entity name.

// Exercised by `storage_tests`, but constructed-and-never-called in the
// crate build until recall consults it (Task 7.3.4) and ingest writes to
// it (7.3.2); the allow comes off then.
#![allow(dead_code)]

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::entity_graph::{EntityGraph, Relation};
use crate::memories::domain::entity_key::EntityKey;
use crate::memories::domain::memory::Entity;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

pub struct SqliteEntityGraph {
    database: Arc<SqliteDatabase>,
}

impl SqliteEntityGraph {
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    fn delete_memory_rows(
        connection: &Connection,
        context: &UserContext,
        memory_id: MemoryId,
    ) -> Result<()> {
        let user = context.user_id().to_string();
        let memory = memory_id.to_string();
        connection
            .execute(
                "DELETE FROM memory_entities WHERE user_id = ?1 AND memory_id = ?2",
                rusqlite::params![user, memory],
            )
            .map_err(|e| map_sqlite_error(e, "entity delete conflict"))?;
        connection
            .execute(
                "DELETE FROM memory_relations WHERE user_id = ?1 AND memory_id = ?2",
                rusqlite::params![user, memory],
            )
            .map_err(|e| map_sqlite_error(e, "relation delete conflict"))?;
        Ok(())
    }

    /// One hop of the walk: the memories whose edges touch `frontier`, and
    /// the fresh keys those edges reach. `visited` guards against walking
    /// back over a key already expanded.
    fn expand(
        connection: &Connection,
        context: &UserContext,
        frontier: &[String],
        as_of: Option<&str>,
        visited: &mut HashSet<String>,
    ) -> Result<(Vec<MemoryId>, Vec<String>)> {
        let placeholders = std::iter::repeat_n("?", frontier.len())
            .collect::<Vec<_>>()
            .join(",");
        // An edge is live at the read point: born, and not yet closed —
        // or closed only after it. With no `as_of` the current graph is
        // "everything still open".
        let liveness = if as_of.is_some() {
            "valid_from <= ? AND (invalid_at IS NULL OR invalid_at > ?)"
        } else {
            "invalid_at IS NULL"
        };
        let sql = format!(
            "SELECT memory_id, subject_key, object_key FROM memory_relations
             WHERE user_id = ? AND (subject_key IN ({placeholders}) OR object_key IN ({placeholders}))
               AND {liveness}"
        );

        let mut params: Vec<String> = Vec::with_capacity(frontier.len() * 2 + 3);
        params.push(context.user_id().to_string());
        params.extend(frontier.iter().cloned()); // subject_key IN (...)
        params.extend(frontier.iter().cloned()); // object_key IN (...)
        if let Some(at) = as_of {
            params.push(at.to_string());
            params.push(at.to_string());
        }

        let mut statement = connection
            .prepare(&sql)
            .map_err(|e| map_sqlite_error(e, "graph hop conflict"))?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(params), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| map_sqlite_error(e, "graph hop conflict"))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| map_sqlite_error(e, "graph hop conflict"))?;

        let mut memories = Vec::new();
        let mut next = Vec::new();
        let mut seen_here = HashSet::new();
        for (memory_id, subject_key, object_key) in rows {
            let id = MemoryId::from_str(&memory_id).map_err(|e| {
                RaError::Internal(format!("graph memory id {memory_id:?} is not a uuid: {e}"))
            })?;
            if seen_here.insert(id) {
                memories.push(id);
            }
            // Both endpoints feed the next hop; `visited` keeps the walk
            // from looping. The edge was matched by one of them, but which
            // does not matter — the walk is undirected.
            for key in [subject_key, object_key] {
                if visited.insert(key.clone()) {
                    next.push(key);
                }
            }
        }
        Ok((memories, next))
    }
}

impl EntityGraph for SqliteEntityGraph {
    fn record(
        &self,
        context: &UserContext,
        memory_id: MemoryId,
        entities: &[Entity],
        relations: &[Relation],
        valid_from: DateTime<Utc>,
    ) -> Result<()> {
        self.database.with_connection(|connection| {
            // One transaction for the whole projection: a memory's
            // entities and edges are written together or not at all, so a
            // failure never leaves half a graph pointing at it.
            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin a graph record"))?;

            // Replace rather than append: recording is how an edit or a
            // re-ingest keeps the projection matching the memory.
            Self::delete_memory_rows(&transaction, context, memory_id)?;

            let user = context.user_id().to_string();
            let memory = memory_id.to_string();

            let mut seen_keys = HashSet::new();
            for entity in entities {
                let key = EntityKey::new(&entity.name);
                // Skip a name that canonicalises to nothing, and dedupe a
                // memory that names one entity twice — the PK would reject
                // the second and abort the whole record.
                if key.is_empty() || !seen_keys.insert(key.as_str().to_string()) {
                    continue;
                }
                transaction
                    .execute(
                        "INSERT INTO memory_entities (user_id, memory_id, entity_key, name, kind)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![user, memory, key.as_str(), entity.name, entity.kind],
                    )
                    .map_err(|e| map_sqlite_error(e, "entity insert conflict"))?;
            }

            for relation in relations {
                let subject = EntityKey::new(&relation.subject);
                let object = EntityKey::new(&relation.object);
                let predicate = relation.predicate.trim();
                // A relation needs two distinct, real endpoints and a
                // predicate; a self-edge or a blank end is noise, not a hop.
                if subject.is_empty()
                    || object.is_empty()
                    || predicate.is_empty()
                    || subject == object
                {
                    continue;
                }
                transaction
                    .execute(
                        "INSERT INTO memory_relations (
                            id, user_id, memory_id, subject_key, predicate, object_key,
                            subject_name, object_name, valid_from, invalid_at, invalidated_by
                          ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL)",
                        rusqlite::params![
                            uuid::Uuid::now_v7().to_string(),
                            user,
                            memory,
                            subject.as_str(),
                            predicate,
                            object.as_str(),
                            relation.subject.trim(),
                            relation.object.trim(),
                            valid_from.to_rfc3339(),
                        ],
                    )
                    .map_err(|e| map_sqlite_error(e, "relation insert conflict"))?;
            }

            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit a graph record"))
        })
    }

    fn remove(&self, context: &UserContext, memory_id: MemoryId) -> Result<()> {
        self.database.with_connection(|connection| {
            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin a graph remove"))?;
            Self::delete_memory_rows(&transaction, context, memory_id)?;
            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit a graph remove"))
        })
    }

    fn neighbours(
        &self,
        context: &UserContext,
        seeds: &[EntityKey],
        hops: usize,
        as_of: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<MemoryId>> {
        if hops == 0 || limit == 0 {
            return Ok(Vec::new());
        }

        let mut visited: HashSet<String> = seeds
            .iter()
            .filter(|key| !key.is_empty())
            .map(|key| key.as_str().to_string())
            .collect();
        if visited.is_empty() {
            return Ok(Vec::new());
        }
        let mut frontier: Vec<String> = visited.iter().cloned().collect();

        let as_of = as_of.map(|at| at.to_rfc3339());
        let mut ordered: Vec<MemoryId> = Vec::new();
        let mut seen: HashSet<MemoryId> = HashSet::new();

        self.database.with_connection(|connection| {
            // BFS: each iteration is one hop, so memories are discovered
            // nearest-first and truncating to `limit` keeps the closest.
            for _ in 0..hops {
                if frontier.is_empty() {
                    break;
                }
                let (memories, next) = Self::expand(
                    connection,
                    context,
                    &frontier,
                    as_of.as_deref(),
                    &mut visited,
                )?;
                for id in memories {
                    if seen.insert(id) {
                        ordered.push(id);
                    }
                }
                frontier = next;
            }
            Ok(())
        })?;

        ordered.truncate(limit);
        Ok(ordered)
    }

    fn invalidate(
        &self,
        context: &UserContext,
        superseding: &[Relation],
        at: DateTime<Utc>,
        by: MemoryId,
    ) -> Result<()> {
        self.database.with_connection(|connection| {
            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin an invalidation"))?;

            let user = context.user_id().to_string();
            let at = at.to_rfc3339();
            let by = by.to_string();

            for relation in superseding {
                let subject = EntityKey::new(&relation.subject);
                let object = EntityKey::new(&relation.object);
                let predicate = relation.predicate.trim();
                if subject.is_empty() || object.is_empty() || predicate.is_empty() {
                    continue;
                }
                // Close only the edges this assertion *contradicts*: same
                // subject and predicate, a different object, and not
                // already closed. Re-affirming the same object touches
                // nothing, which is what makes a re-run idempotent.
                transaction
                    .execute(
                        "UPDATE memory_relations
                         SET invalid_at = ?1, invalidated_by = ?2
                         WHERE user_id = ?3 AND subject_key = ?4 AND predicate = ?5
                           AND object_key <> ?6 AND invalid_at IS NULL",
                        rusqlite::params![
                            at,
                            by,
                            user,
                            subject.as_str(),
                            predicate,
                            object.as_str(),
                        ],
                    )
                    .map_err(|e| map_sqlite_error(e, "invalidation conflict"))?;
            }

            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit an invalidation"))
        })
    }
}
