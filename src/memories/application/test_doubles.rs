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
use crate::identity::domain::user_repository::UserRepository;
use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
use crate::memories::domain::memory_repository::{AuditEntry, AuditOperation, MemoryRepository};
use crate::memories::domain::recall_ranker::RecallRanker;
use crate::memories::domain::reranker::{RerankCandidate, RerankMode, Reranker};
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::clock::{Clock, FixedClock};
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{MemoryId, UserId};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
        subcategory: None,
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
    /// The real (SQLite-backed, in-memory) user table the two contexts
    /// above were authenticated against. Consolidation is the one thing
    /// that walks every user rather than being handed one, so it needs
    /// the same table those contexts came from.
    pub users: Arc<dyn UserRepository>,
    /// The same in-memory database the identity table lives in, carrying
    /// the V8 graph tables. The memories repository is a double, but the
    /// entity graph is exercised for real over this handle so the graph
    /// leg's SQL (the recall CTE) is tested, not stubbed.
    pub database: Arc<crate::shared::sqlite::SqliteDatabase>,
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
            users: Arc::clone(&identity.users),
            database,
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
            None,
            2,
            50,
        )
    }

    /// A real SQLite-backed entity graph over the fixture's database.
    pub fn graph(
        &self,
    ) -> Arc<crate::memories::infrastructure::sqlite_entity_graph::SqliteEntityGraph> {
        Arc::new(
            crate::memories::infrastructure::sqlite_entity_graph::SqliteEntityGraph::new(
                Arc::clone(&self.database),
            ),
        )
    }

    /// A recaller whose third leg hops over `graph`.
    pub fn recaller_with_graph(
        &self,
        graph: Arc<dyn crate::memories::domain::entity_graph::EntityGraph>,
    ) -> MemoryRecaller {
        MemoryRecaller::new(
            Arc::clone(&self.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&self.vectors) as Arc<dyn VectorIndex>,
            Arc::clone(&self.text) as Arc<dyn TextIndex>,
            Arc::clone(&self.embedder) as Arc<dyn Embedder>,
            RecallRanker::new(90),
            fixed_clock(),
            Some(graph),
            2,
            50,
        )
    }

    /// A recaller whose second stage is `reranker`, run in `mode` over
    /// `depth` candidates and abandoned past `timeout`
    /// (reranking-plan.md Phase 4).
    pub fn recaller_with_reranker(
        &self,
        reranker: Arc<dyn Reranker>,
        mode: RerankMode,
        depth: usize,
        timeout: std::time::Duration,
    ) -> MemoryRecaller {
        self.recaller()
            .with_reranker(reranker, mode, depth, timeout)
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

    fn record(
        &self,
        context: &UserContext,
        id: MemoryId,
        operation: AuditOperation,
        actor: &str,
        detail: &str,
    ) {
        self.audit.lock().unwrap().push(AuditEntry {
            memory_id: id,
            operation,
            actor: actor.to_string(),
            detail: detail.to_string(),
            at: now(),
        });
        let _ = context;
    }
}

impl MemoryRepository for InMemoryMemoryRepository {
    fn insert(&self, context: &UserContext, memory: &Memory, actor: &str) -> Result<()> {
        self.memories.lock().unwrap().push(memory.clone());
        self.record(context, memory.id(), AuditOperation::Add, actor, "");
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
        self.record(context, memory.id(), operation, actor, "");
        Ok(())
    }

    fn delete(&self, context: &UserContext, id: MemoryId, actor: &str, reason: &str) -> Result<()> {
        if !self.visible(context, id) {
            return Err(RaError::NotFound(format!("memory {id} not found")));
        }
        self.deleted.lock().unwrap().push(id);
        self.record(context, id, AuditOperation::Delete, actor, reason);
        Ok(())
    }

    fn supersede(
        &self,
        context: &UserContext,
        superseded: MemoryId,
        replacement: MemoryId,
        actor: &str,
        reason: &str,
    ) -> Result<()> {
        if !self.visible(context, superseded) {
            return Err(RaError::NotFound(format!("memory {superseded} not found")));
        }

        let mut memories = self.memories.lock().unwrap();
        let memory = memories
            .iter_mut()
            .find(|m| m.id() == superseded)
            .expect("visible implies present");
        *memory = memory.clone().supersede(replacement, Utc::now());
        drop(memories);

        self.record(
            context,
            superseded,
            AuditOperation::Supersede,
            actor,
            reason,
        );
        Ok(())
    }

    fn merge(
        &self,
        context: &UserContext,
        superseded: &[MemoryId],
        replacement: MemoryId,
        actor: &str,
        reason: &str,
    ) -> Result<usize> {
        let mut retired = 0usize;

        for id in superseded {
            // Mirrors the real adapter's skip conditions rather than its
            // atomicity: a double that returned errors where SQLite skips
            // would let a use case pass here and fail in production.
            if !self.visible(context, *id) {
                continue;
            }

            let mut memories = self.memories.lock().unwrap();
            let memory = memories
                .iter_mut()
                .find(|m| m.id() == *id)
                .expect("visible implies present");
            if memory.is_superseded() {
                continue;
            }
            *memory = memory.clone().supersede(replacement, Utc::now());
            drop(memories);

            self.record(context, *id, AuditOperation::Merge, actor, reason);
            retired += 1;
        }

        Ok(retired)
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

    fn set_importance(&self, context: &UserContext, scores: &[(MemoryId, f32)]) -> Result<()> {
        let mut memories = self.memories.lock().unwrap();
        for memory in memories.iter_mut() {
            if memory.user_id() != context.user_id() {
                continue;
            }
            if let Some((_, importance)) = scores.iter().find(|(id, _)| *id == memory.id()) {
                *memory = memory.clone().with_importance(*importance);
            }
        }
        Ok(())
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
    ) -> Result<Vec<(MemoryId, f32)>> {
        let vectors = self.vectors.lock().unwrap();
        let mut scored: Vec<(MemoryId, f32)> = vectors
            .iter()
            .filter(|((user, _), _)| *user == context.user_id())
            .map(|((_, id), stored)| (*id, cosine(embedding, stored).clamp(0.0, 1.0)))
            .collect();

        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
        });
        scored.truncate(limit);
        Ok(scored)
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
    ///
    /// The weight reported per matched word stands in for a real BM25
    /// contribution rather than being the count itself, so that relevance
    /// computed against this double saturates anywhere near where the real
    /// index's does. A bare count would read as "a two-word match is 0.2
    /// relevant" and every floor test would be testing the wrong number.
    fn search(
        &self,
        context: &UserContext,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        const NOMINAL_BM25_PER_WORD: f32 = 5.0;

        let query = query.to_ascii_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();

        let documents = self.documents.lock().unwrap();
        let mut scored: Vec<(MemoryId, f32)> = documents
            .iter()
            .filter(|((user, _), _)| *user == context.user_id())
            .map(|((_, id), haystack)| {
                (
                    *id,
                    words
                        .iter()
                        .filter(|word| haystack.contains(**word))
                        .count() as f32
                        * NOMINAL_BM25_PER_WORD,
                )
            })
            .filter(|(_, score)| *score > 0.0)
            .collect();

        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
        });
        scored.truncate(limit);
        Ok(scored)
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
    fn embed(&self, texts: &[String], task: EmbeddingTask) -> Result<Vec<Vec<f32>>> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(RaError::Internal("injected embedding failure".to_string()));
        }
        self.inner.embed(texts, task)
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

/// Deterministic stand-ins for a second-stage reranker (reranking-plan.md
/// Phase 4.2). Like the other doubles these exist to make *failure* and
/// *timing* testable, and they record what they were asked to score so a
/// test can prove the filters ran first: the leg must never see a memory
/// the request had already excluded.
#[derive(Default)]
pub struct FakeReranker {
    /// `false` (the default) scores by query/token overlap; `true` scores
    /// by descending position, which reverses the incoming order — the
    /// `reversing` fake's whole trick.
    reverse: bool,
    calls: AtomicUsize,
    /// The candidate ids of every call, in call order.
    seen: Mutex<Vec<Vec<MemoryId>>>,
    /// Wall time spent inside every `score` call. With a short
    /// `[rerank].timeout_ms` this is how a test makes the stage miss its
    /// deadline — the condition that used to be invisible in a response.
    delay: std::time::Duration,
    /// Fails every call, standing in for a model that cannot answer at all
    /// (or answers with scores that do not line up with its candidates).
    failing: bool,
}

impl FakeReranker {
    /// Scores each candidate by how many of the query's word tokens it
    /// contains — the cross-encoder's crude cousin, enough for a test to
    /// say "the memory that shares the query's words should move up".
    pub fn by_token_overlap() -> Self {
        Self::default()
    }

    /// Strictly *reverses* the candidate order it is given. Deterministic
    /// and obvious to assert against: `mode = On` must produce the exact
    /// reverse of the fused head, and `Shadow` must not.
    pub fn reversing() -> Self {
        Self {
            reverse: true,
            ..Default::default()
        }
    }

    /// Like [`Self::by_token_overlap`], but takes `delay` on every call —
    /// long enough past a test's `timeout_ms` to prove a stage that misses
    /// its deadline is *reported*, not merely absent from the ordering.
    pub fn sleeping(delay: std::time::Duration) -> Self {
        Self {
            delay,
            ..Default::default()
        }
    }

    /// Errors on every call: the model is configured and asked, and refuses.
    pub fn failing() -> Self {
        Self {
            failing: true,
            ..Default::default()
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn seen(&self) -> Vec<Vec<MemoryId>> {
        self.seen.lock().unwrap().clone()
    }
}

impl Reranker for FakeReranker {
    fn score(&self, query: &str, candidates: &[RerankCandidate<'_>]) -> Result<Vec<f32>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen
            .lock()
            .unwrap()
            .push(candidates.iter().map(|c| c.id).collect());

        if self.failing {
            return Err(crate::shared::error::RaError::Internal(
                "fake reranker is failing".to_string(),
            ));
        }
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }

        if self.reverse {
            let n = candidates.len();
            // Ascending with position: once the caller orders the leg by
            // score (descending), the candidates come out reversed.
            return Ok((1..=n).map(|i| i as f32).collect());
        }

        let tokens: Vec<String> = query
            .to_ascii_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_string())
            .collect();
        Ok(candidates
            .iter()
            .map(|candidate| {
                candidate_text_tokens(candidate.text)
                    .into_iter()
                    .filter(|word| tokens.contains(word))
                    .count() as f32
            })
            .collect())
    }
}

fn candidate_text_tokens(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect()
}

/// Always fails — the "the model blew up" path of a best-effort leg.
pub struct FailingReranker;

impl Reranker for FailingReranker {
    fn score(&self, _query: &str, _candidates: &[RerankCandidate<'_>]) -> Result<Vec<f32>> {
        Err(RaError::Internal("injected rerank failure".to_string()))
    }
}

/// Sleeps, then scores everything zero: the deadline path. Pair it with a
/// `timeout` below the sleep to prove the leg expires, above it to prove
/// the leg also *succeeds* over a thread boundary.
pub struct SleepingReranker(Duration);

impl SleepingReranker {
    pub fn new(sleep: Duration) -> Self {
        Self(sleep)
    }
}

impl Reranker for SleepingReranker {
    fn score(&self, _query: &str, candidates: &[RerankCandidate<'_>]) -> Result<Vec<f32>> {
        std::thread::sleep(self.0);
        Ok(vec![0.0; candidates.len()])
    }
}
