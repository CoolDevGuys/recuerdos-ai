//! Re-embeds every stored memory under a new embedding model, in place.
//!
//! # The problem it solves
//!
//! Vectors from two different embedding models are not comparable, so a
//! collection *pins* the model that built it and the store refuses to
//! open under a different one (see
//! `sqlite_memory_repository::collection_id`). Switching model or
//! provider — local `bge-small` to Gemini `text-embedding-004`, say —
//! therefore means every stored vector is now the wrong model's, and the
//! guard rightly blocks it.
//!
//! Reindexing is the way through that does not throw the memories away:
//! the *content* is model-independent and still in the `memories` table,
//! so it is re-embedded with the new model, the vector index rebuilt, and
//! the pin updated. Recall then works against the new model with the same
//! memories.
//!
//! # Why one transaction
//!
//! A half-finished reindex — some vectors rewritten, the pin still old,
//! or the pin updated over a partial index — is a worse state than either
//! end. So the whole rebuild runs in a single transaction: it either
//! commits wholesale or leaves the old store exactly as it was, and a
//! failed run can simply be retried. No memory content is ever at risk,
//! because content is only read here, never rewritten.
//!
//! # Run it with the daemon stopped
//!
//! It drops and recreates the vector table and holds the write lock for
//! the duration. A running daemon holding the same table open would make
//! the drop fail. This is a maintenance command, not a hot path.

use super::sqlite_vector_index::{VEC_TABLE, create_vec_table, to_bytes};
use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error};
use std::sync::Arc;

/// How many memories are embedded per batch. Batching matters for a
/// remote provider — it is one HTTP round trip instead of one per memory
/// — and costs nothing for the local model.
const BATCH: usize = 128;

/// What a reindex did, for the operator's summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReindexReport {
    /// The pin as it was, if the store had one.
    pub from: Option<(String, usize)>,
    /// The pin as it now is.
    pub to: (String, usize),
    /// Memories re-embedded.
    pub reindexed: usize,
}

pub struct SqliteReindexer {
    database: Arc<SqliteDatabase>,
    embedder: Arc<dyn Embedder>,
}

impl SqliteReindexer {
    pub fn new(database: Arc<SqliteDatabase>, embedder: Arc<dyn Embedder>) -> Self {
        Self { database, embedder }
    }

    pub fn execute(&self) -> Result<ReindexReport> {
        let model = self.embedder.model_id().to_string();
        let dimensions = self.embedder.dimensions();

        self.database.with_connection(|connection| {
            // The pin as it stands, read before we change it, so the
            // summary can say what it changed *from*. Collections all
            // share the configured model, so the first row is
            // representative.
            let from: Option<(String, i64)> = connection
                .query_row(
                    "SELECT embedding_model, dimensions FROM collections LIMIT 1",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .ok();

            // Read all content that has a vector: everything not
            // hard-deleted (a soft delete already removed the vector).
            // Superseded memories are included — they keep their vectors
            // so `include_superseded` recall can still find them.
            let rows: Vec<(String, String, String)> = {
                let mut statement = connection
                    .prepare("SELECT id, user_id, content FROM memories WHERE deleted_at IS NULL")
                    .map_err(|e| map_sqlite_error(e, "reindex read conflict"))?;
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(|e| map_sqlite_error(e, "reindex read conflict"))?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| map_sqlite_error(e, "reindex read conflict"))?
            };

            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin the reindex"))?;

            // Rebuild the vector table at the new width. Dropping first is
            // what lets the dimensionality change — `CREATE … IF NOT
            // EXISTS` alone would keep the old table's fixed width.
            transaction
                .execute_batch(&format!("DROP TABLE IF EXISTS {VEC_TABLE};"))
                .map_err(|e| map_sqlite_error(e, "could not drop the old vector table"))?;
            create_vec_table(&transaction, dimensions)?;

            let mut reindexed = 0usize;
            for chunk in rows.chunks(BATCH) {
                let contents: Vec<String> = chunk
                    .iter()
                    .map(|(_, _, content)| content.clone())
                    .collect();

                // Document task: these are stored memories, embedded the
                // same way `DirectMemorySaver` embeds them, so the query
                // (embedded as a Query) matches on recall.
                let vectors = self.embedder.embed(&contents, EmbeddingTask::Document)?;
                if vectors.len() != chunk.len() {
                    return Err(RaError::Internal(format!(
                        "embedder returned {} vectors for {} memories",
                        vectors.len(),
                        chunk.len()
                    )));
                }

                for ((id, user_id, _), embedding) in chunk.iter().zip(vectors) {
                    transaction
                        .execute(
                            &format!(
                                "INSERT INTO {VEC_TABLE} (memory_id, user_id, embedding)
                                 VALUES (?1, ?2, ?3)"
                            ),
                            rusqlite::params![id, user_id, to_bytes(&embedding)],
                        )
                        .map_err(|e| map_sqlite_error(e, "reindex insert conflict"))?;
                    reindexed += 1;
                }
            }

            // Repin every collection to the new model. After this the
            // store opens cleanly under the new [embeddings] config.
            transaction
                .execute(
                    "UPDATE collections SET embedding_model = ?1, dimensions = ?2",
                    rusqlite::params![model, dimensions as i64],
                )
                .map_err(|e| map_sqlite_error(e, "reindex repin conflict"))?;

            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit the reindex"))?;

            Ok(ReindexReport {
                from: from.map(|(model, dims)| (model, dims as usize)),
                to: (model.clone(), dimensions),
                reindexed,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::domain::scope::Scope;
    use crate::identity::domain::user_context::UserContext;
    use crate::memories::application::fake_embedder::FakeEmbedder;
    use crate::memories::domain::embedder::EmbeddingTask;
    use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
    use crate::memories::domain::memory_repository::MemoryRepository;
    use crate::memories::domain::vector_index::VectorIndex;
    use crate::memories::infrastructure::sqlite_memory_repository::SqliteMemoryRepository;
    use crate::memories::infrastructure::sqlite_vector_index::SqliteVectorIndex;

    fn authenticate(identity: &crate::bootstrap::wiring::Identity, handle: &str) -> UserContext {
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

    fn memory(context: &UserContext, content: &str) -> Memory {
        Memory::create(
            context.user_id(),
            NewMemory {
                content: content.to_string(),
                category: crate::memories::domain::category::Category::PreferenceCoding,
                subcategory: None,
                tags: vec![],
                entities: vec![],
                confidence: 1.0,
                source: MemorySource::default(),
                expires_at: None,
            },
            chrono::Utc::now(),
        )
        .unwrap()
    }

    /// Seeds a store with `old_dims`-wide vectors under `old_model`,
    /// reindexes to `new_dims`, and returns everything needed to assert.
    #[test]
    fn a_model_switch_re_embeds_in_place_and_recall_then_works() {
        let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
        let identity =
            crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();
        let alex = authenticate(&identity, "alex");

        // Old store: model "old-model" at 64 dimensions.
        let old_repo = SqliteMemoryRepository::new(Arc::clone(&database), "old-model", 64);
        let old_vectors = SqliteVectorIndex::open(Arc::clone(&database), 64).unwrap();
        let old_embedder = FakeEmbedder::new(64);

        let memories: Vec<Memory> = ["User prefers pnpm", "Deploys on Hetzner", "Vegetarian"]
            .iter()
            .map(|content| {
                let m = memory(&alex, content);
                old_repo.insert(&alex, &m, "test").unwrap();
                old_vectors
                    .upsert(
                        &alex,
                        m.id(),
                        &old_embedder
                            .embed_one(content, EmbeddingTask::Document)
                            .unwrap(),
                    )
                    .unwrap();
                m
            })
            .collect();

        // Switch to a 32-dimensional model, and reindex.
        let new_embedder: Arc<dyn Embedder> = Arc::new(FakeEmbedder::new(32));
        let report = SqliteReindexer::new(Arc::clone(&database), Arc::clone(&new_embedder))
            .execute()
            .unwrap();

        assert_eq!(report.reindexed, 3);
        assert_eq!(report.from, Some(("old-model".to_string(), 64)));
        assert_eq!(report.to.1, 32);

        // The pin now matches the new model: a repository built for it
        // opens the collection instead of refusing it.
        let new_repo =
            SqliteMemoryRepository::new(Arc::clone(&database), new_embedder.model_id(), 32);
        let new_vectors = SqliteVectorIndex::open(Arc::clone(&database), 32).unwrap();

        // Every memory has a fresh 32-dim vector, and KNN finds them.
        let hits = new_vectors
            .search(
                &alex,
                &new_embedder
                    .embed_one("pnpm package manager", EmbeddingTask::Query)
                    .unwrap(),
                10,
            )
            .unwrap();
        assert_eq!(hits.len(), 3, "every memory should have been re-embedded");
        assert!(hits.contains(&memories[0].id()));

        // And a normal save through the repo now succeeds — proving the
        // pin no longer blocks the new model.
        let fresh = memory(&alex, "a new memory after reindex");
        new_repo
            .insert(&alex, &fresh, "test")
            .expect("the repin should let the new model write");
    }

    #[test]
    fn reindexing_an_empty_store_is_a_no_op() {
        let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
        SqliteVectorIndex::open(Arc::clone(&database), 64).unwrap();

        let report = SqliteReindexer::new(Arc::clone(&database), Arc::new(FakeEmbedder::new(32)))
            .execute()
            .unwrap();

        assert_eq!(report.reindexed, 0);
        assert_eq!(
            report.from, None,
            "an empty store had no pin to change from"
        );
        assert_eq!(report.to.1, 32);
    }

    #[test]
    fn soft_deleted_memories_are_not_re_embedded() {
        let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
        let identity =
            crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();
        let alex = authenticate(&identity, "alex");

        let repo = SqliteMemoryRepository::new(Arc::clone(&database), "old-model", 64);
        SqliteVectorIndex::open(Arc::clone(&database), 64).unwrap();

        let kept = memory(&alex, "kept");
        let gone = memory(&alex, "forgotten");
        repo.insert(&alex, &kept, "test").unwrap();
        repo.insert(&alex, &gone, "test").unwrap();
        repo.delete(&alex, gone.id(), "test", "no longer wanted")
            .unwrap();

        let report = SqliteReindexer::new(Arc::clone(&database), Arc::new(FakeEmbedder::new(32)))
            .execute()
            .unwrap();

        assert_eq!(
            report.reindexed, 1,
            "only the memory that still has a vector should be re-embedded"
        );
    }
}
