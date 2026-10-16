//! Hybrid recall: the read path the whole service exists to serve.
//!
//! Embeds the query, asks both indexes in parallel, fuses their rankings
//! and applies the caller's filters.
//!
//! # Over-fetch, then filter
//!
//! Filters (category, tags, since) are applied *after* the indexes have
//! answered, because neither index knows about them. Each leg is
//! therefore asked for several times `limit` candidates
//! ([`RecallQuery::candidate_depth`]).
//!
//! The honest limitation: a highly selective filter over a large corpus
//! can still return fewer than `limit` results even when more exist,
//! because the matching memories never made the candidate window. Pushing
//! filters into both indexes is the fix, and is worth doing when filtered
//! recall becomes a common path rather than an occasional one.
//!
//! # Reading without leaving a mark
//!
//! A real read is a fact about the user's life: the memories it returns
//! are stamped as accessed, and those counters are the whole input to
//! importance decay. So [`MemoryRecaller::execute`] writes, deliberately.
//!
//! Measuring a store is not. `rerank audit` and the eval harness replay
//! someone's queries over their *real* memories to compare orderings, and
//! a replay that bumped the access counters would change the store it came
//! to observe — and change the answer on the second run, because decay
//! feeds the ranking it is ranking. [`MemoryRecaller::execute_untracked`]
//! is that read: identical results, identical order, no bookkeeping
//! (reranking-plan.md Phase 6.1, invariant I-6).

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::memories::domain::entity_graph::EntityGraph;
use crate::memories::domain::entity_key::EntityKey;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::recall_query::RecallQuery;
use crate::memories::domain::recall_ranker::{
    RankedIds, RecallRanker, RerankStrategy, ScoredMemory,
};
use crate::memories::domain::reranker::{RerankCandidate, RerankMode, Reranker};
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::clock::Clock;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// The longest entity name, in words, the query scanner will try to match.
/// Entity names are short ("billing service", "Meridian team"); a longer
/// window only manufactures n-grams that match nothing.
const MAX_SEED_WORDS: usize = 3;

/// The rerank head size assumed when no `[rerank]` config overrides it —
/// see [`MemoryRecaller::with_reranker`]. Not a tuned number (Phase 7
/// tunes against the eval grid); the plan's placeholder depth.
const DEFAULT_RERANK_DEPTH: usize = 30;

/// The fallback deadline, matching the planned `[rerank].timeout_ms`.
const DEFAULT_RERANK_TIMEOUT: Duration = Duration::from_millis(400);

/// Whether a recall leaves the access bookkeeping behind (see the
/// [module docs](self)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecuteOptions {
    /// Stamp `last_accessed_at` and bump `access_count` on the memories
    /// that were returned.
    ///
    /// `true` is what every client-facing read does: those two numbers are
    /// the entire input to importance decay, so a read that did not record
    /// itself would quietly age a memory the user clearly still uses.
    /// `false` is for replaying queries over a store in order to *measure*
    /// it — an audit or an eval — where recording the replay would change
    /// the thing being measured (reranking-plan.md Phase 6.1).
    pub track_access: bool,
}

/// What the optional second stage actually did for one recall.
///
/// It exists because `mode = "on"` and `mode = "off"` were indistinguishable
/// from the client's side. The deadline path serves the pre-rerank order and
/// warns in the daemon's log, so an operator who paid for a second stage sees
/// an ordinary response whether or not it ran. Not hypothetical: a host also
/// serving two local models paid ~34 ms per candidate pair, so its depth-10
/// head crossed the 250 ms deadline on every real query — `mode = "on"`
/// behaving exactly like `off`. `rerank audit` could not rescue it either: it
/// measures with no deadline, so it reported a working stage while production
/// discarded the same answer at 250 ms.
///
/// Reported only when a stage is configured. `off` stays invisible, which is
/// what "off is a hard no-op" has always promised the response to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RerankTrace {
    /// Configured, and there was nothing to reorder: fewer than two
    /// candidates survived `[retrieval].min_relevance`, or the depth clamped
    /// below two. Not a failure — the stage has no work at a head of one.
    NotNeeded { candidates: usize },
    /// Judged the head and folded its order into the ranking.
    Applied { judged: usize, took_ms: u64 },
    /// Judged the head and logged the comparison, serving the old order —
    /// which is what `mode = "shadow"` exists to do.
    Shadow {
        judged: usize,
        /// Whether the reranker would have moved the top result. The same
        /// `changed` the `rerank_shadow` log line carries.
        changed: bool,
        took_ms: u64,
    },
    /// Asked, and it did not contribute. The pre-rerank order was served,
    /// which is the point — a recall never fails because of the second
    /// stage — but "the stage is not happening" is a fact the operator
    /// needs from somewhere other than a log line they have to think to
    /// grep.
    Skipped {
        reason: RerankSkip,
        /// How many candidates it was about to judge when it gave up.
        candidates: usize,
        took_ms: u64,
    },
}

/// Why a configured stage declined to contribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RerankSkip {
    /// Past `[rerank].timeout_ms`. On a machine that is not idle this is the
    /// ordinary outcome for a deep head, not a rare one: the default is sized
    /// to a quiet benchmark box, and inference time scales with the number of
    /// pairs judged.
    Deadline,
    /// The model failed, or answered with a number of scores that did not
    /// line up with the candidates it was given.
    Model,
}

/// A recall's results, plus what the optional second stage did with them.
#[derive(Debug)]
pub struct RecallReport {
    pub results: Vec<ScoredMemory>,
    /// `None` when nothing is wired or `mode = "off"`: nothing ran, and
    /// nothing is reported.
    pub rerank: Option<RerankTrace>,
}

impl Default for ExecuteOptions {
    /// Real requests, tracked. The untracked path is opt-in, never the
    /// default: forgetting to pass an option must not silently stop decay
    /// from seeing real reads.
    fn default() -> Self {
        Self { track_access: true }
    }
}

/// The recall pipeline: indexes in, fused (and optionally reranked)
/// memories out.
///
/// `Clone` is cheap and deliberate — every field is an `Arc` or a copyable
/// value. It exists for `rerank audit` (reranking-plan.md Phase 6.2), which
/// compares the pipeline *with* a second stage against the pipeline without
/// one, over one open store. Duplicating the wiring instead would let the
/// two sides of that comparison drift apart, which is the whole comparison.
#[derive(Clone)]
pub struct MemoryRecaller {
    memories: Arc<dyn MemoryRepository>,
    vectors: Arc<dyn VectorIndex>,
    text: Arc<dyn TextIndex>,
    embedder: Arc<dyn Embedder>,
    ranker: RecallRanker,
    clock: Arc<dyn Clock>,
    /// The entity/relation graph, present only when `[graph].enabled`
    /// (Task 7.3.4). `None` is the default and the pre-graph behaviour:
    /// no third leg runs, and recall is byte-identical to a two-leg build.
    graph: Option<Arc<dyn EntityGraph>>,
    /// How many edges one hop may traverse (`[graph].max_hops`) and how
    /// many memories the leg may return (`[graph].hop_limit`). Unused while
    /// `graph` is `None`.
    max_hops: usize,
    hop_limit: usize,
    /// The second-stage ranker, present only when `[rerank].mode != "off"`
    /// (reranking-plan.md Phase 4). `None` is the default and the pre-rerank
    /// behaviour: no second stage runs, and recall is byte-identical to a
    /// build without it — the same "no X, no change" pattern as `graph`.
    reranker: Option<Arc<dyn Reranker>>,
    /// What to do with a configured reranker: `Off` (the default) never
    /// consults it; `Shadow` scores and logs but serves the pre-rerank
    /// order; `On` serves the reranked order.
    rerank_mode: RerankMode,
    /// How many fused candidates the reranker judges per recall
    /// (`[rerank].depth`, clamped to the candidate pool). The defaults on
    /// this trio mirror the planned `[rerank]` block so Phase 5 wires
    /// values through without changing this struct; all are inert while
    /// `reranker` is `None`.
    rerank_depth: usize,
    /// Beyond this, the rerank leg is abandoned and the pre-rerank order
    /// served (`[rerank].timeout_ms`). `Duration::ZERO` means "no
    /// deadline"; a non-zero one runs the scoring on a detached thread so
    /// expiry does not have to wait for it (Phase 5 bounds the thread
    /// count, and the inference itself, with a semaphore).
    rerank_timeout: Duration,
}

impl MemoryRecaller {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        memories: Arc<dyn MemoryRepository>,
        vectors: Arc<dyn VectorIndex>,
        text: Arc<dyn TextIndex>,
        embedder: Arc<dyn Embedder>,
        ranker: RecallRanker,
        clock: Arc<dyn Clock>,
        graph: Option<Arc<dyn EntityGraph>>,
        max_hops: usize,
        hop_limit: usize,
    ) -> Self {
        Self {
            memories,
            vectors,
            text,
            embedder,
            ranker,
            clock,
            graph,
            max_hops,
            hop_limit,
            reranker: None,
            rerank_mode: RerankMode::default(),
            rerank_depth: DEFAULT_RERANK_DEPTH,
            rerank_timeout: DEFAULT_RERANK_TIMEOUT,
        }
    }

    /// The same pipeline with its second stage reconfigured
    /// (reranking-plan.md Phase 6.2). Every component is shared — indexes,
    /// repository, ranker, clock — so the two handles differ *only* in the
    /// thing being audited. `rerank audit` builds the pair: this method with
    /// `None`/`Off` is the pipeline a client gets today, and with the loaded
    /// reranker in `On` it is the pipeline the operator is judging.
    pub fn with_second_stage(
        &self,
        reranker: Option<Arc<dyn Reranker>>,
        mode: RerankMode,
        depth: usize,
        timeout: Duration,
    ) -> Self {
        let mut pipeline = self.clone();
        pipeline.reranker = reranker;
        pipeline.rerank_mode = mode;
        pipeline.rerank_depth = depth;
        pipeline.rerank_timeout = timeout;
        pipeline
    }

    /// The same pipeline with the rerank leg fused a different way
    /// (reranking-plan.md Phase 7.2). Wiring sets the strategy from
    /// `[rerank].strategy` at assembly time; this exists so the tuning
    /// grid can measure both strategies from *one* seeded store, cells
    /// differing only in what the grid says to vary. Like every
    /// `with_*` here it shares the indexes and never writes.
    pub fn with_rerank_strategy(mut self, strategy: RerankStrategy) -> Self {
        self.ranker = self.ranker.with_rerank_strategy(strategy);
        self
    }

    /// Attaches a second-stage reranker (reranking-plan.md Phase 4). Not
    /// calling it — or calling it with `RerankMode::Off` — is the identity
    /// guarantee: recall takes its pre-rerank path, and no model runs.
    /// Wiring passes this only when `[rerank].mode != "off"` (Phase 5).
    pub fn with_reranker(
        mut self,
        reranker: Arc<dyn Reranker>,
        mode: RerankMode,
        depth: usize,
        timeout: Duration,
    ) -> Self {
        self.reranker = Some(reranker);
        self.rerank_mode = mode;
        self.rerank_depth = depth;
        self.rerank_timeout = timeout;
        self
    }

    /// Runs one recall and records that the returned memories were
    /// accessed. This is what a client-facing read does, and the behaviour
    /// every path had before [`ExecuteOptions`] existed.
    pub fn execute(&self, context: &UserContext, query: &RecallQuery) -> Result<Vec<ScoredMemory>> {
        Ok(self
            .execute_with(context, query, ExecuteOptions::default())?
            .results)
    }

    /// Runs one recall that changes nothing: same candidates, same fusion,
    /// same rerank leg, same order — and no `touch_accessed`.
    ///
    /// The point is invariant I-6 of reranking-plan.md: an operator or the
    /// eval harness replaying queries over real memories is measuring, and
    /// a measurement that bumps access counts feeds importance decay, which
    /// feeds the ranking the next measurement reads. Every audit and eval
    /// replay goes through here.
    pub fn execute_untracked(
        &self,
        context: &UserContext,
        query: &RecallQuery,
    ) -> Result<Vec<ScoredMemory>> {
        Ok(self.execute_untracked_traced(context, query)?.results)
    }

    /// [`Self::execute`], plus what the optional second stage did.
    ///
    /// For the surfaces that must not present a dropped second stage as a
    /// working one: the REST response, and anything else that reports recall
    /// quality back to an operator.
    pub fn execute_traced(
        &self,
        context: &UserContext,
        query: &RecallQuery,
    ) -> Result<RecallReport> {
        self.execute_with(context, query, ExecuteOptions::default())
    }

    /// [`Self::execute_untracked`], plus the [`RerankTrace`]. This is what
    /// `rerank audit` runs, so the audit can say when it measured nothing
    /// because the stage never finished.
    pub fn execute_untracked_traced(
        &self,
        context: &UserContext,
        query: &RecallQuery,
    ) -> Result<RecallReport> {
        self.execute_with(
            context,
            query,
            ExecuteOptions {
                track_access: false,
            },
        )
    }

    fn execute_with(
        &self,
        context: &UserContext,
        query: &RecallQuery,
        options: ExecuteOptions,
    ) -> Result<RecallReport> {
        let depth = query.candidate_depth();
        let now = self.clock.now();

        let embedding = self
            .embedder
            .embed_one(query.text(), EmbeddingTask::Query)?;
        let vector_scores = self.vectors.search(context, &embedding, depth)?;
        let vector_hits = ranked(&vector_scores);
        let vector_scores = scores(&vector_scores);

        // A keyword failure degrades the result rather than failing the
        // request: half a hybrid search still answers the question, and
        // the caller would rather have that than an error.
        let (keyword_hits, keyword_scores) = match self.text.search(context, query.text(), depth) {
            Ok(hits) => (ranked(&hits), scores(&hits)),
            Err(error) => {
                tracing::warn!(%error, "keyword search failed; falling back to vectors only");
                (RankedIds::default(), HashMap::new())
            }
        };

        // The third leg: memories connected to the query's entities over
        // the graph. Absent (no graph) or silent (query names nothing
        // known) it contributes nothing, and degrades exactly like the
        // keyword leg — two thirds of a hybrid search still answers.
        let graph_hits = self.graph_leg(context, query);

        let mut candidate_ids: Vec<_> = vector_hits.0.clone();
        for id in keyword_hits.0.iter().chain(graph_hits.0.iter()) {
            if !candidate_ids.contains(id) {
                candidate_ids.push(*id);
            }
        }
        if candidate_ids.is_empty() {
            // Nothing to judge. When a stage is configured that is still worth
            // saying (`not-needed`, zero candidates) — a store the query found
            // nothing in is a different fact from a stage that was never
            // wired, and `None` means the latter.
            return Ok(RecallReport {
                results: Vec::new(),
                rerank: self
                    .stage_configured()
                    .then_some(RerankTrace::NotNeeded { candidates: 0 }),
            });
        }

        let candidates: Vec<Memory> = self
            .memories
            .find_many(context, &candidate_ids)?
            .into_iter()
            .filter(|memory| matches(memory, query, now))
            .collect();

        // A build with no graph goes through the original two-leg entry
        // point verbatim — not `rank_with_graph` over an empty leg — so the
        // "no graph, no change" guarantee is the same code path, not merely
        // the same arithmetic.
        let mut ranked = if self.graph.is_some() {
            self.ranker
                .rank_with_graph(&vector_hits, &keyword_hits, &graph_hits, candidates, now)
        } else {
            self.ranker
                .rank(&vector_hits, &keyword_hits, candidates, now)
        };

        // Relevance is attached after fusion because it is a different
        // question from the one fusion answers. Fusion ranks; this measures
        // how good a match each result actually is, which is what lets a
        // caller — or the floor below — say "nothing here answers you"
        // instead of always serving back the top of a list of noise.
        for scored in &mut ranked {
            scored.relevance = self.ranker.relevance(
                vector_scores.get(&scored.memory.id()).copied(),
                keyword_scores
                    .get(&scored.memory.id())
                    .copied()
                    .unwrap_or_default(),
                scored.match_detail.graph_rank.is_some()
                    && scored.match_detail.vector_rank.is_none()
                    && scored.match_detail.bm25_rank.is_none(),
            );
        }

        // Before the truncate, so a caller asking for five results gets
        // five relevant ones rather than three once the floor has eaten two
        // of the five it was truncated to.
        let floor = query.min_relevance();
        if floor > 0.0 {
            ranked.retain(|scored| scored.relevance >= floor);
        }

        // The second stage (reranking-plan.md Phase 4.4): between filtering
        // and truncation, so it never sees a memory the filters or the
        // floor had already rejected, and its reordering is judged by the
        // client-visible truncation rather than padded back in. `reranker:
        // None` and `mode = Off` return `ranked` untouched — the identity
        // path.
        let (mut ranked, rerank) = self.rerank_leg(query, ranked, now);
        ranked.truncate(query.limit());

        // Feeds Phase 5's importance decay. Best-effort: a bookkeeping
        // failure must not fail the read the caller actually made — and
        // skipped entirely for an untracked read, which exists precisely
        // so that replaying a query cannot move the store it measures.
        if options.track_access {
            let returned: Vec<_> = ranked.iter().map(|scored| scored.memory.id()).collect();
            if let Err(error) = self.memories.touch_accessed(context, &returned, now) {
                tracing::warn!(%error, "failed to record memory access");
            }
        }

        Ok(RecallReport {
            results: ranked,
            rerank,
        })
    }

    /// Runs the graph hop, or nothing when there is no graph. Best-effort:
    /// a hop failure warns and yields an empty leg, so the request still
    /// returns the vector and keyword results.
    fn graph_leg(&self, context: &UserContext, query: &RecallQuery) -> RankedIds {
        let Some(graph) = &self.graph else {
            return RankedIds::default();
        };
        match self.hop(graph.as_ref(), context, query) {
            Ok(ids) => RankedIds(ids),
            Err(error) => {
                tracing::warn!(%error, "graph hop failed; falling back to the other legs");
                RankedIds::default()
            }
        }
    }

    fn hop(
        &self,
        graph: &dyn EntityGraph,
        context: &UserContext,
        query: &RecallQuery,
    ) -> Result<Vec<MemoryId>> {
        // Scan the query for entities the graph knows — no model call —
        // then hop from them. No seeds means the query named nothing
        // known, so the leg stays silent and recall is its two-leg self.
        let seeds = graph.seeds(context, &seed_candidates(query.text()))?;
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        graph.neighbours(
            context,
            &seeds,
            self.max_hops,
            query.as_of(),
            self.hop_limit,
        )
    }

    /// Runs the rerank leg over the fused head, or nothing. Mirrors
    /// [`graph_leg`](Self::graph_leg): a failure or an expired deadline
    /// warns and leaves `ranked` as fusion served it — three-quarters of a
    /// hybrid search still answers. Unlike the graph leg it also reports
    /// *what happened*, because a stage that quietly never runs is
    /// indistinguishable from one that was never turned on: see
    /// [`RerankTrace`]. `Off` (and `reranker: None`) returns `None` — the
    /// pre-existing path pays nothing and says nothing.
    /// A skipped or shadowed leg means the client sees the pre-rerank order,
    /// down to the absent `rerank_score` fields (Phase 6) — which is exactly
    /// why the trace exists.
    fn rerank_leg(
        &self,
        query: &RecallQuery,
        mut ranked: Vec<ScoredMemory>,
        now: DateTime<Utc>,
    ) -> (Vec<ScoredMemory>, Option<RerankTrace>) {
        // Nothing wired, or `Off`: no stage exists for this recall, so there
        // is nothing to report and the response stays the pre-rerank build's
        // response, field for field.
        let Some(reranker) = &self.reranker else {
            return (ranked, None);
        };
        if self.rerank_mode == RerankMode::Off {
            return (ranked, None);
        }
        if ranked.len() < 2 {
            // Reranking one candidate cannot reorder anything. Reported as
            // "not needed" rather than silence, because a head of one is also
            // what a mis-set `[retrieval].min_relevance` produces — and that
            // looks identical to the stage not running unless it says which.
            let candidates = ranked.len();
            return (ranked, Some(RerankTrace::NotNeeded { candidates }));
        }

        // Clamped both ways: never more than the caller's own candidate
        // window justified, and never more than fusion actually produced.
        let depth = self
            .rerank_depth
            .min(ranked.len())
            .min(query.candidate_depth());
        if depth < 2 {
            let candidates = ranked.len();
            return (ranked, Some(RerankTrace::NotNeeded { candidates }));
        }

        let started = std::time::Instant::now();
        let leg = match self.rerank_scores(reranker, query.text(), &ranked[..depth]) {
            Ok(leg) => leg,
            Err(failure) => {
                let took_ms = started.elapsed().as_millis() as u64;
                // The reason is logged and returned because the remedies
                // differ: a deadline is met by raising `timeout_ms` or
                // shortening `depth`, a model error by neither.
                tracing::warn!(
                    error = %failure.error,
                    candidates = depth,
                    rerank_ms = took_ms,
                    "rerank leg failed; falling back to the pre-rerank order"
                );
                return (
                    ranked,
                    Some(RerankTrace::Skipped {
                        reason: failure.skip,
                        candidates: depth,
                        took_ms,
                    }),
                );
            }
        };
        let rerank_ms = started.elapsed().as_millis() as u64;

        if self.rerank_mode == RerankMode::Shadow {
            let changed =
                self.log_shadow_comparison(query.text(), &ranked[..depth], &leg, rerank_ms);
            return (
                ranked,
                Some(RerankTrace::Shadow {
                    judged: leg.len(),
                    changed,
                    took_ms: rerank_ms,
                }),
            );
        }

        self.ranker.apply_rerank(&mut ranked, &leg, now);
        (
            ranked,
            Some(RerankTrace::Applied {
                judged: leg.len(),
                took_ms: rerank_ms,
            }),
        )
    }

    /// Asks the reranker to score the head, honouring
    /// [`rerank_timeout`](Self::rerank_timeout). With a deadline set the
    /// scoring runs on a detached thread, because the contract is
    /// synchronous and a timed-out call must not make the request wait
    /// for inference it has given up on; the runaway call is bounded
    /// server-side by Phase 5's concurrency cap, not here.
    fn rerank_scores(
        &self,
        reranker: &Arc<dyn Reranker>,
        query: &str,
        head: &[ScoredMemory],
    ) -> std::result::Result<Vec<(MemoryId, f32)>, RerankFailure> {
        if self.rerank_timeout.is_zero() {
            let candidates: Vec<RerankCandidate> = head
                .iter()
                .map(|scored| RerankCandidate {
                    id: scored.memory.id(),
                    text: scored.memory.content(),
                })
                .collect();
            return reranker
                .score(query, &candidates)
                .and_then(|scores| align_rerank(head, scores))
                .map_err(RerankFailure::model);
        }

        let reranker = Arc::clone(reranker);
        let query = query.to_string();
        let texts: Vec<(MemoryId, String)> = head
            .iter()
            .map(|scored| (scored.memory.id(), scored.memory.content().to_string()))
            .collect();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let candidates: Vec<RerankCandidate> = texts
                .iter()
                .map(|(id, text)| RerankCandidate { id: *id, text })
                .collect();
            // The receiver may have walked away (timeout): drop silently.
            let _ = sender.send(reranker.score(&query, &candidates));
        });

        match receiver.recv_timeout(self.rerank_timeout) {
            Ok(scores) => scores
                .and_then(|scores| align_rerank(head, scores))
                .map_err(RerankFailure::model),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(RerankFailure {
                skip: RerankSkip::Deadline,
                error: RaError::Internal(format!(
                    "rerank exceeded its {} ms deadline",
                    self.rerank_timeout.as_millis()
                )),
            }),
            // The scoring thread panicked (or died) rather than answering.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(RerankFailure::model(
                RaError::Internal("rerank thread died".to_string()),
            )),
        }
    }

    /// The shadow-mode log (reranking-plan.md Phase 4.4): the comparison
    /// between what fusion served and what the reranker *would* serve, in
    /// ids and ranks only. Never memory content, never the raw query — a
    /// short hash is enough to join a log line to the request that
    /// provoked it, and these logs will outlive the memories they
    /// describe. Event name `rerank_shadow` so Phase 6 can grep a
    /// disagreement rate straight out of the daemon's log.
    /// Whether a second stage exists for this recall: a model is wired and
    /// `mode` is not `off`. `off` reports nothing, so a deployment without the
    /// stage keeps returning exactly what it always returned, field for field.
    fn stage_configured(&self) -> bool {
        self.reranker.is_some() && self.rerank_mode != RerankMode::Off
    }

    fn log_shadow_comparison(
        &self,
        query: &str,
        head: &[ScoredMemory],
        leg: &[(MemoryId, f32)],
        rerank_ms: u64,
    ) -> bool {
        // The *tails* of the ids, not the heads: these are UUIDv7, whose
        // leading bits are the creation timestamp — the head is identical
        // for every memory saved in the same millisecond, which would make
        // "which result moved" unreadable in the log.
        let short = |id: MemoryId| {
            let id = id.to_string();
            id[id.len() - 8..].to_string()
        };
        let top1_before = short(head[0].memory.id());
        let top1_after = short(leg[0].0);
        let changed = top1_before != top1_after;
        tracing::info!(
            name: "rerank_shadow",
            query_hash = %query_hash(query),
            depth = head.len(),
            rerank_ms,
            top1_before = %top1_before,
            top1_after = %top1_after,
            changed,
            before = %head.iter().map(|s| short(s.memory.id())).collect::<Vec<_>>().join(","),
            after = %leg.iter().map(|(id, _)| short(*id)).collect::<Vec<_>>().join(","),
            "rerank_shadow",
        );
        changed
    }
}

/// Why the rerank leg gave up: the machine-readable reason a response
/// reports, and the error worth putting in the log beside it.
struct RerankFailure {
    skip: RerankSkip,
    error: RaError,
}

impl RerankFailure {
    /// Anything that is not the deadline — the model failing, or answering
    /// with scores that do not line up with the candidates it was given.
    fn model(error: RaError) -> Self {
        Self {
            skip: RerankSkip::Model,
            error,
        }
    }
}

/// Pairs each reranker score with the candidate it belongs to (the trait's
/// alignment contract, enforced here) and orders the pairs best-first.
/// Ties keep the incoming — fused — order, so the leg is deterministic in
/// the same sense the fusion is.
fn align_rerank(head: &[ScoredMemory], scores: Vec<f32>) -> Result<Vec<(MemoryId, f32)>> {
    if scores.len() != head.len() {
        return Err(RaError::Internal(format!(
            "reranker returned {} scores for {} candidates",
            scores.len(),
            head.len()
        )));
    }
    let mut leg: Vec<(MemoryId, f32)> = head
        .iter()
        .map(|scored| scored.memory.id())
        .zip(scores)
        .collect();
    leg.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(leg)
}

/// A short, stable hash of the query for shadow logs: enough to correlate
/// a log line with a request, not enough to read the query back.
fn query_hash(query: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(query.as_bytes());
    hex::encode(&digest[..8])
}

/// One leg's `(id, score)` answers in the rank order the fusion wants.
fn ranked(hits: &[(MemoryId, f32)]) -> RankedIds {
    RankedIds(hits.iter().map(|(id, _)| *id).collect())
}

/// The same answers keyed by id, so relevance can be looked up per result
/// after fusion has reordered them.
fn scores(hits: &[(MemoryId, f32)]) -> HashMap<MemoryId, f32> {
    hits.iter().copied().collect()
}

/// Every 1- to [`MAX_SEED_WORDS`]-word window of the query, canonicalised
/// to an [`EntityKey`] the same way the writer filed its entities — so
/// "the billing service team" offers `billing service` and `billing
/// service team` as candidate seeds. Deduplicated, empties dropped; the
/// graph then keeps only the ones some memory actually declared.
fn seed_candidates(text: &str) -> Vec<EntityKey> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for start in 0..words.len() {
        let end = (start + MAX_SEED_WORDS).min(words.len());
        for finish in (start + 1)..=end {
            let key = EntityKey::new(&words[start..finish].join(" "));
            if !key.is_empty() && seen.insert(key.as_str().to_string()) {
                candidates.push(key);
            }
        }
    }
    candidates
}

fn matches(memory: &Memory, query: &RecallQuery, now: chrono::DateTime<chrono::Utc>) -> bool {
    if !query.include_superseded() && !memory.is_active_at(now) {
        return false;
    }
    if !query.categories().is_empty() && !query.categories().contains(memory.category()) {
        return false;
    }
    // Subcategories are OR-ed: matching any one is enough.
    if !query.subcategories().is_empty()
        && !query
            .subcategories()
            .iter()
            .any(|wanted| memory.subcategory() == Some(wanted.as_str()))
    {
        return false;
    }
    // Tags are AND-ed: filters narrow.
    if !query
        .tags()
        .iter()
        .all(|wanted| memory.tags().iter().any(|tag| tag == wanted))
    {
        return false;
    }
    if let Some(since) = query.since()
        && memory.created_at() < since
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, fixed_clock, new_memory, now};
    use crate::memories::domain::category::Category;
    use crate::memories::domain::entity_graph::{EntityGraph, Relation};
    use crate::memories::domain::memory::Entity;
    use crate::memories::domain::recall_ranker::RerankStrategy;

    fn query(text: &str) -> RecallQuery {
        RecallQuery::new(text, 10).unwrap()
    }

    fn contents(results: &[ScoredMemory]) -> Vec<&str> {
        results.iter().map(|s| s.memory.content()).collect()
    }

    fn ids(results: &[ScoredMemory]) -> Vec<MemoryId> {
        results.iter().map(|s| s.memory.id()).collect()
    }

    fn entity(name: &str, kind: &str) -> Entity {
        Entity {
            name: name.to_string(),
            kind: kind.to_string(),
        }
    }

    fn rel(subject: &str, predicate: &str, object: &str) -> Relation {
        Relation {
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
        }
    }

    /// Inserts a memory straight into the repository — no vector or keyword
    /// entry — so its only possible route into recall is the graph. Returns
    /// it so the caller can record its edges and assert on its id.
    fn graph_only_memory(fixture: &Fixture, context: &UserContext, content: &str) -> Memory {
        let memory = Memory::create(context.user_id(), new_memory(content), now()).unwrap();
        fixture.memories.insert(context, &memory, "test").unwrap();
        memory
    }

    #[test]
    fn recalls_a_memory_by_a_paraphrase_of_its_words() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm as their package manager");
        fixture.save(&fixture.alex, "The cat sat on the mat");

        let results = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("which package manager does the user prefer"),
            )
            .unwrap();

        assert_eq!(
            contents(&results).first(),
            Some(&"User prefers pnpm as their package manager")
        );
    }

    #[test]
    fn recall_never_returns_another_users_memories() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "alex prefers pnpm");
        fixture.save(&fixture.sam, "sam prefers pnpm");

        let results = fixture
            .recaller()
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();

        assert_eq!(contents(&results), vec!["alex prefers pnpm"]);
    }

    #[test]
    fn filters_by_category() {
        let fixture = Fixture::new();
        let mut decision = new_memory("We chose SQLite over Postgres for installer size");
        decision.category = Category::Decision;
        fixture
            .saver()
            .execute(&fixture.alex, decision, "test")
            .unwrap();
        fixture.save(&fixture.alex, "We chose pnpm over npm for speed");

        let results = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("we chose").with_categories(vec![Category::Decision]),
            )
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.category(), &Category::Decision);
    }

    #[test]
    fn filters_by_tag_requiring_all_of_them() {
        let fixture = Fixture::new();
        let mut both = new_memory("uses typescript with react");
        both.tags = vec!["typescript".to_string(), "react".to_string()];
        fixture
            .saver()
            .execute(&fixture.alex, both, "test")
            .unwrap();

        let mut one = new_memory("uses typescript on the server");
        one.tags = vec!["typescript".to_string()];
        fixture.saver().execute(&fixture.alex, one, "test").unwrap();

        let results = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("uses typescript")
                    .with_tags(vec!["typescript".to_string(), "react".to_string()]),
            )
            .unwrap();

        assert_eq!(contents(&results), vec!["uses typescript with react"]);
    }

    #[test]
    fn filters_by_creation_time() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "a memory about pnpm");

        let after = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("pnpm").with_since(Some(now() + chrono::Duration::days(1))),
            )
            .unwrap();
        assert!(after.is_empty());

        let before = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("pnpm").with_since(Some(now() - chrono::Duration::days(1))),
            )
            .unwrap();
        assert_eq!(before.len(), 1);
    }

    #[test]
    fn superseded_memories_are_excluded_unless_requested() {
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "deploys on flyio");
        let new = fixture.save(&fixture.alex, "deploys on hetzner");

        let superseded = old.clone().supersede(new.id(), now());
        fixture
            .memories
            .update(&fixture.alex, &superseded, "test")
            .unwrap();

        let default = fixture
            .recaller()
            .execute(&fixture.alex, &query("deploys"))
            .unwrap();
        assert_eq!(contents(&default), vec!["deploys on hetzner"]);

        let including = fixture
            .recaller()
            .execute(&fixture.alex, &query("deploys").including_superseded())
            .unwrap();
        assert_eq!(including.len(), 2);
    }

    #[test]
    fn expired_memories_are_excluded() {
        let fixture = Fixture::new();
        let mut expiring = new_memory("a temporary note about pnpm");
        expiring.expires_at = Some(now() + chrono::Duration::hours(1));
        fixture
            .saver()
            .execute(&fixture.alex, expiring, "test")
            .unwrap();

        // The fixture clock is fixed, so the memory is still live here.
        assert_eq!(
            fixture
                .recaller()
                .execute(&fixture.alex, &query("pnpm"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn deleted_memories_are_not_recalled() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "a note about pnpm");
        fixture
            .memories
            .delete(&fixture.alex, memory.id(), "test", "")
            .unwrap();

        assert!(
            fixture
                .recaller()
                .execute(&fixture.alex, &query("pnpm"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn respects_the_limit() {
        let fixture = Fixture::new();
        for index in 0..5 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }

        let results = fixture
            .recaller()
            .execute(&fixture.alex, &RecallQuery::new("pnpm", 2).unwrap())
            .unwrap();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn an_empty_store_returns_nothing() {
        let fixture = Fixture::new();
        assert!(
            fixture
                .recaller()
                .execute(&fixture.alex, &query("anything"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn recall_survives_a_keyword_index_failure() {
        // Degraded, not broken: the vector leg still answers.
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm");
        fixture.text.fail_next_upsert();

        let results = fixture
            .recaller()
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();

        assert_eq!(results.len(), 1);
    }

    #[test]
    fn recall_records_that_the_returned_memories_were_accessed() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "a note about pnpm");
        assert_eq!(
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .unwrap()
                .last_accessed_at(),
            None
        );

        fixture
            .recaller()
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();

        assert_eq!(
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .unwrap()
                .last_accessed_at(),
            Some(now())
        );
    }

    #[test]
    fn filters_by_subcategory_returns_only_matching() {
        let fixture = Fixture::new();
        let mut testing = new_memory("uses vitest for unit tests");
        testing.subcategory = Some("testing".to_string());
        fixture
            .saver()
            .execute(&fixture.alex, testing, "test")
            .unwrap();

        let mut linting = new_memory("uses eslint for code quality");
        linting.subcategory = Some("linting".to_string());
        fixture
            .saver()
            .execute(&fixture.alex, linting, "test")
            .unwrap();

        let results = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("code").with_subcategories(vec!["testing".to_string()]),
            )
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.subcategory(), Some("testing"));
    }

    #[test]
    fn the_floor_drops_a_result_it_was_measured_against() {
        // The reported symptom: a question with no answer in the store came
        // back full of near-noise, every row wearing a confident score,
        // because that score is a rank position and a rank-1 hit out of
        // nothing still looks like a rank-1 hit.
        //
        // Asserted against the relevance the fixture's own indexes report
        // rather than a hardcoded threshold: the fake embedder is a
        // bag-of-words stand-in whose absolute numbers mean nothing, and a
        // test that pinned one here would be testing the fake.
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm as their package manager");

        let unrelated = query("the cat sat on the mat");
        let measured = fixture
            .recaller()
            .execute(&fixture.alex, &unrelated)
            .unwrap();

        assert_eq!(
            measured.len(),
            1,
            "with no floor the weak candidate comes back, which is what this test needs"
        );
        let noise = measured[0].relevance;

        let above = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &unrelated.clone().with_min_relevance(noise + 0.01),
            )
            .unwrap();
        assert!(
            above.is_empty(),
            "a floor of {} left a result measuring {noise}",
            noise + 0.01
        );

        // And the boundary is inclusive, so a floor set exactly at a
        // result's own relevance does not delete it.
        let at = fixture
            .recaller()
            .execute(&fixture.alex, &unrelated.with_min_relevance(noise))
            .unwrap();
        assert_eq!(
            at.len(),
            1,
            "a floor exactly at {noise} should keep a result measuring {noise}"
        );
    }

    #[test]
    fn a_floor_spares_the_memory_that_actually_answers() {
        // The other half: a floor that only ever deletes things is a broken
        // floor. Between the noise and the real match there has to be room.
        let fixture = Fixture::new();
        let answering = fixture.save(&fixture.alex, "User prefers pnpm as their package manager");
        let distraction = fixture.save(&fixture.alex, "The cat sat on the mat");

        let results = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("which package manager does the user prefer"),
            )
            .unwrap();

        let relevance = |wanted: MemoryId| -> f32 {
            results
                .iter()
                .find(|scored| scored.memory.id() == wanted)
                .unwrap_or_else(|| panic!("{wanted} was not recalled at all"))
                .relevance
        };
        let answer = relevance(answering.id());
        let noise = relevance(distraction.id());

        assert!(
            answer > noise,
            "the answering memory scored {answer}, the distraction {noise}"
        );

        // Anywhere in the gap works, so the assertion does not depend on
        // where the fake model happens to land.
        let between = (answer + noise) / 2.0;
        let floored = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("which package manager does the user prefer").with_min_relevance(between),
            )
            .unwrap();

        assert_eq!(
            contents(&floored),
            vec!["User prefers pnpm as their package manager"],
            "a floor between {noise} and {answer} kept the wrong one"
        );
    }

    #[test]
    fn results_carry_how_they_were_matched() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm");

        let results = fixture
            .recaller()
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();

        let detail = results[0].match_detail;
        assert!(
            detail.vector_rank.is_some() || detail.bm25_rank.is_some(),
            "a result should say which leg found it"
        );
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn a_two_hop_memory_neither_text_leg_ranks_enters_recall_over_the_graph() {
        // The leg's reason to exist (Task 7.3.4): billing service
        // —maintained_by→ Meridian team ←leads— Nadia. A query about the
        // billing service reaches Nadia only over two relations. Both
        // memories are inserted without a vector or keyword entry, so the
        // graph is their sole route in — proving graph evidence, and
        // nothing else, put them in the results.
        let fixture = Fixture::new();
        let graph = fixture.graph();

        let owns = graph_only_memory(
            &fixture,
            &fixture.alex,
            "the billing service is maintained by the Meridian team",
        );
        let leads = graph_only_memory(&fixture, &fixture.alex, "Nadia leads the Meridian team");

        graph
            .record(
                &fixture.alex,
                owns.id(),
                &[
                    entity("billing service", "service"),
                    entity("Meridian team", "team"),
                ],
                &[rel("billing service", "maintained_by", "Meridian team")],
                now(),
            )
            .unwrap();
        graph
            .record(
                &fixture.alex,
                leads.id(),
                &[entity("Nadia", "person"), entity("Meridian team", "team")],
                &[rel("Nadia", "leads", "Meridian team")],
                now(),
            )
            .unwrap();

        let results = fixture
            .recaller_with_graph(graph as Arc<dyn EntityGraph>)
            .execute(
                &fixture.alex,
                &RecallQuery::new("who runs the billing service", 5).unwrap(),
            )
            .unwrap();

        let leads_hit = results
            .iter()
            .find(|s| s.memory.id() == leads.id())
            .expect("the two-hop memory should reach recall over the graph");
        assert!(
            leads_hit.match_detail.graph_rank.is_some(),
            "it entered only as a graph hit"
        );
        assert_eq!(
            leads_hit.match_detail.vector_rank, None,
            "no vector entry was indexed for it"
        );
        assert_eq!(
            leads_hit.match_detail.bm25_rank, None,
            "no keyword entry was indexed for it"
        );
    }

    #[test]
    fn the_graph_leg_never_crosses_users() {
        // Seeding and hopping are both user-scoped: alex querying an entity
        // name sam also used must not reach sam's memory. Both users store
        // an entity called "shared service"; the isolation is the WHERE
        // clause, not the names being distinct.
        let fixture = Fixture::new();
        let graph = fixture.graph();

        let alex_memory = graph_only_memory(
            &fixture,
            &fixture.alex,
            "alex's note about the shared service",
        );
        let sam_memory = graph_only_memory(
            &fixture,
            &fixture.sam,
            "sam's note about the shared service",
        );
        graph
            .record(
                &fixture.alex,
                alex_memory.id(),
                &[
                    entity("shared service", "service"),
                    entity("alex thing", "thing"),
                ],
                &[rel("shared service", "relates_to", "alex thing")],
                now(),
            )
            .unwrap();
        graph
            .record(
                &fixture.sam,
                sam_memory.id(),
                &[
                    entity("shared service", "service"),
                    entity("sam thing", "thing"),
                ],
                &[rel("shared service", "relates_to", "sam thing")],
                now(),
            )
            .unwrap();

        let results = fixture
            .recaller_with_graph(graph as Arc<dyn EntityGraph>)
            .execute(
                &fixture.alex,
                &RecallQuery::new("the shared service", 5).unwrap(),
            )
            .unwrap();

        let found = ids(&results);
        assert!(
            found.contains(&alex_memory.id()),
            "alex's own edge is reachable"
        );
        assert!(
            !found.contains(&sam_memory.id()),
            "the hop crossed into another user's rows"
        );
    }

    #[test]
    fn a_query_naming_no_known_entity_leaves_recall_its_two_leg_self() {
        // The "no seeds → today's behaviour, exactly" guarantee: with a
        // graph present but a query that mentions no stored entity, the leg
        // is silent and the ranking matches the two-leg recaller's, every
        // graph_rank absent.
        let fixture = Fixture::new();
        let graph = fixture.graph();

        let memory = fixture.save(&fixture.alex, "User prefers pnpm as their package manager");
        // The only stored entity is "pnpm", a word the query below never uses.
        graph
            .record(
                &fixture.alex,
                memory.id(),
                &[entity("pnpm", "tool")],
                &[],
                now(),
            )
            .unwrap();

        let question = query("which package manager keeps dependencies tidy");
        let with_graph = fixture
            .recaller_with_graph(graph as Arc<dyn EntityGraph>)
            .execute(&fixture.alex, &question)
            .unwrap();
        let without_graph = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();

        assert_eq!(
            ids(&with_graph),
            ids(&without_graph),
            "an unseeded graph leg changed the ranking"
        );
        assert!(
            with_graph
                .iter()
                .all(|s| s.match_detail.graph_rank.is_none()),
            "nothing should carry a graph rank when the query names no entity"
        );
    }

    // --- the rerank stage (reranking-plan.md Phase 4) -------------------

    use crate::memories::application::test_doubles::{
        FailingReranker, FakeReranker, SleepingReranker,
    };
    use std::sync::Mutex;

    /// Everything a rerank decision may change, as a fingerprint: ids and
    /// order, the exact `f32` bits of both numbers, and the rerank fields.
    /// Two recalls with equal fingerprints are indistinguishable to every
    /// client, which is the identity DoD.
    type Fingerprint = Vec<(String, u32, u32, Option<usize>, Option<u32>)>;

    fn fingerprint(results: &[ScoredMemory]) -> Fingerprint {
        results
            .iter()
            .map(|s| {
                (
                    s.memory.id().to_string(),
                    s.score.to_bits(),
                    s.relevance.to_bits(),
                    s.match_detail.rerank_rank,
                    s.rerank_score.map(f32::to_bits),
                )
            })
            .collect()
    }

    /// The plan's "captured via a tracing test layer": records every
    /// `rerank_shadow` event's fields, flat, for string-level assertions
    /// (including the ones about what must *not* be there).
    #[derive(Clone, Default)]
    struct ShadowLog(Arc<Mutex<Vec<String>>>);

    impl ShadowLog {
        fn lines(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    impl tracing::Subscriber for ShadowLog {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.name() == "rerank_shadow"
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut fields = Vec::new();
            event.record(&mut CollectFields(&mut fields));
            self.0.lock().unwrap().push(fields.join(" "));
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    struct CollectFields<'a>(&'a mut Vec<String>);

    impl tracing::field::Visit for CollectFields<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push(format!("{}={:?}", field.name(), value));
        }
    }

    fn reranker(fake: FakeReranker) -> Arc<FakeReranker> {
        Arc::new(fake)
    }

    #[test]
    fn an_absent_or_off_reranker_leaves_recall_byte_identical() {
        // The phase's anchor test: *no* reranker and `Off` with one loaded
        // must both return the pre-change results — ids, order, scores,
        // relevance, and every rerank field absent — and the loaded
        // reranker must never run.
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");

        let plain = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();

        let fake = reranker(FakeReranker::reversing());
        let off = fixture
            .recaller_with_reranker(
                fake.clone() as Arc<dyn Reranker>,
                RerankMode::Off,
                30,
                Duration::from_secs(5),
            )
            .execute(&fixture.alex, &question)
            .unwrap();

        assert_eq!(
            fingerprint(&plain),
            fingerprint(&off),
            "mode `Off` changed recall"
        );
        assert!(
            plain
                .iter()
                .all(|s| s.rerank_score.is_none() && s.match_detail.rerank_rank.is_none()),
            "the pre-change shape includes rerank fields"
        );
        assert_eq!(fake.calls(), 0, "mode `Off` consulted the reranker");
    }

    /// A recaller over the `Replace` rerank strategy: with the reversing
    /// fake it makes "the rerank order was served" an *exact* assertion.
    /// (The default weight-1 RRF leg by design cannot fully invert a fused
    /// head — that is the strategy's arithmetic, tested on its own in
    /// `recall_ranker`; asserting an inversion through it would be testing
    /// the wrong thing.)
    fn replace_recaller(
        fixture: &Fixture,
        reranker: Arc<dyn Reranker>,
        mode: RerankMode,
        depth: usize,
        timeout: Duration,
    ) -> MemoryRecaller {
        MemoryRecaller::new(
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&fixture.vectors) as Arc<dyn VectorIndex>,
            Arc::clone(&fixture.text) as Arc<dyn TextIndex>,
            Arc::clone(&fixture.embedder) as Arc<dyn Embedder>,
            RecallRanker::new(90).with_rerank_strategy(RerankStrategy::Replace),
            fixed_clock(),
            None,
            2,
            50,
        )
        .with_reranker(reranker, mode, depth, timeout)
    }

    #[test]
    fn on_mode_returns_the_reversed_head_and_reports_the_scores() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");

        let before = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();
        assert!(
            before.len() >= 2,
            "the fixture should offer several results"
        );

        let after = replace_recaller(
            &fixture,
            reranker(FakeReranker::reversing()) as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::from_secs(5),
        )
        .execute(&fixture.alex, &question)
        .unwrap();

        let mut expected = ids(&before);
        expected.reverse();
        assert_eq!(
            ids(&after),
            expected,
            "mode `On` did not serve the rerank order"
        );
        assert!(
            after
                .iter()
                .all(|s| s.match_detail.rerank_rank.is_some() && s.rerank_score.is_some()),
            "reranked results must carry their rerank fields"
        );
    }

    #[test]
    fn a_zero_timeout_scores_inline_and_still_reorders() {
        // The no-deadline branch of `rerank_scores`: same outcome, no
        // thread hop.
        let fixture = Fixture::new();
        for index in 0..3 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        let before = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();

        let after = replace_recaller(
            &fixture,
            reranker(FakeReranker::reversing()) as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::ZERO,
        )
        .execute(&fixture.alex, &question)
        .unwrap();

        let mut expected = ids(&before);
        expected.reverse();
        assert_eq!(ids(&after), expected);
    }

    #[test]
    fn shadow_returns_the_pre_rerank_order_and_logs_only_ids_and_a_hash() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        let plain = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();

        let log = ShadowLog::default();
        let recaller = fixture.recaller_with_reranker(
            reranker(FakeReranker::reversing()) as Arc<dyn Reranker>,
            RerankMode::Shadow,
            30,
            Duration::from_secs(5),
        );
        let after = tracing::subscriber::with_default(log.clone(), || {
            recaller.execute(&fixture.alex, &question).unwrap()
        });

        assert_eq!(
            fingerprint(&plain),
            fingerprint(&after),
            "shadow mode changed what the client sees"
        );

        let lines = log.lines();
        assert_eq!(lines.len(), 1, "shadow mode should log exactly once");
        let line = &lines[0];
        for expected_field in [
            "query_hash=",
            "depth=",
            "rerank_ms=",
            "top1_before=",
            "top1_after=",
            "changed=",
        ] {
            assert!(
                line.contains(expected_field),
                "missing {expected_field} in: {line}"
            );
        }
        assert!(
            line.contains("changed=true"),
            "the reversing fake must produce a disagreement worth logging: {line}"
        );
        // The plan's privacy rule, spelled out as an assertion: the log
        // carries ids and a hash — never memory content, never the query.
        assert!(
            !line.contains("pnpm") && !line.contains("note number"),
            "shadow log leaked memory content or the raw query: {line}"
        );
    }

    #[test]
    fn a_failing_reranker_warns_and_serves_the_pre_rerank_order() {
        let fixture = Fixture::new();
        for index in 0..3 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        let before = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();

        let after = fixture
            .recaller_with_reranker(
                Arc::new(FailingReranker) as Arc<dyn Reranker>,
                RerankMode::On,
                30,
                Duration::from_secs(5),
            )
            .execute(&fixture.alex, &question)
            .unwrap();

        assert_eq!(ids(&before), ids(&after), "a failed leg must not reorder");
    }

    #[test]
    fn an_expired_deadline_falls_back_to_the_pre_rerank_order() {
        let fixture = Fixture::new();
        for index in 0..3 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        let before = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();

        let started = std::time::Instant::now();
        let after = fixture
            .recaller_with_reranker(
                Arc::new(SleepingReranker::new(Duration::from_millis(500))) as Arc<dyn Reranker>,
                RerankMode::On,
                30,
                Duration::from_millis(20),
            )
            .execute(&fixture.alex, &question)
            .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(ids(&before), ids(&after), "an expired leg must not reorder");
        assert!(
            elapsed < Duration::from_millis(400),
            "the deadline was honoured only in the result, not in the time: {elapsed:?}"
        );
    }

    #[test]
    fn pools_and_heads_under_two_candidates_are_left_to_fusion() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "one memory about pnpm");

        // A pool of one: nothing to reorder, so nothing to pay for.
        let fake = reranker(FakeReranker::reversing());
        let single = fixture
            .recaller_with_reranker(
                fake.clone() as Arc<dyn Reranker>,
                RerankMode::On,
                30,
                Duration::from_secs(5),
            )
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(fake.calls(), 0, "a single candidate should not be reranked");

        // A head smaller than two: same conclusion.
        fixture.save(&fixture.alex, "a second memory about pnpm");
        let fake = reranker(FakeReranker::reversing());
        let both = fixture
            .recaller()
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();
        let shallow = fixture
            .recaller_with_reranker(
                fake.clone() as Arc<dyn Reranker>,
                RerankMode::On,
                1,
                Duration::from_secs(5),
            )
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();
        assert_eq!(fake.calls(), 0, "depth 1 should not call the reranker");
        assert_eq!(ids(&both), ids(&shallow));

        // An empty store: the request succeeds with nothing, reranker or not.
        let empty = Fixture::new();
        let none = empty
            .recaller_with_reranker(
                reranker(FakeReranker::reversing()) as Arc<dyn Reranker>,
                RerankMode::On,
                30,
                Duration::from_secs(5),
            )
            .execute(&empty.alex, &query("anything"))
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn depth_deeper_than_the_pool_reranks_the_whole_result_set() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }

        let fake = reranker(FakeReranker::reversing());
        fixture
            .recaller_with_reranker(
                fake.clone() as Arc<dyn Reranker>,
                RerankMode::On,
                1_000,
                Duration::from_secs(5),
            )
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();

        assert_eq!(fake.calls(), 1);
        assert_eq!(
            fake.seen()[0].len(),
            4,
            "the clamp is to the pool, not to the requested depth"
        );
    }

    #[test]
    fn the_filters_run_first_so_the_reranker_never_sees_a_rejected_memory() {
        let fixture = Fixture::new();
        let mut first = new_memory("we chose pnpm over npm");
        first.category = Category::Decision;
        fixture
            .saver()
            .execute(&fixture.alex, first, "test")
            .unwrap();
        let mut second = new_memory("we chose vitest over jest");
        second.category = Category::Decision;
        fixture
            .saver()
            .execute(&fixture.alex, second, "test")
            .unwrap();
        // Same keyword hits, outside the filtered slice: it must never
        // reach the reranker — only the two survivors are candidates.
        let rejected = fixture.save(&fixture.alex, "we chose the cat over the mat");

        let fake = reranker(FakeReranker::by_token_overlap());
        let results = fixture
            .recaller_with_reranker(
                fake.clone() as Arc<dyn Reranker>,
                RerankMode::On,
                30,
                Duration::from_secs(5),
            )
            .execute(
                &fixture.alex,
                &query("chose").with_categories(vec![Category::Decision]),
            )
            .unwrap();

        assert_eq!(results.len(), 2);
        let seen = fake.seen();
        assert_eq!(seen.len(), 1, "the reranker ran exactly once");
        assert_eq!(seen[0].len(), 2, "only the survivors are rerank candidates");
        assert!(
            !seen[0].contains(&rejected.id()),
            "the reranker saw a memory the category filter had already rejected"
        );
    }

    #[test]
    fn reranking_leaves_relevance_alone_and_the_floor_still_applies() {
        // Invariant I-4 and the plan's filter-interaction case: `relevance`
        // is computed exactly as today (so `min_relevance` filters exactly
        // as today) whatever the reranker thinks.
        let fixture = Fixture::new();
        let answering = fixture.save(&fixture.alex, "User prefers pnpm as their package manager");
        let distraction = fixture.save(&fixture.alex, "The cat sat on the mat");
        let question = query("which package manager does the user prefer");

        let plain = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();
        let on = fixture
            .recaller_with_reranker(
                reranker(FakeReranker::reversing()) as Arc<dyn Reranker>,
                RerankMode::On,
                30,
                Duration::from_secs(5),
            )
            .execute(&fixture.alex, &question)
            .unwrap();

        let relevance_of = |results: &[ScoredMemory], wanted: MemoryId| -> f32 {
            results
                .iter()
                .find(|s| s.memory.id() == wanted)
                .unwrap()
                .relevance
        };
        assert_eq!(
            relevance_of(&plain, answering.id()).to_bits(),
            relevance_of(&on, answering.id()).to_bits(),
        );
        assert_eq!(
            relevance_of(&plain, distraction.id()).to_bits(),
            relevance_of(&on, distraction.id()).to_bits(),
        );

        // With the floor between the two, both modes drop the weak one —
        // the floor sees the unchanged relevance, in both modes.
        let between =
            (relevance_of(&plain, answering.id()) + relevance_of(&plain, distraction.id())) / 2.0;
        let floored = fixture
            .recaller_with_reranker(
                reranker(FakeReranker::reversing()) as Arc<dyn Reranker>,
                RerankMode::On,
                30,
                Duration::from_secs(5),
            )
            .execute(
                &fixture.alex,
                &query("which package manager does the user prefer").with_min_relevance(between),
            )
            .unwrap();
        assert_eq!(ids(&floored), vec![answering.id()]);
    }

    // Phase 6.1: measuring a store must not change it.

    #[test]
    fn a_tracked_read_stamps_what_it_returns_and_an_untracked_one_stamps_nothing() {
        let fixture = Fixture::new();
        let saved = fixture.save(
            &fixture.alex,
            "the user prefers pnpm for javascript packages",
        );
        let question = query("pnpm");
        let stored = || {
            fixture
                .memories
                .find(&fixture.alex, saved.id())
                .unwrap()
                .unwrap()
        };

        let tracked = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();
        assert!(
            ids(&tracked).contains(&saved.id()),
            "the fixture must return the memory it saved"
        );
        assert_eq!(
            stored().access_count(),
            1,
            "a client-facing read is the input to importance decay"
        );
        assert_eq!(stored().last_accessed_at(), Some(now()));

        // The audit's path, over the same stores and the same query.
        let untracked = fixture
            .recaller()
            .execute_untracked(&fixture.alex, &question)
            .unwrap();
        assert_eq!(
            ids(&untracked),
            ids(&tracked),
            "an untracked recall must order exactly like a tracked one"
        );
        assert_eq!(
            stored().access_count(),
            1,
            "an untracked recall bumped the access counter it exists to avoid"
        );
        assert_eq!(
            stored().last_accessed_at(),
            Some(now()),
            "an untracked recall restamped last_accessed_at"
        );
    }

    #[test]
    fn an_untracked_recall_still_runs_the_second_stage() {
        // The audit compares orderings, so switching off the bookkeeping must
        // not switch off the leg being audited — otherwise every report would
        // say the reranker changed nothing.
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        let before = ids(&fixture
            .recaller()
            .execute_untracked(&fixture.alex, &question)
            .unwrap());

        let fake = reranker(FakeReranker::reversing());
        let after = replace_recaller(
            &fixture,
            fake.clone() as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::ZERO,
        )
        .execute_untracked(&fixture.alex, &question)
        .unwrap();

        assert_eq!(fake.calls(), 1, "the untracked path skipped the reranker");
        let mut reversed = before;
        reversed.reverse();
        assert_eq!(ids(&after), reversed);
    }

    #[test]
    fn a_cloned_recaller_reads_the_same_stores_and_can_drop_the_stage() {
        // The audit's two handles, and the guarantee they are built on:
        // `with_second_stage` shares everything else, so a difference between
        // the two orderings can only be the stage.
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");

        let fused = fixture
            .recaller()
            .execute_untracked(&fixture.alex, &question)
            .unwrap();
        let with_stage = fixture.recaller().with_reranker(
            reranker(FakeReranker::reversing()) as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::from_secs(5),
        );

        let without = with_stage
            .with_second_stage(None, RerankMode::Off, 30, Duration::from_secs(5))
            .execute_untracked(&fixture.alex, &question)
            .unwrap();
        assert_eq!(
            fingerprint(&without),
            fingerprint(&fused),
            "`with_second_stage(None)` was not the plain pipeline"
        );

        let reranked = with_stage
            .with_second_stage(
                Some(reranker(FakeReranker::reversing()) as Arc<dyn Reranker>),
                RerankMode::On,
                30,
                Duration::from_secs(5),
            )
            .execute_untracked(&fixture.alex, &question)
            .unwrap();
        assert_ne!(
            ids(&reranked),
            ids(&without),
            "attaching the stage through the clone did nothing"
        );
    }

    // ── What a response says about the second stage ──────────────────────
    //
    // A stage that missed its deadline served the pre-rerank order and
    // mentioned it only in the daemon's log, so from the client's side
    // `mode = "on"` on a slow machine was indistinguishable from
    // `mode = "off"`. That is not a hypothetical: a host serving two local
    // models paid ~34 ms per candidate pair and so lost its depth-10 head to
    // the 250 ms deadline on every real query. These tests pin the report,
    // one per way the leg can end.

    #[test]
    fn an_applied_stage_reports_the_head_it_judged() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }

        let report = replace_recaller(
            &fixture,
            reranker(FakeReranker::reversing()),
            RerankMode::On,
            30,
            Duration::from_secs(5),
        )
        .execute_traced(&fixture.alex, &query("pnpm"))
        .unwrap();

        let Some(RerankTrace::Applied { judged, took_ms }) = report.rerank else {
            panic!("expected an applied stage, got {:?}", report.rerank);
        };
        assert!(
            judged >= 2,
            "a head of {judged} was called applied; the leg needs two to reorder"
        );
        assert!(
            took_ms < 5_000,
            "a fake taking {took_ms} ms is the test's fault"
        );
    }

    #[test]
    fn a_stage_that_misses_its_deadline_is_reported_as_skipped() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");

        let traced = replace_recaller(
            &fixture,
            reranker(FakeReranker::sleeping(Duration::from_millis(150))),
            RerankMode::On,
            30,
            Duration::from_millis(20),
        )
        .execute_traced(&fixture.alex, &question)
        .unwrap();
        let plain = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();

        // The safety valve: a late stage serves what fusion produced.
        assert_eq!(
            fingerprint(&traced.results),
            fingerprint(&plain),
            "a timed-out stage changed the order it had given up on"
        );
        // The report: which is exactly what the client could not see before.
        assert!(
            matches!(
                traced.rerank,
                Some(RerankTrace::Skipped {
                    reason: RerankSkip::Deadline,
                    ..
                })
            ),
            "a missed deadline should be reported as such, got {:?}",
            traced.rerank
        );
    }

    #[test]
    fn a_model_that_fails_is_reported_apart_from_one_that_is_slow() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }

        let report = replace_recaller(
            &fixture,
            reranker(FakeReranker::failing()),
            RerankMode::On,
            30,
            Duration::from_secs(5),
        )
        .execute_traced(&fixture.alex, &query("pnpm"))
        .unwrap();

        // The remedies differ — a deadline is raised, a broken model is not —
        // so the two must not share a reason string.
        assert!(
            matches!(
                report.rerank,
                Some(RerankTrace::Skipped {
                    reason: RerankSkip::Model,
                    ..
                })
            ),
            "expected a model failure, got {:?}",
            report.rerank
        );
    }

    #[test]
    fn a_head_of_one_reports_not_needed_rather_than_silence() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "the only note about pnpm");

        let report = replace_recaller(
            &fixture,
            reranker(FakeReranker::reversing()),
            RerankMode::On,
            30,
            Duration::from_secs(5),
        )
        .execute_traced(&fixture.alex, &query("pnpm"))
        .unwrap();

        assert_eq!(report.results.len(), 1, "the fixture is not one memory");
        assert_eq!(
            report.rerank,
            Some(RerankTrace::NotNeeded { candidates: 1 }),
            "a single candidate is a fact about the head, not evidence the \
             stage is missing"
        );
    }

    #[test]
    fn mode_off_reports_nothing_even_with_a_model_wired() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let fake = reranker(FakeReranker::reversing());

        let report = replace_recaller(
            &fixture,
            fake as Arc<dyn Reranker>,
            RerankMode::Off,
            30,
            Duration::from_secs(5),
        )
        .execute_traced(&fixture.alex, &query("pnpm"))
        .unwrap();

        // `None` is what keeps the response of a stage-less deployment
        // exactly what it has always been: no field, no cost, no mention.
        assert_eq!(report.rerank, None, "mode `Off` reported a stage");
    }

    #[test]
    fn shadow_reports_the_head_it_judged_and_what_it_would_have_moved() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        let plain = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();

        let report = replace_recaller(
            &fixture,
            reranker(FakeReranker::reversing()),
            RerankMode::Shadow,
            30,
            Duration::from_secs(5),
        )
        .execute_traced(&fixture.alex, &question)
        .unwrap();

        assert_eq!(
            fingerprint(&report.results),
            fingerprint(&plain),
            "shadow mode changed what the client sees"
        );
        // `changed` is the disagreement rate an operator shadows *for*, so it
        // belongs in the response as well as the log line.
        assert!(
            matches!(
                report.rerank,
                Some(RerankTrace::Shadow {
                    changed: true,
                    judged,
                    ..
                }) if judged >= 2
            ),
            "a reversing fake in shadow should report a would-be move, got {:?}",
            report.rerank
        );
    }
}
