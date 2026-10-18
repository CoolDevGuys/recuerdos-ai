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
    RankedIds, RecallRanker, RerankStrategy, ScoredMemory, apply_expansion,
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
use std::sync::atomic::{AtomicBool, Ordering};
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
        /// Whether the stage was refused a session rather than running out of
        /// inference time.
        ///
        /// Not on the wire, and that is a decision rather than an oversight:
        /// `reason` carries one value for both kinds of "it did not make it",
        /// because a third would break every SDK already shipped (invariant
        /// 3.5), so `RerankResponse` drops this field on its way out. It stays
        /// on the trace because the distinction is the actionable half — "raise
        /// `timeout_ms`" is bad advice for a host that never had a session to
        /// give — and the `warn!` line and `recall audit`'s counters are where
        /// an operator is supposed to read it.
        busy: bool,
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
    /// How many of the served results the hop may reserve a seat in
    /// (`[graph].expansion_slots`, recall-quality-plan.md Phase 3). Also unused
    /// while `graph` is `None` — an empty leg has nothing to promote — and `0`
    /// makes the stage return before reading the list, which is the pre-Phase-3
    /// pipeline by code path rather than by arithmetic.
    expansion_slots: usize,
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
        expansion_slots: usize,
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
            expansion_slots,
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

    /// The same pipeline with its third leg reconfigured.
    ///
    /// Wiring passes the graph knobs to [`new`](Self::new), so the served path
    /// never calls this — it is the test-only mirror of `with_second_stage`, and
    /// the only way to ask for a graph leg *and* a second stage at once, which is
    /// the combination the slot's position in the pipeline is pinned against.
    // *not* used by the wiring, for the reason `rank_with_legs` documents above:
    // the constructor already takes these four, and routing the served path
    // through here would be a change of shape for no gain.
    #[allow(dead_code)]
    pub fn with_graph(
        mut self,
        graph: Option<Arc<dyn EntityGraph>>,
        max_hops: usize,
        hop_limit: usize,
        expansion_slots: usize,
    ) -> Self {
        self.graph = graph;
        self.max_hops = max_hops;
        self.hop_limit = hop_limit;
        self.expansion_slots = expansion_slots;
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

        // The weight on relevance (recall-quality-plan.md Phase 2), applied where
        // the number exists and before everything that consumes the order: the
        // floor below, the rerank head, the truncation. At the shipped default of
        // zero this returns without reading `ranked` at all, which is what keeps
        // "knob at zero, no change" — and the byte-identical eval baseline — a
        // code path rather than an arithmetic coincidence.
        self.ranker.apply_relevance(&mut ranked);

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

        // The graph's reserved slots (recall-quality-plan.md Phase 3), last in the
        // pipeline on purpose. After the floor, so a hop `[retrieval].min_relevance`
        // refused is not served back through the side door; after the rerank leg, so
        // a second stage cannot reorder a hop out of the seat it was given; before
        // the truncate, because the promoted memory is meant to be *inside* what the
        // client asked for, not appended past it. The experiment that measured this
        // gain (§1.3) ran it before the floor; the difference is recorded in §17.
        let promoted = apply_expansion(
            &mut ranked,
            &graph_hits,
            query.limit(),
            self.expansion_slots,
        );
        if promoted > 0 {
            // Debug rather than info: this fires on ordinary relational recall, and
            // an operator who wants to review *which* result moved reads that off
            // `matched.graph_rank`, not off this line. It exists so "the slot never
            // ran" and "the slot ran and changed nothing" are different facts.
            tracing::debug!(
                promoted,
                slots = self.expansion_slots,
                limit = query.limit(),
                "the graph hop took reserved slots"
            );
        }
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
                    // `reason` on the wire is one value for both kinds of
                    // "it did not make it"; here they stay separate, because
                    // "raise timeout_ms / lower depth" is bad advice for a
                    // host that never had a session to begin with.
                    busy = failure.busy,
                    // And the two halves of the cost, which is how that line
                    // can be checked instead of believed: a busy refusal reads
                    // `queued_ms` ≈ the whole budget with `inference_ms` 0, a
                    // slow model the other way round. Both say `deadline`, and
                    // they are not the same machine.
                    queued_ms = failure.queued.as_millis() as u64,
                    inference_ms = failure.inferred.as_millis() as u64,
                    "rerank leg failed; falling back to the pre-rerank order"
                );
                return (
                    ranked,
                    Some(RerankTrace::Skipped {
                        reason: failure.skip,
                        candidates: depth,
                        took_ms,
                        busy: failure.busy,
                    }),
                );
            }
        };
        let rerank_ms = started.elapsed().as_millis() as u64;
        // The split for every leg, at a level nobody runs a host with turned on.
        // It is here anyway because "am I queueing for the reranker?" is the
        // first question a machine with `mode = on` asks, and on a healthy host
        // the answer is in no warning — nothing is wrong, so nothing warns.
        // `RUST_LOG=debug` makes the question answerable, which includes the
        // three-concurrent-recall measurement Phase 1.5 asks for on a real box.
        tracing::debug!(
            name: "rerank_leg",
            query_hash = %query_hash(query.text()),
            judged = leg.scores.len(),
            depth,
            complete = leg.complete,
            rerank_ms,
            queued_ms = leg.queued.as_millis() as u64,
            inference_ms = leg.inferred.as_millis() as u64,
            "rerank stage finished"
        );
        if !leg.complete {
            // The part that finished is applied, so this is not a failure and
            // the client sees `applied`. It is still a host that could not
            // judge the head it was asked to, which is a capacity fact the
            // operator needs from somewhere: here it is a warning naming how
            // much was lost, not a green `applied` that looks like a win.
            tracing::warn!(
                judged = leg.scores.len(),
                depth,
                rerank_ms,
                queued_ms = leg.queued.as_millis() as u64,
                inference_ms = leg.inferred.as_millis() as u64,
                "rerank leg applied the part of the head it finished before its deadline"
            );
        }

        if self.rerank_mode == RerankMode::Shadow {
            let changed =
                self.log_shadow_comparison(query.text(), &ranked[..depth], &leg, rerank_ms);
            return (
                ranked,
                Some(RerankTrace::Shadow {
                    judged: leg.scores.len(),
                    changed,
                    took_ms: rerank_ms,
                }),
            );
        }

        self.ranker.apply_rerank(&mut ranked, &leg.scores, now);
        (
            ranked,
            Some(RerankTrace::Applied {
                judged: leg.scores.len(),
                took_ms: rerank_ms,
            }),
        )
    }

    /// Asks the reranker to score the head, honouring
    /// [`rerank_timeout`](Self::rerank_timeout) in runs of
    /// [`Reranker::preferred_chunk`].
    ///
    /// Two different things are bounded here, and they were two different
    /// bugs.
    ///
    /// *Waiting* for a session is bounded by the deadline itself
    /// ([`Reranker::score_before`]): a saturated host gets `busy` back instead
    /// of queueing a thread behind a head whose request has already been
    /// answered.
    ///
    /// *Judging* is bounded by the chunk. Each run's scores are sent as they
    /// land, and the flag below tells the worker to stop at its next boundary.
    /// ONNX cannot be pre-empted mid-batch, so what a missed deadline abandons
    /// is one run of inference rather than the whole head.
    ///
    /// What finished before the deadline is kept and applied: those ranks are
    /// real evidence about those candidates, and [`apply_rerank`] already treats
    /// a leg covering part of the head as an ordering for that part. Two is the
    /// floor for using it, because one score cannot reorder anything — the same
    /// reason [`RerankTrace::NotNeeded`] exists. A *model* error is not treated
    /// that way: it is reported as the failure it is, because answering
    /// `applied` while the model is broken hides exactly the thing the operator
    /// needs to see.
    ///
    /// [`apply_rerank`]: crate::memories::domain::recall_ranker::RecallRanker::apply_rerank
    fn rerank_scores(
        &self,
        reranker: &Arc<dyn Reranker>,
        query: &str,
        head: &[ScoredMemory],
    ) -> std::result::Result<RerankLeg, RerankFailure> {
        let ids: Vec<MemoryId> = head.iter().map(|scored| scored.memory.id()).collect();
        if self.rerank_timeout.is_zero() {
            let candidates: Vec<RerankCandidate> = head
                .iter()
                .map(|scored| RerankCandidate {
                    id: scored.memory.id(),
                    text: scored.memory.content(),
                })
                .collect();
            // No deadline, no thread, no queue to speak of: this call is made on
            // the request's own thread and every millisecond of it is inference.
            // `rerank audit` is the caller that lands here, which is precisely why
            // it measures what the stage *costs* rather than what it waited.
            let started = std::time::Instant::now();
            let inferred = || started.elapsed();
            return reranker
                .score(query, &candidates)
                .and_then(|scores| align_positions(&ids, scores).map(order_best_first))
                .map(|scores| RerankLeg {
                    scores,
                    complete: true,
                    queued: Duration::ZERO,
                    inferred: inferred(),
                })
                .map_err(|error| RerankFailure::model(error).timed(Duration::ZERO, inferred()));
        }

        // A model with no opinion on run size is scored in one run, which is
        // exactly what this did before chunking existed.
        let chunk = reranker.preferred_chunk().unwrap_or(head.len()).max(1);
        let runs = head.len().div_ceil(chunk);
        let deadline = std::time::Instant::now() + self.rerank_timeout;
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = std::sync::mpsc::channel();
        {
            let reranker = Arc::clone(reranker);
            let query = query.to_string();
            let texts: Vec<(MemoryId, String)> = head
                .iter()
                .map(|scored| (scored.memory.id(), scored.memory.content().to_string()))
                .collect();
            let stop = Arc::clone(&stop);
            // One instant for both halves of the wait: the thread stops waiting
            // for a session at it, and this side stops waiting for the answer at
            // it. Handing the thread a duration instead would let the two drift
            // apart by the time it takes to spawn, which is how a stage ends up
            // scoring a head whose request was answered a moment ago.
            std::thread::spawn(move || {
                for batch in texts.chunks(chunk) {
                    // Checked *before* starting a run, never during: the run
                    // that is already inside the model finishes, and the one
                    // after it is the work this saves.
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let candidates: Vec<RerankCandidate> = batch
                        .iter()
                        .map(|(id, text)| RerankCandidate { id: *id, text })
                        .collect();
                    let scores = reranker.score_before(&query, &candidates, deadline);
                    // The receiver walked away at its deadline: say nothing
                    // further, and let the flag end the loop.
                    if sender.send(scores).is_err() {
                        return;
                    }
                }
            });
        }

        let mut judged: Vec<(MemoryId, f32)> = Vec::with_capacity(head.len());
        let mut queued = Duration::ZERO;
        let mut inferred = Duration::ZERO;
        let mut next_run = 0usize;
        let mut interrupted: Option<RerankFailure> = None;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            // One worker, sending sequentially, so the answers arrive in order
            // and this side's own counter is the run index — no need to carry
            // it across the channel to find out which candidates these are.
            match receiver.recv_timeout(remaining) {
                Ok(Ok(answer)) => {
                    // The split arrives per run and is summed here: a head whose
                    // first run waited 200 ms for a session and whose second ran
                    // clean is a queue story, and the sum is what the log line
                    // has to say to be able to tell it that way.
                    queued += answer.queued;
                    inferred += answer.inferred;
                    let batch = &ids[next_run * chunk..ids.len().min((next_run + 1) * chunk)];
                    match align_positions(batch, answer.scores) {
                        Ok(pairs) => judged.extend(pairs),
                        Err(error) => {
                            interrupted = Some(RerankFailure::model(error));
                            break;
                        }
                    }
                    next_run += 1;
                    if next_run == runs {
                        break;
                    }
                }
                Ok(Err(error)) => {
                    interrupted = Some(RerankFailure::from_error(error));
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    interrupted = Some(RerankFailure::deadline(RaError::Internal(format!(
                        "rerank exceeded its {} ms deadline",
                        self.rerank_timeout.as_millis()
                    ))));
                    break;
                }
                // The scoring thread panicked (or died) rather than answering.
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    interrupted = Some(RerankFailure::model(RaError::Internal(
                        "rerank thread died".to_string(),
                    )));
                    break;
                }
            }
        }
        // Unconditionally, because both ways through this want it: a leg that
        // stopped early must not spend inference on candidates it will not
        // apply, and a complete leg has nothing left to ask the worker for.
        stop.store(true, Ordering::SeqCst);

        let scores = order_best_first(judged);
        // The failures carry the accumulated split with them, which is the point
        // of 1.5: a busy refusal arrives as "250 ms queued, 0 ms judged" and a
        // slow model as "1 ms queued, 249 ms judged". Same `deadline` on the
        // wire, opposite diagnoses.
        match interrupted.map(|failure| failure.timed(queued, inferred)) {
            Some(failure) if failure.skip == RerankSkip::Model => Err(failure),
            Some(failure) if scores.len() < 2 => Err(failure),
            Some(_) => Ok(RerankLeg {
                scores,
                complete: false,
                queued,
                inferred,
            }),
            None => Ok(RerankLeg {
                scores,
                complete: true,
                queued,
                inferred,
            }),
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
        leg: &RerankLeg,
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
        let top1_after = short(leg.scores[0].0);
        let changed = top1_before != top1_after;
        tracing::info!(
            name: "rerank_shadow",
            query_hash = %query_hash(query),
            depth = head.len(),
            // Usually the same as `depth`, and the whole story when it is not:
            // a leg cut off mid-head compares the top of the head with what the
            // stage managed to judge, and a reader who cannot see that number
            // cannot tell a disagreement from an unfinished look.
            judged = leg.scores.len(),
            rerank_ms,
            // Shadow mode is the mode an operator turns on to be told about the
            // stage, so it gets the queue/inference split too: whether the
            // second opinion is *slow* or merely *waiting* is the same question
            // here as on a host serving the results.
            queued_ms = leg.queued.as_millis() as u64,
            inference_ms = leg.inferred.as_millis() as u64,
            top1_before = %top1_before,
            top1_after = %top1_after,
            changed,
            before = %head
                .iter()
                .map(|s| short(s.memory.id()))
                .collect::<Vec<_>>()
                .join(","),
            after = %leg
                .scores
                .iter()
                .map(|(id, _)| short(*id))
                .collect::<Vec<_>>()
                .join(","),
            "rerank_shadow",
        );
        changed
    }
}

/// Why the rerank leg gave up: the machine-readable reason a response
/// reports, and the error worth putting in the log beside it.
struct RerankFailure {
    skip: RerankSkip,
    /// Whether the stage was refused a session rather than running out of
    /// inference time. Both are `deadline` on the wire — see
    /// [`RerankFailure::busy`] — but they are not the same fault line, and
    /// the log and the audit counters say which.
    busy: bool,
    error: RaError,
    /// What the leg had spent by the time it failed, if it had spent anything.
    /// A model error after three judged runs is worth reporting with those
    /// runs' cost, and a busy refusal reports its whole budget as queue and no
    /// inference at all — which is the difference between "the model is slow"
    /// and "there was no model to be had".
    queued: Duration,
    inferred: Duration,
}

impl RerankFailure {
    /// Anything that is not the deadline — the model failing, or answering
    /// with scores that do not line up with the candidates it was given.
    fn model(error: RaError) -> Self {
        Self {
            skip: RerankSkip::Model,
            busy: false,
            error,
            queued: Duration::ZERO,
            inferred: Duration::ZERO,
        }
    }

    /// No session was free in time, so nothing was judged.
    ///
    /// The client keeps hearing `reason: "deadline"`, which is what that
    /// value has always meant — "the stage did not make it" — and adding a
    /// third one would break every SDK already shipped (invariant 3.5). The
    /// distinction lives where the operator reads it: the warn line and the
    /// audit counters.
    fn busy(error: RaError) -> Self {
        Self {
            skip: RerankSkip::Deadline,
            busy: true,
            error,
            queued: Duration::ZERO,
            inferred: Duration::ZERO,
        }
    }

    /// Sorts a model error into the two kinds of "it did not contribute":
    /// the adapter saying it never got a session, and everything else.
    fn from_error(error: RaError) -> Self {
        match error {
            RaError::RerankBusy(_) => Self::busy(error),
            _ => Self::model(error),
        }
    }

    /// Out of time with a session asked for and, in the ordinary case, held:
    /// the wire's `deadline`, and not busy. The stage was judging when the
    /// budget ran out; a busy leg is the one that never started.
    fn deadline(error: RaError) -> Self {
        Self {
            skip: RerankSkip::Deadline,
            busy: false,
            error,
            queued: Duration::ZERO,
            inferred: Duration::ZERO,
        }
    }

    /// Stamps the cost the leg had already accumulated. Separate from the
    /// constructors because they are written where the timings are not yet known,
    /// and because a failure that reported `0 ms` of queue when nothing was
    /// measured would be a number, not a measurement.
    fn timed(mut self, queued: Duration, inferred: Duration) -> Self {
        self.queued = queued;
        self.inferred = inferred;
        self
    }
}

/// Pairs each reranker score with the candidate it belongs to, in the order
/// the candidates were given (the trait's alignment contract, enforced here).
///
/// A run-at-a-time leg builds itself from these: append each run's pairs and
/// order once at the end, and the result is the same list a single whole-head
/// call would have produced, because a score belongs to a `(query, candidate)`
/// pair and not to the batch it happened to travel in.
fn align_positions(head: &[MemoryId], scores: Vec<f32>) -> Result<Vec<(MemoryId, f32)>> {
    if scores.len() != head.len() {
        return Err(RaError::Internal(format!(
            "reranker returned {} scores for {} candidates",
            scores.len(),
            head.len()
        )));
    }
    Ok(head.iter().copied().zip(scores).collect())
}

/// Orders a leg best-first. Ties keep the incoming — fused — order, so the leg
/// is deterministic in the same sense the fusion is.
fn order_best_first(mut leg: Vec<(MemoryId, f32)>) -> Vec<(MemoryId, f32)> {
    leg.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    leg
}

/// What the rerank leg managed to judge: the judged candidates in rank order,
/// and whether they were the whole head.
///
/// The second field is the difference between a stage that ran and one that ran
/// out of time halfway. It is not a new wire value — a client sees
/// `applied` with `judged` below the depth it asked for, which the SDK's
/// existing shape carries — but the log and the audit counters need the fact
/// itself, because "the last two of ten were cut" and "all ten were judged"
/// mean different things about the machine.
struct RerankLeg {
    scores: Vec<(MemoryId, f32)>,
    complete: bool,
    /// Time the runs spent waiting for a session, and time spent judging.
    ///
    /// Carried up from [`Reranker::score_before`] because it can only be measured
    /// where the session is handed out. `took_ms` on the trace is their sum plus
    /// the leg's own bookkeeping, and it is the only one of the three the client
    /// sees; these two are what tells an operator whether the model is slow or
    /// the host is contended.
    queued: Duration,
    inferred: Duration,
}

// Invariant, held by `rerank_scores`: `scores` has at least two entries. A leg
// too short to reorder anything comes back as `Err` (so the response says
// `skipped`) rather than as an applied leg of one, and `log_shadow_comparison`
// relies on that when it reads the leg's top result.

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
// `pub(crate)` for the eval's own test, which asserts which of its cases this
// scanner can and cannot seed (`chain_cases_paraphrase_their_entities_so_no_hop_can_start`).
pub(crate) fn seed_candidates(text: &str) -> Vec<EntityKey> {
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
    use crate::memories::application::test_doubles::{
        BusyReranker, ChunkedReranker, Fixture, fixed_clock, new_memory, now,
    };
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

    // --- graph expansion slots (recall-quality-plan.md Phase 3) ----------

    /// The bridge-and-answer chain from Task 7.3.4's test, in the shape G1
    /// describes: "the billing service is maintained by the Meridian team" is an
    /// ordinary memory the indexes see and rank, and "Nadia leads the Meridian
    /// team" is reachable *only* over the relation between them. Returns both so a
    /// test can point at them.
    fn bridge_and_answer(
        fixture: &Fixture,
        graph: &Arc<crate::memories::infrastructure::sqlite_entity_graph::SqliteEntityGraph>,
    ) -> (Memory, Memory) {
        let bridge = fixture.save(
            &fixture.alex,
            "the billing service is maintained by the Meridian team",
        );
        let answer = graph_only_memory(fixture, &fixture.alex, "Nadia leads the Meridian team");
        graph
            .record(
                &fixture.alex,
                bridge.id(),
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
                answer.id(),
                &[entity("Nadia", "person"), entity("Meridian team", "team")],
                &[rel("Nadia", "leads", "Meridian team")],
                now(),
            )
            .unwrap();
        (bridge, answer)
    }

    /// A memory the indexes *do* see, repeated enough times that the answer —
    /// which they never see — cannot outrank them on fusion alone. That is G1 as
    /// a fixture: the slot is the only thing that can bring it into the window.
    fn direct_notes(fixture: &Fixture, count: usize) {
        for index in 0..count {
            fixture.save(
                &fixture.alex,
                &format!("note {index} about the billing service roster"),
            );
        }
    }

    #[test]
    fn a_reserved_slot_puts_the_connected_answer_second_and_leaves_first_alone() {
        let fixture = Fixture::new();
        let graph = fixture.graph();
        let (bridge, answer) = bridge_and_answer(&fixture, &graph);
        direct_notes(&fixture, 8);

        let question = RecallQuery::new("who owns decisions about the billing service", 5).unwrap();
        let closed = fixture
            .recaller_with_graph(graph.clone() as Arc<dyn EntityGraph>)
            .execute_untracked(&fixture.alex, &question)
            .unwrap();
        let open = fixture
            .recaller_with_graph_slots(graph as Arc<dyn EntityGraph>, 1)
            .execute_untracked(&fixture.alex, &question)
            .unwrap();

        assert!(
            closed.iter().all(|s| s.memory.id() != answer.id()),
            "the fixture no longer reproduces G1: fusion itself returned the answer, \
             so this test is not testing the slot"
        );
        assert_eq!(
            open.first().map(|s| s.memory.id()),
            closed.first().map(|s| s.memory.id()),
            "the slot moved the top result"
        );
        assert_eq!(
            open[1].memory.id(),
            answer.id(),
            "the chain should arrive as bridge + answer; head was {:?}",
            contents(&open)
        );
        assert_eq!(
            contents(&open)
                .iter()
                .filter(|c| **c == bridge.content())
                .count(),
            1,
            "the bridge should appear once"
        );
    }

    #[test]
    fn zero_reserved_slots_leave_the_order_the_score_already_decided() {
        // The pipeline-level anchor. A slot is a *placement*, so the list with the
        // knob at zero is exactly the score's own order, non-increasing from the
        // first row to the last — the property a promotion deliberately breaks, and
        // the one the eval baseline measures at slots = 0 (`change: -0.0`).
        let fixture = Fixture::new();
        let graph = fixture.graph();
        let (_, answer) = bridge_and_answer(&fixture, &graph);
        direct_notes(&fixture, 8);

        let question = RecallQuery::new("who owns decisions about the billing service", 5).unwrap();
        let closed = fixture
            .recaller_with_graph(graph.clone() as Arc<dyn EntityGraph>)
            .execute_untracked(&fixture.alex, &question)
            .unwrap();
        assert!(
            closed.len() >= 3,
            "the fixture returned too little to say anything"
        );
        for pair in closed.windows(2) {
            assert!(
                pair[0].score >= pair[1].score,
                "slots = 0 served a list out of score order"
            );
        }

        // And the knob is live on this fixture: one slot, and the far end of the
        // chain arrives second — taking a seat whose row scores better than it does.
        let open = fixture
            .recaller_with_graph_slots(graph as Arc<dyn EntityGraph>, 1)
            .execute_untracked(&fixture.alex, &question)
            .unwrap();
        assert_eq!(
            open[1].memory.id(),
            answer.id(),
            "the hop did not take the slot: {:?}",
            contents(&open)
        );
        assert!(
            open[1].score <= open[2].score,
            "the promotion rewrote the score, which it is documented not to do"
        );
    }

    #[test]
    fn a_slot_promotes_nothing_when_the_query_names_no_known_entity() {
        let fixture = Fixture::new();
        let graph = fixture.graph();
        let memory = fixture.save(&fixture.alex, "User prefers pnpm as their package manager");
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
        let closed = fixture
            .recaller_with_graph(graph.clone() as Arc<dyn EntityGraph>)
            .execute_untracked(&fixture.alex, &question)
            .unwrap();
        let open = fixture
            .recaller_with_graph_slots(graph as Arc<dyn EntityGraph>, 2)
            .execute_untracked(&fixture.alex, &question)
            .unwrap();

        assert_eq!(ids(&open), ids(&closed));
    }

    #[test]
    fn a_category_filter_the_hop_fails_is_not_bypassed_by_a_slot() {
        // The hop is the one route into the answer, so the graph would put it in
        // slot 1 — but the caller asked for `decision` only, and the filter runs
        // before the slot. A structural slot must not become a way around filters.
        let fixture = Fixture::new();
        let graph = fixture.graph();
        let (bridge, answer) = bridge_and_answer(&fixture, &graph);
        direct_notes(&fixture, 8);

        let results = fixture
            .recaller_with_graph_slots(graph as Arc<dyn EntityGraph>, 1)
            .execute_untracked(
                &fixture.alex,
                &RecallQuery::new("who owns decisions about the billing service", 5)
                    .unwrap()
                    .with_categories(vec![Category::Decision]),
            )
            .unwrap();

        assert!(
            results
                .iter()
                .all(|s| s.memory.category() == &Category::Decision),
            "a filtered memory was served: {:?}",
            contents(&results)
        );
        assert!(results.iter().all(|s| s.memory.id() != answer.id()));
        assert!(results.iter().all(|s| s.memory.id() != bridge.id()));
    }

    #[test]
    fn the_relevance_floor_refuses_a_hop_a_slot_would_otherwise_seat() {
        // A hop's relevance is the `GRAPH_HOP_RELEVANCE` estimate (0.35), so a
        // floor above it deletes relational recall — which is the documented
        // meaning of a high floor, and the slot has to agree with it rather than
        // smuggle the hop back in.
        let fixture = Fixture::new();
        let graph = fixture.graph();
        let (_, answer) = bridge_and_answer(&fixture, &graph);
        direct_notes(&fixture, 8);

        let question = RecallQuery::new("who owns decisions about the billing service", 5)
            .unwrap()
            .with_min_relevance(0.5);
        let results = fixture
            .recaller_with_graph_slots(graph as Arc<dyn EntityGraph>, 1)
            .execute_untracked(&fixture.alex, &question)
            .unwrap();

        assert!(
            results.iter().all(|s| s.relevance >= 0.5),
            "the floor was applied before the slot, not after"
        );
        assert!(results.iter().all(|s| s.memory.id() != answer.id()));
    }

    #[test]
    fn a_limit_of_one_shows_exactly_what_it_show_before_the_slot() {
        let fixture = Fixture::new();
        let graph = fixture.graph();
        bridge_and_answer(&fixture, &graph);
        direct_notes(&fixture, 8);

        let question = RecallQuery::new("who owns decisions about the billing service", 1).unwrap();
        let closed = fixture
            .recaller_with_graph(graph.clone() as Arc<dyn EntityGraph>)
            .execute_untracked(&fixture.alex, &question)
            .unwrap();
        let open = fixture
            .recaller_with_graph_slots(graph as Arc<dyn EntityGraph>, 2)
            .execute_untracked(&fixture.alex, &question)
            .unwrap();

        assert_eq!(ids(&open), ids(&closed));
    }

    #[test]
    fn the_slot_is_applied_after_the_rerank_leg_not_before_it() {
        // The order the two post-fusion stages run in, pinned rather than assumed:
        // the reranker reverses the head it judges (`Replace` over a fake that
        // scores by leading digit), and the hop still lands in slot 1 of *that*
        // list. Run before the rerank leg, the reranker would be free to bury it,
        // which is the behaviour this phase deliberately replaced.
        let fixture = Fixture::new();
        let graph = fixture.graph();
        let (_, answer) = bridge_and_answer(&fixture, &graph);
        direct_notes(&fixture, 8);

        // `Replace` because the default weight-1 RRF leg cannot fully invert a
        // fused head — and a reversal is what makes "before or after the rerank
        // leg" observable rather than a matter of nuance.
        let reverse = Arc::new(FakeReranker::reversing());
        let recaller = fixture
            .recaller_with_reranker(reverse, RerankMode::On, 10, Duration::from_secs(5))
            .with_graph(Some(graph as Arc<dyn EntityGraph>), 2, 50, 1);

        let results = recaller
            .execute_untracked(
                &fixture.alex,
                &RecallQuery::new("who owns decisions about the billing service", 5).unwrap(),
            )
            .unwrap();

        assert_eq!(
            results[1].memory.id(),
            answer.id(),
            "the hop did not take its slot in the reranked list: {:?}",
            contents(&results)
        );
        assert!(
            results[0].match_detail.rerank_rank.is_some(),
            "the rerank leg did not run at all, so this pins nothing"
        );
    }

    #[test]
    fn a_hop_starts_only_from_names_the_graph_confirms_and_a_slot_follows_it() {
        // The G2 guard (plan Task 3.4), pinned rather than assumed. The recaller
        // proposes entity mentions it found in the query text; the *store* decides
        // which ones exist, and `neighbours` is asked about that subset and nothing
        // else. It matters at full volume now that a confirmed hop buys a seat: a
        // fuzzy seed source would hand a reserved slot, and `GRAPH_HOP_RELEVANCE`,
        // to whatever a near match reached — the shape that measured abstention 0.0
        // in §17. A typo is the fuzzy source a real query would supply.
        let fixture = Fixture::new();
        let graph = fixture.graph();
        let (bridge, answer) = bridge_and_answer(&fixture, &graph);
        direct_notes(&fixture, 8);
        let known = bridge.id();

        // Every mention of a query that spells the team wrong, plus one that
        // spells the service right: the store's `seeds` is the only thing that
        // turns a mention into a starting point.
        let typo = RecallQuery::new("who runs the Merdian team", 5).unwrap();
        let with_typo = fixture
            .recaller_with_graph_slots(graph.clone() as Arc<dyn EntityGraph>, 2)
            .execute_untracked(&fixture.alex, &typo)
            .unwrap();
        assert!(
            with_typo
                .iter()
                .all(|s| s.match_detail.graph_rank.is_none() || s.memory.id() == known),
            "a misspelt entity seeded a hop it should not have: {:?}",
            contents(&with_typo)
        );
        assert!(
            with_typo.iter().all(|s| s.memory.id() != answer.id()),
            "the far end of a chain nobody named correctly was promoted into a slot"
        );

        // And the same query with the name spelled as stored does reach it, so the
        // assertion above is about the typo and not about a dead leg.
        let spelled = RecallQuery::new("who runs the Meridian team", 5).unwrap();
        let with_name = fixture
            .recaller_with_graph_slots(graph as Arc<dyn EntityGraph>, 1)
            .execute_untracked(&fixture.alex, &spelled)
            .unwrap();
        assert!(
            with_name
                .iter()
                .any(|s| s.memory.id() == answer.id() && s.match_detail.graph_rank.is_some()),
            "the leg went silent on a name the store does know: {:?}",
            contents(&with_name)
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

    /// The first digit in a fixture memory's content, which is what
    /// `ChunkedReranker` scores on: lets a test compute the order it expects
    /// from the order fusion produced, instead of hard-coding either.
    fn digit(content: &str) -> u32 {
        content
            .chars()
            .find_map(|ch| ch.to_digit(10))
            .unwrap_or_else(|| panic!("no digit in fixture content {content:?}"))
    }

    /// Waits for a detached scoring thread to reach a state.
    ///
    /// The rerank leg's thread outlives the request by design — that is what
    /// letting a deadline expire means — so a counter it owns cannot be read
    /// the moment `execute_traced` returns. Asserting one immediately passes on
    /// an idle machine and fails when the suite runs six test binaries at once,
    /// which is a measurement error in the test rather than a defect found.
    fn waits_for(predicate: impl Fn() -> bool) -> bool {
        for _ in 0..400 {
            if predicate() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

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

    /// The fixture's pipeline over an arbitrary ranker, for the tests that are
    /// about a ranker knob rather than about the fixture's index fakes.
    fn recaller_with(fixture: &Fixture, ranker: RecallRanker) -> MemoryRecaller {
        recaller_with_slots(fixture, ranker, 0)
    }

    /// [`recaller_with`], with `[graph].expansion_slots` named rather than
    /// defaulted — the anchor tests ask for a zero and mean it.
    fn recaller_with_slots(
        fixture: &Fixture,
        ranker: RecallRanker,
        expansion_slots: usize,
    ) -> MemoryRecaller {
        MemoryRecaller::new(
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&fixture.vectors) as Arc<dyn VectorIndex>,
            Arc::clone(&fixture.text) as Arc<dyn TextIndex>,
            Arc::clone(&fixture.embedder) as Arc<dyn Embedder>,
            ranker,
            fixed_clock(),
            None,
            2,
            50,
            expansion_slots,
        )
    }

    /// Five memories the fake indexes agree about, which is what gives affinity
    /// a head to work on.
    fn five_notes(fixture: &Fixture) {
        for index in 0..5 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
    }

    #[test]
    fn every_off_spelling_of_the_relevance_weight_runs_the_same_recall() {
        // δ = 0 has to be identity through the wiring, and now that the shipped
        // default is above zero the pipeline's anchor cannot be "whatever
        // `RecallRanker::new` does" — `new` applies affinity today. So the anchor
        // is that every spelling of off is the *same* run bit for bit: the zero,
        // a negative `[retrieval]` would refuse, and a NaN that escaped it. Both
        // columns are read untracked: `execute` records access, importance decay
        // reads access, and a test comparing two runs would otherwise be
        // measuring its own first run.
        let fixture = Fixture::new();
        five_notes(&fixture);
        let question = query("pnpm");
        let fingerprint = |results: &[ScoredMemory]| {
            results
                .iter()
                .map(|scored| {
                    (
                        scored.memory.id(),
                        scored.score.to_bits(),
                        scored.relevance.to_bits(),
                    )
                })
                .collect::<Vec<_>>()
        };

        let weights = [0.0_f32, -0.1, f32::NAN];
        let runs: Vec<Vec<ScoredMemory>> = weights
            .iter()
            .map(|weight| {
                recaller_with(
                    &fixture,
                    RecallRanker::new(90).with_relevance_weight(*weight),
                )
                .execute_untracked(&fixture.alex, &question)
                .unwrap()
            })
            .collect();
        assert!(runs[0].len() >= 2, "the fixture should return a head");

        let reference = fingerprint(&runs[0]);
        for (weight, run) in weights[1..].iter().zip(&runs[1..]) {
            assert_eq!(
                fingerprint(run),
                reference,
                "δ = {weight} changed recall through the pipeline"
            );
        }
    }

    #[test]
    fn a_relevance_weight_discounts_every_result_by_exactly_its_relevance() {
        // What the knob does to a real recall, asserted as the formula rather
        // than as one particular swap: each score becomes the fused one times
        // `1 − δ·(1 − relevance)`, relevance itself is untouched, no row is
        // dropped or added, and the list is ordered by the discounted numbers.
        // Pinning a specific reorder here would test the fixture's index fakes
        // rather than the stage. The shipped default runs through it alongside a
        // deliberately loud 0.2, so the number a deployment actually gets is the
        // one under test.
        let fixture = Fixture::new();
        five_notes(&fixture);
        let question = query("pnpm");
        let plain = recaller_with(&fixture, RecallRanker::new(90).with_relevance_weight(0.0))
            .execute_untracked(&fixture.alex, &question)
            .unwrap();
        assert!(
            plain.iter().any(|scored| scored.relevance < 1.0),
            "the fixture has nothing for affinity to discount — this test would pass vacuously"
        );

        for delta in [
            0.2_f32,
            crate::memories::domain::recall_ranker::DEFAULT_RELEVANCE_WEIGHT,
        ] {
            let tuned = recaller_with(&fixture, RecallRanker::new(90).with_relevance_weight(delta))
                .execute_untracked(&fixture.alex, &question)
                .unwrap();

            assert_eq!(tuned.len(), plain.len(), "δ = {delta} changed the row set");
            for scored in &tuned {
                let before = plain
                    .iter()
                    .find(|candidate| candidate.memory.id() == scored.memory.id())
                    .unwrap_or_else(|| {
                        panic!("{} appeared only after affinity", scored.memory.id())
                    });
                assert_eq!(
                    scored.relevance.to_bits(),
                    before.relevance.to_bits(),
                    "affinity rewrote the relevance it was only meant to read"
                );
                let expected = before.score * (1.0 - delta * (1.0 - before.relevance));
                assert!(
                    (scored.score - expected).abs() <= expected.abs() * 1e-6 + f32::EPSILON,
                    "{}: expected {} after δ = {delta}, got {}",
                    scored.memory.id(),
                    expected,
                    scored.score
                );
            }
            assert!(
                tuned
                    .windows(2)
                    .all(|pair| pair[0].score >= pair[1].score || pair[1].score.is_nan()),
                "δ = {delta}: the discounted list is not ordered by the discounted score"
            );
        }
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
            0,
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

    /// Phase 1.2, without a race: the session is already held, so the only
    /// thing this recall can do is miss its budget while queueing.
    #[test]
    fn a_stage_that_cannot_get_a_session_is_skipped_as_a_deadline() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let busy = Arc::new(BusyReranker::new(Duration::from_millis(5)));
        let held = busy.hold_session();

        let report = replace_recaller(
            &fixture,
            Arc::clone(&busy) as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::from_millis(20),
        )
        .execute_traced(&fixture.alex, &query("pnpm"))
        .unwrap();

        // The wire says what it has always said for "the stage did not make
        // it": no third reason value, because the shipped SDKs reject one.
        assert!(
            matches!(
                report.rerank,
                Some(RerankTrace::Skipped {
                    reason: RerankSkip::Deadline,
                    ..
                })
            ),
            "a busy stage should be `skipped`/`deadline`, got {:?}",
            report.rerank
        );
        // The session stays held while the refusal is waited for. Releasing it
        // here would let a worker that the scheduler had not run yet take it
        // *after* its own deadline and judge the head, and then there would be
        // no refusal left to observe — the test would be reading whether this
        // thread or that one won.
        assert!(
            waits_for(|| busy.refusals() == 1),
            "the stage should have said busy; it has refused {} times",
            busy.refusals()
        );
        assert_eq!(busy.judged(), 0, "a refused stage must not score anything");
        drop(held);
    }

    /// Phase 1.2's concurrency case: with one session and everything slower
    /// than the budget, the second recall is refused rather than turned into
    /// a queue that then does work nobody is waiting for.
    ///
    /// The intervals are deliberately far apart (a head that costs ten times
    /// the budget) and the start of the second call is gated on the first
    /// actually holding the session: an earlier version used 150 ms against
    /// 20 ms and slept 20 ms to get there, and it failed the first time the
    /// suite ran with six other test binaries competing for the CPU.
    #[test]
    fn a_second_recall_does_not_queue_behind_a_busy_session() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        let busy = Arc::new(BusyReranker::new(Duration::from_millis(400)));
        let recaller = || {
            replace_recaller(
                &fixture,
                Arc::clone(&busy) as Arc<dyn Reranker>,
                RerankMode::On,
                30,
                Duration::from_millis(40),
            )
        };
        let first_recaller = recaller();
        let second_recaller = recaller();

        std::thread::scope(|scope| {
            let first = scope.spawn(|| first_recaller.execute_traced(&fixture.alex, &question));
            // Not a sleep: wait until the first caller is *inside* the session,
            // so the second one's refusal is about the pool and not about how
            // fast this machine scheduled a thread.
            assert!(
                busy.wait_until_acquired(Duration::from_secs(2)),
                "the first recall never reached the session to hold it busy"
            );

            let second = second_recaller
                .execute_traced(&fixture.alex, &question)
                .unwrap();
            let took_ms = match second.rerank {
                Some(RerankTrace::Skipped {
                    reason: RerankSkip::Deadline,
                    took_ms,
                    ..
                }) => took_ms,
                other => {
                    panic!("a busy stage is `skipped`/`deadline` to the client, got {other:?}")
                }
            };
            // The leg's own elapsed time rather than the whole request's:
            // retrieval takes whatever this machine gives it, and what is under
            // test is that the stage gave up at its budget instead of waiting for
            // a session. A queued leg reports the cost of the head in flight —
            // 400 ms — so 200 still separates refusing from queueing by a
            // factor of two without depending on how fast a thread wakes here.
            assert!(
                took_ms <= 200,
                "the rerank leg took {took_ms} ms against a 40 ms budget: that looks like \
                 queueing for the session, not refusing (the head in flight costs 400 ms)"
            );
            assert!(
                waits_for(|| busy.refusals() == 1),
                "the second recall should have been refused, having scored nothing; \
                 refusals so far: {}",
                busy.refusals()
            );
            // What it did *not* do is asserted after the scope, once the first
            // head has certainly finished: `judged` counts both callers, so
            // reading it here would be reading a race.
            first.join().unwrap().unwrap();
        });

        // What the refusal bought: exactly one head was ever judged, and — the
        // exact statement of "it did not queue" — the session was handed out
        // once. Before the timed acquire, the abandoned caller blocked on the
        // session, took a second token, and judged a request the client had
        // already been answered; that accumulating work is what
        // reranking-plan §12 (part three) measured as 34 → 120 ms per pair on a
        // busy host.
        assert!(
            waits_for(|| busy.judged() == 1),
            "the head that got the session should have been judged, and it was \
             {} times",
            busy.judged()
        );
        assert_eq!(
            busy.acquired(),
            1,
            "the refused recall took a session: it queued behind the head in \
             flight instead of giving up"
        );
    }

    /// Phase 1.3: a deadline that arrives mid-head keeps the part that
    /// finished, and the rest of the head is left exactly as fusion ordered it.
    #[test]
    fn a_head_finished_part_way_is_applied_and_the_tail_keeps_its_order() {
        let fixture = Fixture::new();
        for index in 0..8 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        // One candidate a run, 60 ms each against 200 ms of budget: three land,
        // the fourth would arrive at 240 and is dropped. How many *actually*
        // made it is read back from the trace, because a machine competing with
        // five other test binaries shifts those arrivals — this test is about
        // the shape of a partial leg, not about this many milliseconds.
        let chunked = Arc::new(ChunkedReranker::new(1, Duration::from_millis(60)));
        let traced = replace_recaller(
            &fixture,
            Arc::clone(&chunked) as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::from_millis(200),
        )
        .execute_traced(&fixture.alex, &question)
        .unwrap();

        let fused = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();
        let head_len = fused.len();
        let judged = match traced.rerank {
            Some(RerankTrace::Applied { judged, .. }) if judged >= 2 && judged < head_len => judged,
            other => panic!(
                "a head of {head_len} scored 60 ms a candidate against a 200 ms budget \
                 should have been judged part way, got {other:?}"
            ),
        };

        // The digit in "note number N about pnpm" is this double's score, so the
        // expected order is computed from the fused one rather than assumed: the
        // judged prefix reordered by score, the tail exactly as fusion left it.
        let by_digit: Vec<(MemoryId, u32)> = fused
            .iter()
            .map(|scored| (scored.memory.id(), digit(scored.memory.content())))
            .collect();
        let mut head = by_digit[..judged].to_vec();
        head.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let mut expected: Vec<String> = head.iter().map(|(id, _)| id.to_string()).collect();
        expected.extend(by_digit[judged..].iter().map(|(id, _)| id.to_string()));
        let got: Vec<String> = traced
            .results
            .iter()
            .map(|scored| scored.memory.id().to_string())
            .collect();

        assert_eq!(
            got, expected,
            "the {judged} judged candidates should be reordered into the top \
             {judged}, the tail left alone"
        );
    }

    /// Phase 1.3's cancellation bound: the run already inside the model
    /// finishes (it cannot be pre-empted), the next one is never started, and
    /// the stage is not left stuck afterwards.
    #[test]
    fn a_leg_that_gives_up_stops_at_the_next_chunk_boundary() {
        let fixture = Fixture::new();
        for index in 0..8 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        let chunked = Arc::new(ChunkedReranker::new(1, Duration::from_millis(60)));
        // How many candidates the stage is asked about at all: the relevance
        // floor decides that, and this test is about what happens *inside* a
        // head, not about how deep this fixture's happens to be.
        let head_len = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap()
            .len();

        let traced = replace_recaller(
            &fixture,
            Arc::clone(&chunked) as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::from_millis(200),
        )
        .execute_traced(&fixture.alex, &question)
        .unwrap();
        let judged = match traced.rerank {
            Some(RerankTrace::Applied { judged, .. }) if judged < head_len => judged,
            other => panic!(
                "a 200 ms budget against 60 ms a candidate should have abandoned \
                 part of a head of {head_len}, got {other:?}"
            ),
        };
        // Long enough for anything the worker went on to do to finish and be
        // counted.
        std::thread::sleep(Duration::from_millis(400));

        assert!(
            chunked.runs() <= judged + 1,
            "the caller gave up after judging {judged}; the run already in flight was \
             allowed to finish, so at most {} runs of work should have been spent, not {}",
            judged + 1,
            chunked.runs()
        );

        // Not stuck: a recall with room to think still judges the whole head.
        let full = replace_recaller(
            &fixture,
            Arc::clone(&chunked) as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::from_secs(5),
        )
        .execute_traced(&fixture.alex, &question)
        .unwrap();
        assert!(
            matches!(
                full.rerank,
                Some(RerankTrace::Applied { judged, .. }) if judged == head_len
            ),
            "a stage that abandoned a head must still serve a later one in full \
             ({head_len} candidates), got {:?}",
            full.rerank
        );
    }

    /// One score cannot reorder anything, so a leg that got one candidate is a
    /// skip, not an `applied` of one: the client keeps the fused order and is
    /// told the stage did not contribute.
    #[test]
    fn one_judged_candidate_is_a_skip_rather_than_an_applied_of_one() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        // 150 ms a candidate against a 200 ms budget: one lands, and the second
        // would arrive at 300. The ratio is wide so that "exactly one" is not a
        // statement about how the scheduler treats this process today.
        let chunked = Arc::new(ChunkedReranker::new(1, Duration::from_millis(150)));
        let traced = replace_recaller(
            &fixture,
            chunked as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::from_millis(200),
        )
        .execute_traced(&fixture.alex, &question)
        .unwrap();
        let plain = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();

        assert!(
            matches!(
                traced.rerank,
                Some(RerankTrace::Skipped {
                    reason: RerankSkip::Deadline,
                    ..
                })
            ),
            "one judged candidate is not a reorder, got {:?}",
            traced.rerank
        );
        assert_eq!(
            fingerprint(&traced.results),
            fingerprint(&plain),
            "a leg that was not applied changed the order"
        );
    }

    /// The property the whole chunked path rests on: batching changes when the
    /// recaller can take a rank, never which rank a candidate gets.
    #[test]
    fn runs_are_a_yield_point_and_not_a_change_to_the_ranking() {
        let fixture = Fixture::new();
        for index in 0..4 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }
        let question = query("pnpm");
        let one_at_a_time = Arc::new(ChunkedReranker::new(1, Duration::ZERO));
        let whole_head = Arc::new(ChunkedReranker::new(8, Duration::ZERO));

        let chunked = replace_recaller(
            &fixture,
            Arc::clone(&one_at_a_time) as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::from_secs(5),
        )
        .execute_traced(&fixture.alex, &question)
        .unwrap();
        let unchunked = replace_recaller(
            &fixture,
            Arc::clone(&whole_head) as Arc<dyn Reranker>,
            RerankMode::On,
            30,
            Duration::from_secs(5),
        )
        .execute_traced(&fixture.alex, &question)
        .unwrap();

        assert_eq!(
            fingerprint(&chunked.results),
            fingerprint(&unchunked.results),
            "scoring the head one candidate at a time changed the ranking"
        );
        // And the runs really happened: an identical result from two paths that
        // both scored once would prove nothing. The head's depth comes from the
        // trace rather than from the fixture, because the relevance floor
        // decides how much of the five the stage is asked about.
        let judged = match chunked.rerank {
            Some(RerankTrace::Applied { judged, .. }) => judged,
            other => panic!("a generous budget should judge the whole head, got {other:?}"),
        };
        assert_eq!(
            one_at_a_time.runs(),
            judged,
            "a chunk of 1 should spend one run per judged candidate"
        );
        assert_eq!(whole_head.runs(), 1, "a chunk of 8 should be one run");
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
