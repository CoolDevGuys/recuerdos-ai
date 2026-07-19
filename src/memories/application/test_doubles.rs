//! In-memory doubles for the memories use cases.
//!
//! Beyond speed, these exist to make *failure* testable. The real stores
//! fail on disk errors and poisoned indexes — conditions that are hard to
//! provoke and easy to get wrong. The doubles let a test say "the vector
//! write fails now" and assert what the use case does about it, which is
//! where the interesting behaviour lives.
//!
//! Each double mirrors the real adapter's user scoping, so an isolation
//! bug in a use case shows up here rather than only in the slower
//! integration suite.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::embedder::Embedder;
use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
use crate::memories::domain::memory_repository::{AuditEntry, AuditOperation, MemoryRepository};
use crate::memories::domain::recall_ranker::RecallRanker;
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::clock::{Clock, FixedClock};
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{MemoryId, UserId};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::memories::application::direct_memory_saver::DirectMemorySaver;
use crate::memories::application::fake_embedder::FakeEmbedder;
use crate::memories::application::memory_exporter::MemoryExporter;
use crate::memories::application::memory_finder::MemoryFinder;
use crate::memories::application::memory_forgetter::MemoryForgetter;
use crate::memories::application::memory_recaller::MemoryRecaller;
use crate::memories::application::memory_updater::MemoryUpdater;

pub const DIMENSIONS: usize = 64;

pub fn fixed_clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock::at(
        DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
    ))
}

pub fn now() -> DateTime<Utc> {
    fixed_clock().now()
}

pub fn new_memory(content: &str) -> NewMemory {
    NewMemory {
        content: content.to_string(),
        category: crate::memories::domain::category::Category::PreferenceCoding,
        tags: vec![],
        entities: vec![],
        confidence: 1.0,
        source: MemorySource::default(),
        expires_at: None,
    }
}

/// Everything a memories use case needs, wired over doubles.
pub struct Fixture {
    pub memories: Arc<InMemoryMemoryRepository>,
    pub vectors: Arc<InMemoryVectorIndex>,
    pub text: Arc<InMemoryTextIndex>,
    pub embedder: Arc<FallibleEmbedder>,
    pub alex: UserContext,
    pub sam: UserContext,
}

impl Fixture {
    pub fn new() -> Self {
        let database = Arc::new(crate::shared::sqlite::SqliteDatabase::open_in_memory().unwrap());
        let identity =
            crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();

        Self {
            memories: Arc::new(InMemoryMemoryRepository::default()),
            vectors: Arc::new(InMemoryVectorIndex::default()),
            text: Arc::new(InMemoryTextIndex::default()),
            embedder: Arc::new(FallibleEmbedder::default()),
            alex: authenticate(&identity, "alex"),
            sam: authenticate(&identity, "sam"),
        }
    }

    pub fn saver(&self) -> DirectMemorySaver {
        DirectMemorySaver::new(
            Arc::clone(&self.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&self.vectors) as Arc<dyn VectorIndex>,
            Arc::clone(&self.text) as Arc<dyn TextIndex>,
            Arc::clone(&self.embedder) as Arc<dyn Embedder>,
            fixed_clock(),
        )
    }

    pub fn recaller(&self) -> MemoryRecaller {
        MemoryRecaller::new(
            Arc::clone(&self.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&self.vectors) as Arc<dyn VectorIndex>,
            Arc::clone(&self.text) as Arc<dyn TextIndex>,
            Arc::clone(&self.embedder) as Arc<dyn Embedder>,
            RecallRanker::new(90),
            fixed_clock(),
        )
    }

    pub fn finder(&self) -> MemoryFinder {
        MemoryFinder::new(Arc::clone(&self.memories) as Arc<dyn MemoryRepository>)
    }

    pub fn updater(&self) -> MemoryUpdater {
        MemoryUpdater::new(
            Arc::clone(&self.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&self.vectors) as Arc<dyn VectorIndex>,
            Arc::clone(&self.text) as Arc<dyn TextIndex>,
            Arc::clone(&self.embedder) as Arc<dyn Embedder>,
            fixed_clock(),
        )
    }

    pub fn forgetter(&self) -> MemoryForgetter {
        MemoryForgetter::new(
            Arc::clone(&self.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&self.vectors) as Arc<dyn VectorIndex>,
            Arc::clone(&self.text) as Arc<dyn TextIndex>,
        )
    }

    pub fn exporter(&self) -> MemoryExporter {
        MemoryExporter::new(Arc::clone(&self.memories) as Arc<dyn MemoryRepository>)
    }

    /// Saves a memory through the real use case, returning it.
    pub fn save(&self, context: &UserContext, content: &str) -> Memory {
        self.saver()
            .execute(context, new_memory(content), "test")
            .expect("save should succeed")
    }
}

fn authenticate(identity: &crate::bootstrap::wiring::Identity, handle: &str) -> UserContext {
    identity.user_creator.execute(handle, None).unwrap();
    let issued = identity
        .api_key_issuer
        .execute(
            handle,
            vec![crate::identity::domain::scope::Scope::Admin],
            "test",
        )
        .unwrap();
    identity
        .key_authenticator
        .execute(&issued.token.render())
        .unwrap()
}

#[derive(Default)]
pub struct InMemoryMemoryRepository {
    memories: Mutex<Vec<Memory>>,
    deleted: Mutex<Vec<MemoryId>>,
    audit: Mutex<Vec<AuditEntry>>,
}

impl InMemoryMemoryRepository {
    fn is_deleted(&self, id: MemoryId) -> bool {
        self.deleted.lock().unwrap().contains(&id)
    }

    fn visible(&self, context: &UserContext, id: MemoryId) -> bool {
        !self.is_deleted(id)
            && self
                .memories
                .lock()
                .unwrap()
                .iter()
                .any(|m| m.id() == id && m.user_id() == context.user_id())
    }

    fn record(&self, context: &UserContext, id: MemoryId, operation: AuditOperation, actor: &str) {
        self.audit.lock().unwrap().push(AuditEntry {
            memory_id: id,
            operation,
            actor: actor.to_string(),
            detail: String::new(),
            at: now(),
        });
        let _ = context;
    }
}

impl MemoryRepository for InMemoryMemoryRepository {
    fn insert(&self, context: &UserContext, memory: &Memory, actor: &str) -> Result<()> {
        self.memories.lock().unwrap().push(memory.clone());
        self.record(context, memory.id(), AuditOperation::Add, actor);
        Ok(())
    }

    fn update(&self, context: &UserContext, memory: &Memory, actor: &str) -> Result<()> {
        if !self.visible(context, memory.id()) {
            return Err(RaError::NotFound(format!(
                "memory {} not found",
                memory.id()
            )));
        }

        let mut memories = self.memories.lock().unwrap();
        let slot = memories
            .iter_mut()
            .find(|m| m.id() == memory.id())
            .expect("visible implies present");
        *slot = memory.clone();
        drop(memories);

        let operation = if memory.is_superseded() {
            AuditOperation::Supersede
        } else {
            AuditOperation::Update
        };
        self.record(context, memory.id(), operation, actor);
        Ok(())
    }

    fn delete(&self, context: &UserContext, id: MemoryId, actor: &str) -> Result<()> {
        if !self.visible(context, id) {
            return Err(RaError::NotFound(format!("memory {id} not found")));
        }
        self.deleted.lock().unwrap().push(id);
        self.record(context, id, AuditOperation::Delete, actor);
        Ok(())
    }

    fn find(&self, context: &UserContext, id: MemoryId) -> Result<Option<Memory>> {
        Ok(self
            .memories
            .lock()
            .unwrap()
            .iter()
            .find(|m| m.id() == id && m.user_id() == context.user_id() && !self.is_deleted(m.id()))
            .cloned())
    }

    fn find_many(&self, context: &UserContext, ids: &[MemoryId]) -> Result<Vec<Memory>> {
        Ok(self
            .memories
            .lock()
            .unwrap()
            .iter()
            .filter(|m| {
                ids.contains(&m.id())
                    && m.user_id() == context.user_id()
                    && !self.is_deleted(m.id())
            })
            .cloned()
            .collect())
    }

    fn list(&self, context: &UserContext, include_inactive: bool) -> Result<Vec<Memory>> {
        let mut found: Vec<Memory> = self
            .memories
            .lock()
            .unwrap()
            .iter()
            .filter(|m| {
                m.user_id() == context.user_id()
                    && !self.is_deleted(m.id())
                    && (include_inactive || !m.is_superseded())
            })
            .cloned()
            .collect();
        found.sort_by_key(|m| std::cmp::Reverse(m.created_at()));
        Ok(found)
    }

    fn audit_trail(&self, context: &UserContext, limit: usize) -> Result<Vec<AuditEntry>> {
        let _ = context;
        let mut entries = self.audit.lock().unwrap().clone();
        entries.reverse();
        entries.truncate(limit);
        Ok(entries)
    }

    fn touch_accessed(
        &self,
        context: &UserContext,
        ids: &[MemoryId],
        at: DateTime<Utc>,
    ) -> Result<()> {
        let mut memories = self.memories.lock().unwrap();
        for memory in memories.iter_mut() {
            if ids.contains(&memory.id()) && memory.user_id() == context.user_id() {
                *memory = memory.clone().mark_accessed(at);
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryVectorIndex {
    vectors: Mutex<HashMap<(UserId, MemoryId), Vec<f32>>>,
    fail_next: AtomicBool,
}

impl InMemoryVectorIndex {
    pub fn fail_next_upsert(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    pub fn contains(&self, id: MemoryId) -> bool {
        self.vectors.lock().unwrap().keys().any(|(_, m)| *m == id)
    }

    pub fn is_empty(&self) -> bool {
        self.vectors.lock().unwrap().is_empty()
    }
}

impl VectorIndex for InMemoryVectorIndex {
    fn upsert(&self, context: &UserContext, id: MemoryId, embedding: &[f32]) -> Result<()> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(RaError::Internal("injected vector failure".to_string()));
        }
        self.vectors
            .lock()
            .unwrap()
            .insert((context.user_id(), id), embedding.to_vec());
        Ok(())
    }

    fn remove(&self, context: &UserContext, id: MemoryId) -> Result<()> {
        self.vectors
            .lock()
            .unwrap()
            .remove(&(context.user_id(), id));
        Ok(())
    }

    fn search(
        &self,
        context: &UserContext,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<MemoryId>> {
        let vectors = self.vectors.lock().unwrap();
        let mut scored: Vec<(MemoryId, f32)> = vectors
            .iter()
            .filter(|((user, _), _)| *user == context.user_id())
            .map(|((_, id), stored)| (*id, cosine(embedding, stored)))
            .collect();

        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
        });
        scored.truncate(limit);
        Ok(scored.into_iter().map(|(id, _)| id).collect())
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[derive(Default)]
pub struct InMemoryTextIndex {
    documents: Mutex<HashMap<(UserId, MemoryId), String>>,
    fail_next: AtomicBool,
}

impl InMemoryTextIndex {
    pub fn fail_next_upsert(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    pub fn contains(&self, id: MemoryId) -> bool {
        self.documents.lock().unwrap().keys().any(|(_, m)| *m == id)
    }
}

impl TextIndex for InMemoryTextIndex {
    fn upsert(&self, context: &UserContext, memory: &Memory) -> Result<()> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(RaError::Internal("injected text index failure".to_string()));
        }
        let haystack = format!(
            "{} {} {}",
            memory.content(),
            memory.tags().join(" "),
            memory.category().as_str()
        )
        .to_ascii_lowercase();
        self.documents
            .lock()
            .unwrap()
            .insert((context.user_id(), memory.id()), haystack);
        Ok(())
    }

    fn remove(&self, context: &UserContext, id: MemoryId) -> Result<()> {
        self.documents
            .lock()
            .unwrap()
            .remove(&(context.user_id(), id));
        Ok(())
    }

    /// Ranks by how many query words a document contains — a crude BM25
    /// stand-in that is enough to give the ranker two differing opinions
    /// to fuse.
    fn search(&self, context: &UserContext, query: &str, limit: usize) -> Result<Vec<MemoryId>> {
        let query = query.to_ascii_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();

        let documents = self.documents.lock().unwrap();
        let mut scored: Vec<(MemoryId, usize)> = documents
            .iter()
            .filter(|((user, _), _)| *user == context.user_id())
            .map(|((_, id), haystack)| {
                (
                    *id,
                    words
                        .iter()
                        .filter(|word| haystack.contains(**word))
                        .count(),
                )
            })
            .filter(|(_, hits)| *hits > 0)
            .collect();

        scored.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
        });
        scored.truncate(limit);
        Ok(scored.into_iter().map(|(id, _)| id).collect())
    }
}

/// The fake embedder plus an injectable failure.
pub struct FallibleEmbedder {
    inner: FakeEmbedder,
    fail_next: AtomicBool,
}

impl Default for FallibleEmbedder {
    fn default() -> Self {
        Self {
            inner: FakeEmbedder::new(DIMENSIONS),
            fail_next: AtomicBool::new(false),
        }
    }
}

impl FallibleEmbedder {
    pub fn fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
}

impl Embedder for FallibleEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(RaError::Internal("injected embedding failure".to_string()));
        }
        self.inner.embed(texts)
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}
