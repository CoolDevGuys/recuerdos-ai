//! `EntityGraph` backed by SQLite edge tables (migration V8).
//!
//! # The hop is one recursive CTE
//!
//! `neighbours` walks the graph in a single depth-bounded recursive CTE
//! rather than a hop-at-a-time loop in Rust (Task 7.3.4, which put this on
//! the recall hot path). One statement means one round trip regardless of
//! hop count, and it lets SQLite do the frontier bookkeeping the app used
//! to: `reachable` grows the frontier undirected and bounded by depth,
//! `key_depth` keeps each key's nearest depth, and `hits` ranks the
//! memories those keys touch by hop distance. `seeds` is the cheap indexed
//! lookup that finds which of a query's entity names the graph knows,
//! before any hopping.
//!
//! # Isolation
//!
//! Every statement carries `user_id = ?` from the [`UserContext`], exactly
//! like the memory repository: a hop cannot reach, seed from, or invalidate
//! another user's edges. `storage_tests` asserts it even when two users
//! store the identical entity name.

// Ingest writes edges (`record`/`invalidate`, Task 7.3.2/7.3.3) and recall
// hops them (`seeds`/`neighbours`, Task 7.3.4). Only `remove` is still
// uncalled — the forgetter is not yet wired to it (Task 7.3.5) — so the
// crate allow stays until then.
#![allow(dead_code)]

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::entity_graph::{EntityGraph, Relation, normalise_predicate};
use crate::memories::domain::entity_key::EntityKey;
use crate::memories::domain::memory::Entity;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use rusqlite::types::Value;
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

    /// Replaces a memory's entity rows wholesale. Entities carry no time,
    /// so there is no history to preserve. Shared by `record` and
    /// `record_entities` so the two can never file entities differently.
    fn replace_entities(
        connection: &Connection,
        user: &str,
        memory: &str,
        entities: &[Entity],
    ) -> Result<()> {
        connection
            .execute(
                "DELETE FROM memory_entities WHERE user_id = ?1 AND memory_id = ?2",
                rusqlite::params![user, memory],
            )
            .map_err(|e| map_sqlite_error(e, "entity delete conflict"))?;

        let mut seen_keys = HashSet::new();
        for entity in entities {
            let key = EntityKey::new(&entity.name);
            // Skip a name that canonicalises to nothing, and dedupe a
            // memory that names one entity twice — the PK would reject the
            // second and abort the whole write.
            if key.is_empty() || !seen_keys.insert(key.as_str().to_string()) {
                continue;
            }
            connection
                .execute(
                    "INSERT INTO memory_entities (user_id, memory_id, entity_key, name, kind)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![user, memory, key.as_str(), entity.name, entity.kind],
                )
                .map_err(|e| map_sqlite_error(e, "entity insert conflict"))?;
        }
        Ok(())
    }

    /// Replaces a memory's still-open edges, preserving any a later memory
    /// already closed (Task 7.3.3): a fact superseded on Tuesday must not
    /// come back because its memory was re-written on Wednesday. Shared by
    /// `record` and `record_relations`.
    fn replace_open_relations(
        connection: &Connection,
        user: &str,
        memory: &str,
        relations: &[Relation],
        valid_from: DateTime<Utc>,
    ) -> Result<()> {
        connection
            .execute(
                "DELETE FROM memory_relations
                 WHERE user_id = ?1 AND memory_id = ?2 AND invalid_at IS NULL",
                rusqlite::params![user, memory],
            )
            .map_err(|e| map_sqlite_error(e, "relation delete conflict"))?;

        for relation in relations {
            let subject = EntityKey::new(&relation.subject);
            let object = EntityKey::new(&relation.object);
            // Canonicalise here too, not just trim: the candidate path
            // already normalises, but a direct caller (a backfill, a test)
            // must file the edge under the same predicate the invalidator
            // will look it up by, or the two silently miss.
            let predicate = normalise_predicate(&relation.predicate);
            // A relation needs two distinct, real endpoints and a predicate;
            // a self-edge or a blank end is noise, not a hop.
            if subject.is_empty() || object.is_empty() || predicate.is_empty() || subject == object
            {
                continue;
            }
            connection
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
        Ok(())
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

            let user = context.user_id().to_string();
            let memory = memory_id.to_string();

            Self::replace_entities(&transaction, &user, &memory, entities)?;
            Self::replace_open_relations(&transaction, &user, &memory, relations, valid_from)?;

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

    fn record_entities(
        &self,
        context: &UserContext,
        memory_id: MemoryId,
        entities: &[Entity],
    ) -> Result<()> {
        self.database.with_connection(|connection| {
            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin an entity record"))?;
            let user = context.user_id().to_string();
            let memory = memory_id.to_string();
            Self::replace_entities(&transaction, &user, &memory, entities)?;
            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit an entity record"))
        })
    }

    fn record_relations(
        &self,
        context: &UserContext,
        memory_id: MemoryId,
        relations: &[Relation],
        valid_from: DateTime<Utc>,
    ) -> Result<()> {
        self.database.with_connection(|connection| {
            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin a relation record"))?;
            let user = context.user_id().to_string();
            let memory = memory_id.to_string();
            Self::replace_open_relations(&transaction, &user, &memory, relations, valid_from)?;
            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit a relation record"))
        })
    }

    fn has_relations(&self, context: &UserContext, memory_id: MemoryId) -> Result<bool> {
        self.database.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM memory_relations WHERE user_id = ?1 AND memory_id = ?2
                     )",
                    rusqlite::params![context.user_id().to_string(), memory_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map(|exists| exists != 0)
                .map_err(|e| map_sqlite_error(e, "graph relation lookup conflict"))
        })
    }

    fn seeds(&self, context: &UserContext, candidates: &[EntityKey]) -> Result<Vec<EntityKey>> {
        // Distinct, non-empty candidate keys. Deduping before the query
        // keeps the IN-list small when a query repeats a name.
        let keys: Vec<String> = candidates
            .iter()
            .filter(|key| !key.is_empty())
            .map(|key| key.as_str().to_string())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        self.database.with_connection(|connection| {
            let placeholders = std::iter::repeat_n("?", keys.len())
                .collect::<Vec<_>>()
                .join(",");
            // Answered straight off `idx_memory_entities_user_key`.
            let sql = format!(
                "SELECT DISTINCT entity_key FROM memory_entities
                 WHERE user_id = ? AND entity_key IN ({placeholders})"
            );
            let mut params: Vec<String> = Vec::with_capacity(keys.len() + 1);
            params.push(context.user_id().to_string());
            params.extend(keys);

            let mut statement = connection
                .prepare(&sql)
                .map_err(|e| map_sqlite_error(e, "graph seed conflict"))?;
            let found = statement
                .query_map(rusqlite::params_from_iter(params), |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| map_sqlite_error(e, "graph seed conflict"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| map_sqlite_error(e, "graph seed conflict"))?;
            // Already canonical (that is how it was stored); `EntityKey::new`
            // is idempotent on a canonical string, so this rebuilds the key
            // without a second normalisation pass changing it.
            Ok(found.iter().map(|key| EntityKey::new(key)).collect())
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

        // Distinct, non-empty seed keys. An empty set means the query named
        // no known entity, so the leg is silent and recall is its two-leg
        // self.
        let seed_keys: Vec<String> = seeds
            .iter()
            .filter(|key| !key.is_empty())
            .map(|key| key.as_str().to_string())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if seed_keys.is_empty() {
            return Ok(Vec::new());
        }

        // A `None` as_of reads the graph as it stands now; resolving it to a
        // concrete instant gives the walk one liveness filter and excludes
        // any not-yet-valid edge from the current view.
        let as_of = as_of
            .map(|at| at.to_rfc3339())
            .unwrap_or_else(|| Utc::now().to_rfc3339());

        // Seeds sit at depth 0; a memory an edge reaches from a depth-`d`
        // key is `d + 1` hops out. To find memories up to `hops` away we
        // need reachable keys up to depth `hops - 1`, so the walk expands
        // while a key's depth is below that. `hops >= 1` here (guarded
        // above), so the subtraction cannot underflow.
        let max_depth = (hops - 1) as i64;

        // One depth-bounded recursive CTE, replacing the per-hop round trips
        // the app-side walk used to make (Task 7.3.4): `reachable` grows the
        // frontier undirected — arriving at a key by either endpoint lets it
        // continue from the other — bounded by `max_depth`; `key_depth`
        // keeps each key's nearest depth; and `hits` gathers every live edge
        // touching a reached key, ranking its memory by hop distance, then
        // by how many of those edges reach it, then by id for a stable
        // order. `UNION` (not `UNION ALL`) plus the depth bound guarantee
        // termination.
        let seed_values = std::iter::repeat_n("(?)", seed_keys.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "WITH RECURSIVE
             seeds(entity_key) AS (VALUES {seed_values}),
             reachable(entity_key, depth) AS (
                 SELECT entity_key, 0 FROM seeds
                 UNION
                 SELECT
                     CASE WHEN r.subject_key = rc.entity_key
                          THEN r.object_key ELSE r.subject_key END,
                     rc.depth + 1
                 FROM reachable rc
                 JOIN memory_relations r
                   ON r.subject_key = rc.entity_key OR r.object_key = rc.entity_key
                 WHERE r.user_id = ?
                   AND r.valid_from <= ? AND (r.invalid_at IS NULL OR r.invalid_at > ?)
                   AND rc.depth < ?
             ),
             key_depth(entity_key, depth) AS (
                 SELECT entity_key, MIN(depth) FROM reachable GROUP BY entity_key
             ),
             hits(memory_id, hop, edge_count) AS (
                 SELECT r.memory_id, MIN(kd.depth) + 1, COUNT(DISTINCT r.id)
                 FROM memory_relations r
                 JOIN key_depth kd
                   ON kd.entity_key = r.subject_key OR kd.entity_key = r.object_key
                 WHERE r.user_id = ?
                   AND r.valid_from <= ? AND (r.invalid_at IS NULL OR r.invalid_at > ?)
                 GROUP BY r.memory_id
             )
             SELECT memory_id FROM hits
             ORDER BY hop ASC, edge_count DESC, memory_id ASC
             LIMIT ?"
        );

        // Positional params, in the order the `?`s appear above.
        let user = context.user_id().to_string();
        let mut params: Vec<Value> = Vec::with_capacity(seed_keys.len() + 8);
        for key in &seed_keys {
            params.push(Value::Text(key.clone())); // seeds VALUES
        }
        params.push(Value::Text(user.clone())); // reachable: user
        params.push(Value::Text(as_of.clone())); // reachable: valid_from
        params.push(Value::Text(as_of.clone())); // reachable: invalid_at
        params.push(Value::Integer(max_depth)); // reachable: depth bound
        params.push(Value::Text(user)); // hits: user
        params.push(Value::Text(as_of.clone())); // hits: valid_from
        params.push(Value::Text(as_of)); // hits: invalid_at
        params.push(Value::Integer(limit as i64)); // LIMIT

        self.database.with_connection(|connection| {
            let mut statement = connection
                .prepare(&sql)
                .map_err(|e| map_sqlite_error(e, "graph hop conflict"))?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(params), |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| map_sqlite_error(e, "graph hop conflict"))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| map_sqlite_error(e, "graph hop conflict"))?;

            let mut ordered = Vec::with_capacity(rows.len());
            for memory_id in rows {
                ordered.push(MemoryId::from_str(&memory_id).map_err(|e| {
                    RaError::Internal(format!("graph memory id {memory_id:?} is not a uuid: {e}"))
                })?);
            }
            Ok(ordered)
        })
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
                // The same canonical form `record` filed the edge under, so
                // an assertion and the edge it contradicts always match.
                let predicate = normalise_predicate(&relation.predicate);
                if subject.is_empty() || object.is_empty() || predicate.is_empty() {
                    continue;
                }
                // Close only the edges this assertion *contradicts*: same
                // subject and predicate, a different object, and not
                // already closed. Re-affirming the same object touches
                // nothing, which is what makes a re-run idempotent.
                //
                // `memory_id <> by` keeps a memory from invalidating its own
                // edges: a memory that asserts two objects for one
                // subject+predicate is internally contradictory, but that is
                // the model's business, not grounds for the edge to retire
                // itself the instant it is written.
                transaction
                    .execute(
                        "UPDATE memory_relations
                         SET invalid_at = ?1, invalidated_by = ?2
                         WHERE user_id = ?3 AND subject_key = ?4 AND predicate = ?5
                           AND object_key <> ?6 AND invalid_at IS NULL AND memory_id <> ?7",
                        rusqlite::params![
                            at,
                            by,
                            user,
                            subject.as_str(),
                            predicate,
                            object.as_str(),
                            by,
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
