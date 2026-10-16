//! Fuses the vector and keyword result lists into one ranking.
//!
//! # Why reciprocal rank fusion
//!
//! The two legs of hybrid search produce incomparable numbers: cosine
//! distance from sqlite-vec and BM25 from tantivy live on different
//! scales, and BM25's scale moves with the corpus. Normalising them
//! against each other means inventing a conversion that drifts as the
//! corpus grows.
//!
//! RRF sidesteps that by discarding the scores and keeping only the
//! *ranks*: each list contributes `1 / (k + rank)`. A memory both legs
//! rank highly beats one that only a single leg loves, which is exactly
//! the behaviour hybrid search is for — semantic paraphrase from the
//! vector leg, exact identifiers (`useQuery`, `pnpm`) from the keyword
//! leg.
//!
//! # Why recency and confidence are bounded multipliers
//!
//! project-plan.md §8 gives the shape `rrf × recency × confidence`. Taken
//! literally with an exponential decay, a two-year-old memory at a 90-day
//! half-life scores ~0.004 — effectively deleted, which is wrong for a
//! store whose whole job is remembering an architecture decision made
//! last year.
//!
//! So both multipliers are floored into a narrow band. The width of that
//! band is not a taste call — it has to be calibrated against RRF's rank
//! spacing, and getting it wrong silently inverts the ranking:
//!
//! - With `k = 60`, adjacent ranks differ by ~1.6% and ranks 1 vs 20 by
//!   ~24%.
//! - A multiplier spanning 2× (floor 0.5) therefore swamps *any* rank
//!   difference — a fresh memory nobody ranked highly would outrank the
//!   single best match. Recency would become the primary sort key and
//!   relevance the tiebreak, which is precisely backwards.
//! - At [`MULTIPLIER_FLOOR`] = 0.8 the band is 20%: enough to reorder
//!   near-adjacent results, not enough to leapfrog a memory that is many
//!   ranks better.
//!
//! # Why importance shares confidence's band instead of getting its own
//!
//! Importance (`consolidation::domain::decay`) is the third signal, and
//! it is deliberately *not* a third multiplier. Each floored band spans
//! 1.25x, and they compound: two bands already span 1.56x, against the
//! 1.33x that separates rank 1 from rank 20. A third would take the
//! product to 1.95x and quietly make decay able to bury the best match
//! in the corpus — precisely the failure the floor exists to prevent.
//!
//! So confidence and importance are multiplied *first* and floored
//! *once*. Both answer the same question — how much is this memory
//! worth, independent of the query — and treating them as one band means
//! adding decay cannot widen the spread beyond what confidence alone
//! already did. That property is pinned by a test below; anything that
//! adds a fourth signal should join this band, not open another.
//!
//! Relevance leads; recency and quality break ties among comparably
//! relevant results. All of it is pinned by tests below.
//!
//! # Second stage: the rerank leg (reranking-plan.md Phase 4)
//!
//! When a [`Reranker`](super::reranker::Reranker) is wired, its scores
//! over the fused head enter the ranking as a fourth input. Two strategies
//! decide *how*, both implemented here and selected by
//! [`RecallRanker::with_rerank_strategy`] (config arrives in Phase 5; the
//! default is the RRF leg).
//!
//! ## `Rrf { weight }` — a fourth fusion leg
//!
//! The rerank list joins the RRF sum as `weight × 1/(k + rank)`, so the
//! bounded recency/quality band applies to it exactly as to the other
//! legs. The leg arithmetic says what each weight *means*; the Phase 7
//! grid (§12) says what it *buys*, and above 1 the two disagreed:
//!
//! - one retrieval leg at rank 1 contributes `1/61 ≈ 0.0164`; two legs
//!   agreeing at rank 1, `2/61 ≈ 0.0328`.
//! - `weight = 0` is off — the leg contributes nothing and the fused order
//!   survives untouched (pinned by test).
//! - `weight = 1` lets the reranker break ties *against a single leg* but
//!   never overrule vector+BM25 agreement.
//! - `weight ≥ 2` overvotes that agreement. The plan expected the wins to
//!   live there, on the theory that vector and BM25 can agree on a wrong
//!   answer; measured on the expanded 108-memory eval set, they nearly
//!   never do. The quality plateau is 0.5–1.0 (identical p@1 and MRR
//!   across it); `2` gives back 2 points of p@1 and a third of the
//!   `needle` kind's, and `4` costs the category filter half its
//!   precision too. A leg strong enough to let a confident dissent beat
//!   two agreeing index signals is strong enough to lose the memory both
//!   of them found — and task 7.4's distribution shows how often this
//!   model *is* the dissenter: it scores correct pairs near 0.006 as
//!   readily as near 0.99. The default is the plateau's top: 1.0.
//!
//! ## `Replace` — the rerank score becomes the head's head
//!
//! The reranked candidates are re-ordered by their raw scores multiplied
//! by the *same* [`MULTIPLIER_FLOOR`]-bounded recency/quality band, and
//! form the head; every memory the reranker never saw keeps its fused
//! relative order directly behind it. Because the band spans only 1.25×
//! ([`MULTIPLIER_FLOOR`] = 0.8), recency and quality can decide ties and
//! gaps under 25%, and cannot bury a clear rerank-score winner — the
//! identical argument that bounds the other multipliers, applied to a new
//! signal instead of a new band (invariant from §"Why importance shares
//! confidence's band"). The raw scores must therefore be non-negative and
//! comparable across candidates of one call — adapters on logit-scale
//! models must squash them first; that is a Phase 5 obligation.
//!
//! Phase 7's grid answered the survival question: not as a default. Its
//! premise — that a raw score's *gaps* mean something — is exactly what
//! task 7.4's distribution denies: expected-pair scores span the full
//! range with a pile-up near zero, so on three of four grid models the
//! banded raw scores lost precision (worst −9.6) — a head of near-zero
//! scores reorders into band noise. The one model crisp enough to gain
//! from `Replace` (+5.8 p@1, §12) needed a depth that spent 96% of the
//! latency budget, and its gain sat on a cliff: one step shallower and
//! the `needle` kind regressed by a third. `Rrf`, reading only the
//! *order* the scores imply, delivered the same +5.8 on the chosen model
//! at depth 10 for half the cost, flat across depths.
//!
//! Neither strategy touches `relevance`: the cross-encoder value is
//! reported beside it as [`ScoredMemory::rerank_score`] (invariant I-4 —
//! see `memories::domain::reranker` for why a cross-encoder probability
//! must not silently *become* relevance on an unmeasured distribution).

use super::memory::Memory;
use crate::shared::ids::MemoryId;
use chrono::{DateTime, Utc};

/// RRF's damping constant. 60 is the value from the original paper and
/// the de-facto default; it flattens the difference between ranks 1 and 2
/// enough that one leg cannot dominate on its own.
const RRF_K: f32 = 60.0;

/// The weakest multiplier recency or confidence may apply.
///
/// Calibrated against `RRF_K` — see the module docs. Widening this
/// without re-reading them will quietly turn recency into the primary
/// sort key.
const MULTIPLIER_FLOOR: f32 = 0.8;

/// The graph-leg weight `new` uses: `1.0`, i.e. the graph leg contributes
/// its full `1/(k+rank)` term, byte-for-byte the pre-7.3.7 behaviour. The
/// precision-recovery tuning is opt-in — wiring applies the configured
/// weight via [`RecallRanker::with_graph_ranking`], so a plain `new` ranker
/// (and every test built on one) is unchanged.
const DEFAULT_GRAPH_RANK_WEIGHT: f32 = 1.0;

/// The unanchored-floor `new` uses: `0`, i.e. no cap. See
/// [`RecallRanker::with_graph_ranking`] for what a non-zero value does.
const DEFAULT_GRAPH_UNANCHORED_FLOOR: usize = 0;

/// The rerank stage's reordering strategy (reranking-plan.md Phase 4.3).
/// See the module docs for the calibration argument behind each variant.
#[derive(Debug, Clone, Copy)]
pub enum RerankStrategy {
    /// The rerank list joins the RRF sum as a fourth leg, damped by
    /// `weight`. `weight = 0` is off; the *measured* plateau is 0.5–1.0
    /// and values ≥ 2 are measured harm (the module docs carry the
    /// grid numbers behind that).
    Rrf { weight: f32 },
    /// The reranked head is ordered by raw score × the bounded
    /// recency/quality band; everything the reranker never saw follows
    /// in its fused order. Assumes non-negative, mutually comparable
    /// raw scores — the Phase 5 adapter squashes every model's logits into
    /// `(0, 1)` for exactly this contract, monotonically so no rank moves.
    Replace,
}

/// A full-weight fourth RRF leg — and the Phase 7 grid's chosen value,
/// unchanged: `1.0` is the top of the measured 0.5–1.0 plateau, the
/// semantics where the reranker breaks ties against a single retrieval
/// leg without ever overruling vector+keyword agreement (the module docs
/// carry the grid numbers, including what heavier weights cost). The
/// constant stays so the default is stated in the calibrated style
/// rather than inline.
const DEFAULT_RERANK_WEIGHT: f32 = 1.0;

impl Default for RerankStrategy {
    fn default() -> Self {
        Self::Rrf {
            weight: DEFAULT_RERANK_WEIGHT,
        }
    }
}

/// One leg's opinion: memory ids in rank order, best first.
#[derive(Debug, Clone, Default)]
pub struct RankedIds(pub Vec<MemoryId>);

/// How a memory was found — surfaced to clients so a surprising result
/// can be explained rather than just distrusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchDetail {
    /// 1-based rank in the vector leg, if it appeared there.
    pub vector_rank: Option<usize>,
    /// 1-based rank in the keyword leg, if it appeared there.
    pub bm25_rank: Option<usize>,
    /// 1-based rank in the graph leg, if a hop reached it (Task 7.3.4).
    /// A memory neither the vector nor the keyword leg surfaced can still
    /// earn a place here, reachable only over the relations it shares with
    /// the query's entities.
    pub graph_rank: Option<usize>,
    /// 1-based rank in the rerank stage, if it scored this memory
    /// (reranking-plan.md Phase 4). The rank lives here while its raw
    /// score rides beside it on [`ScoredMemory`]: this type derives `Eq`,
    /// and a model's raw `f32` is not ours to promise equality semantics
    /// for — a position is.
    pub rerank_rank: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ScoredMemory {
    pub memory: Memory,
    /// The fused rank score. Comparable *within* one result set, which is
    /// all it was ever meant for: it is built from RRF positions, so it
    /// says how a result ranks, never how well it matches. A query that
    /// matches nothing still has a top result with a healthy-looking score.
    pub score: f32,
    /// How relevant this memory is to the query in absolute terms, in
    /// `0.0..=1.0`, measured from the index legs' own signals rather than
    /// their positions.
    ///
    /// This is the number that can be thresholded, and the one a caller
    /// should show. It is deliberately *not* the sort key — [`score`] is
    /// that, and the calibration documented at the top of this module
    /// depends on it — so relevance may be non-monotonic down the list. A
    /// keyword leg that matched an exact identifier and a vector leg that
    /// matched a paraphrase disagree about which is more relevant, and the
    /// fusion is the tiebreak; both orderings are defensible, and RRF is
    /// the one with a body of evidence behind it.
    ///
    /// `0.0` means no leg measured this at all — a memory that entered
    /// recall through the graph, whose relevance is estimated rather than
    /// measured.
    pub relevance: f32,
    /// The rerank stage's own score for this memory, present only when a
    /// reranker scored it (reranking-plan.md Phase 4). Deliberately *not*
    /// folded into `relevance`: the value is on the model's scale —
    /// probabilities or logits, uncalibrated on this corpus — so it is a
    /// ranker's input and an operator's diagnostic, never a threshold
    /// (see the `reranker` module docs and invariant I-4 of the plan).
    pub rerank_score: Option<f32>,
    pub match_detail: MatchDetail,
}

/// The fusion tuning, as a value.
///
/// `Clone` (every field is a `Copy` number) because the audit hands a copy
/// of one configured ranker to two recallers — the pipeline with the rerank
/// stage and the pipeline without it (reranking-plan.md Phase 6.2) — and the
/// comparison is only worth making if both sides fuse identically.
#[derive(Clone)]
pub struct RecallRanker {
    recency_half_life_days: f32,
    /// Weight on the graph leg's RRF term (Task 7.3.7). `1.0` is the raw
    /// three-leg fusion; below `1.0` lets the graph *add* recall without
    /// *displacing* a memory the vector or keyword leg already ranked well.
    /// The graph brought relational recall from 71.4% to 85.7%, but at full
    /// weight it also let a graph-boosted "bridge" memory outrank the direct
    /// answer on one case, nudging precision@1 down; this is the dial that
    /// buys the precision back.
    graph_rank_weight: f32,
    /// The rank an *unanchored* graph hit — one neither the vector nor the
    /// keyword leg found — is capped at (Task 7.3.7). Its RRF term may not
    /// exceed `1/(k + floor)`, so a memory reached only by a relation hop
    /// cannot leapfrog a direct match ranked at or above `floor`, while
    /// still ranking ahead of weaker direct matches (staying in the top-k
    /// that recall is measured over). `0` disables the cap.
    graph_unanchored_floor: usize,
    /// How the rerank stage reorders the fused list (reranking-plan.md
    /// Phase 4.3). Inert while no rerank scores are supplied — which is
    /// every path until a reranker is wired — so the knob alone changes
    /// nothing today.
    rerank_strategy: RerankStrategy,
    /// How the legs' raw signals are read as relevance. Used only for the
    /// `relevance` field — the ranking above does not consult it, which is
    /// what keeps this knob from being able to reorder anything.
    relevance: RelevanceCalibration,
}

impl RecallRanker {
    pub fn new(recency_half_life_days: u32) -> Self {
        Self {
            // A zero half-life would divide by zero; treat it as "no decay".
            recency_half_life_days: if recency_half_life_days == 0 {
                f32::INFINITY
            } else {
                recency_half_life_days as f32
            },
            graph_rank_weight: DEFAULT_GRAPH_RANK_WEIGHT,
            graph_unanchored_floor: DEFAULT_GRAPH_UNANCHORED_FLOOR,
            rerank_strategy: RerankStrategy::default(),
            relevance: RelevanceCalibration::default(),
        }
    }

    /// Sets how the legs' raw signals are read as relevance.
    ///
    /// Defaults to a no-op for ranking: this field is consulted only when
    /// computing [`ScoredMemory::relevance`], never when ordering, so a
    /// recalibration cannot quietly change which memory comes first.
    pub fn with_relevance_calibration(mut self, relevance: RelevanceCalibration) -> Self {
        self.relevance = relevance;
        self
    }

    /// How relevant a result is, given what each leg measured about it.
    ///
    /// Split out from [`rank`](Self::rank) because the two answer different
    /// questions and only one of them is allowed to decide the order. See
    /// [`RelevanceCalibration`].
    pub fn relevance(&self, vector: Option<f32>, bm25: f32, graph_only: bool) -> f32 {
        self.relevance.relevance(vector, bm25, graph_only)
    }

    /// Sets the graph-leg ranking knobs (Task 7.3.7 — precision recovery).
    ///
    /// Both default to a no-op (`weight = 1.0`, `floor = 0`), so this only
    /// changes behaviour when wiring passes the configured `[graph]` values.
    /// It touches nothing about the vector or keyword legs: a query that
    /// produces no graph hits ranks identically regardless of either knob,
    /// which is what keeps the empty-graph-leg identity guarantee intact.
    pub fn with_graph_ranking(mut self, rank_weight: f32, unanchored_floor: usize) -> Self {
        self.graph_rank_weight = rank_weight;
        self.graph_unanchored_floor = unanchored_floor;
        self
    }

    /// Sets how the rerank stage reorders the fused list, once scores are
    /// supplied to [`rank_with_legs`](Self::rank_with_legs) or
    /// [`apply_rerank`](Self::apply_rerank). Defaults to
    /// [`RerankStrategy::default()`] — a full-weight fourth RRF leg. The
    /// module docs carry the calibration argument for the weight range and
    /// for the band's interaction with `Replace`.
    pub fn with_rerank_strategy(mut self, strategy: RerankStrategy) -> Self {
        self.rerank_strategy = strategy;
        self
    }

    /// Fuses the retrieval legs and returns memories best-first, then
    /// applies the rerank stage (reranking-plan.md Phase 4.3). This is
    /// the general entry point; [`rank`](Self::rank) and
    /// [`rank_with_graph`](Self::rank_with_graph) are thin wrappers over
    /// it that pass no rerank scores — an empty rerank list, which
    /// [`apply_rerank`](Self::apply_rerank) ignores outright, leaving
    /// the pre-rerank behaviour byte-for-byte intact.
    ///
    /// `rerank` is the reranker's output reduced to a *ranked list*: ids
    /// best-first, each with its raw score, over (a subset of) the
    /// candidates. Sorting it is the caller's job — it belongs to the
    /// model's scale, and the ranker's contract is to rank candidates,
    /// not to re-adjudicate the ranker's own ordering.
    // *not* used by the recaller — it fuses and then applies the rerank
    // leg in two steps (the floor runs between them), so this stays the
    // one-call entry point pinned by the ranker tests rather than the
    // production path.
    #[allow(dead_code)]
    pub fn rank_with_legs(
        &self,
        vector: &RankedIds,
        keyword: &RankedIds,
        graph: &RankedIds,
        rerank: &[(MemoryId, f32)],
        candidates: Vec<Memory>,
        now: DateTime<Utc>,
    ) -> Vec<ScoredMemory> {
        let mut scored = self.rank_with_graph(vector, keyword, graph, candidates, now);
        self.apply_rerank(&mut scored, rerank, now);
        scored
    }

    /// Fuses the vector and keyword legs and returns memories best-first.
    ///
    /// `candidates` supplies the memory for every id either leg
    /// mentioned; ids without one are dropped (a row deleted between the
    /// index hit and the fetch).
    ///
    /// This is the two-leg call the graph leg must not disturb: it is
    /// exactly [`rank_with_graph`](Self::rank_with_graph) over an empty
    /// graph leg, so a build with no graph, or a query that names no known
    /// entity, ranks byte-for-byte as it did before Task 7.3.4.
    pub fn rank(
        &self,
        vector: &RankedIds,
        keyword: &RankedIds,
        candidates: Vec<Memory>,
        now: DateTime<Utc>,
    ) -> Vec<ScoredMemory> {
        self.rank_with_graph(vector, keyword, &RankedIds::default(), candidates, now)
    }

    /// Fuses all three legs — vector, keyword and the graph hop — and
    /// returns memories best-first.
    ///
    /// The graph leg is just a third ranked list: it adds one more
    /// `1/(k + rank)` term on the same scale as the other two, so a memory
    /// several legs agree on still beats one only a single leg loves, and
    /// `RRF_K`/[`MULTIPLIER_FLOOR`] are untouched — this adds a leg, it does
    /// not retune the bands. An empty graph leg contributes nothing and
    /// leaves the two-leg ranking identical.
    pub fn rank_with_graph(
        &self,
        vector: &RankedIds,
        keyword: &RankedIds,
        graph: &RankedIds,
        candidates: Vec<Memory>,
        now: DateTime<Utc>,
    ) -> Vec<ScoredMemory> {
        let mut scored: Vec<ScoredMemory> = candidates
            .into_iter()
            .map(|memory| {
                let detail = MatchDetail {
                    vector_rank: rank_of(vector, memory.id()),
                    bm25_rank: rank_of(keyword, memory.id()),
                    graph_rank: rank_of(graph, memory.id()),
                    // Set by the rerank stage alone: an id the model never
                    // scored, and a ranker that never ran, are the same fact
                    // to the reader, and both mean "no second stage here".
                    rerank_rank: None,
                };
                let score = self.score(&memory, detail, now);
                ScoredMemory {
                    memory,
                    score,
                    // Computed by whoever supplied the leg scores — see the
                    // field docs. Zero rather than a copy of `score`: a
                    // rank position is not a relevance measurement, and
                    // silently passing one off as the other is exactly the
                    // confusion this field exists to remove.
                    relevance: 0.0,
                    rerank_score: None,
                    match_detail: detail,
                }
            })
            .filter(|scored| {
                scored.match_detail.vector_rank.is_some()
                    || scored.match_detail.bm25_rank.is_some()
                    || scored.match_detail.graph_rank.is_some()
            })
            .collect();

        sort_ranked(&mut scored);

        scored
    }

    /// Applies the rerank stage to an already-fused list
    /// (reranking-plan.md Phase 4.3/4.4).
    ///
    /// `rerank` is the same best-first `(id, raw score)` list
    /// [`rank_with_legs`](Self::rank_with_legs) takes; it is expected to
    /// cover the head of `ranked`, though any ids it names are honoured
    /// wherever they sit. Empty — the reranker absent, off, failed, or
    /// past its deadline — is a *hard no-op*: the list is returned
    /// untouched, not re-sorted or re-validated, which is what makes the
    /// "no reranker, no change" guarantee a code path rather than an
    /// arithmetic coincidence.
    ///
    /// Under both strategies the bounded recency/quality band survives:
    /// `Rrf` recomputes the sum inside the same band, and `Replace`
    /// multiplies the raw score by that same band, so the rerank stage
    /// adds a signal without opening a multiplier of its own.
    pub fn apply_rerank(
        &self,
        ranked: &mut Vec<ScoredMemory>,
        rerank: &[(MemoryId, f32)],
        now: DateTime<Utc>,
    ) {
        if rerank.is_empty() {
            return;
        }

        match self.rerank_strategy {
            RerankStrategy::Rrf { weight } => {
                for (index, (id, raw)) in rerank.iter().enumerate() {
                    let Some(scored) = ranked.iter_mut().find(|scored| scored.memory.id() == *id)
                    else {
                        continue;
                    };
                    let detail = scored.match_detail;
                    scored.match_detail.rerank_rank = Some(index + 1);
                    scored.rerank_score = Some(*raw);

                    // The stored score is `(leg sum) × band`; rebuild the
                    // sum from the recorded ranks to recover that band and
                    // add the new leg *inside* it, exactly where the other
                    // legs' terms sit. A degenerate sum (a memory carried
                    // only by a zero-weighted graph term) has no band to
                    // recover; leave its score alone rather than divide.
                    let base = reciprocal_rank(detail.vector_rank)
                        + reciprocal_rank(detail.bm25_rank)
                        + self.graph_relevance(detail);
                    if base > 0.0 {
                        let band = scored.score / base;
                        scored.score = (base + weight * reciprocal_rank(Some(index + 1))) * band;
                    }
                }
                sort_ranked(ranked);
            }
            RerankStrategy::Replace => {
                let all = std::mem::take(ranked);
                let mut head = Vec::new();
                let mut tail = Vec::new();
                for mut scored in all {
                    match rerank.iter().position(|(id, _)| *id == scored.memory.id()) {
                        Some(index) => {
                            let raw = rerank[index].1;
                            scored.match_detail.rerank_rank = Some(index + 1);
                            scored.rerank_score = Some(raw);
                            // The band the whole module calibrates, applied
                            // to the model's own number: ties and gaps
                            // under 1.25× are for recency and quality,
                            // anything wider is the reranker's call.
                            scored.score = raw
                                * self.recency_multiplier(scored.memory.created_at(), now)
                                * quality(&scored.memory);
                            head.push(scored);
                        }
                        // The tail keeps its fused relative order — the
                        // reranker never judged it, and `Replace` claims
                        // only to reorder the head.
                        None => tail.push(scored),
                    }
                }
                // Stable sort: an exact tie on the banded score keeps the
                // pre-rerank fusion order, which was already deterministic.
                head.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                ranked.extend(head.into_iter().chain(tail));
            }
        }
    }

    fn score(&self, memory: &Memory, detail: MatchDetail, now: DateTime<Utc>) -> f32 {
        let relevance = reciprocal_rank(detail.vector_rank)
            + reciprocal_rank(detail.bm25_rank)
            + self.graph_relevance(detail);

        relevance * self.recency_multiplier(memory.created_at(), now) * quality(memory)
    }

    /// The graph leg's contribution to relevance, after the two 7.3.7
    /// precision knobs. Zero when the graph did not reach this memory —
    /// which is what makes every downstream ranking identical when the
    /// graph leg is empty, whatever the knobs are set to.
    fn graph_relevance(&self, detail: MatchDetail) -> f32 {
        let Some(rank) = detail.graph_rank else {
            return 0.0;
        };

        // Option 1 — weighted RRF: scale the graph term down so it rides
        // alongside the other two legs without overpowering them.
        let mut term = self.graph_rank_weight * reciprocal_rank(Some(rank));

        // Option 2 — unanchored floor: a memory *only* the graph found has
        // no vector or keyword rank to stand on, so cap its term at the
        // floor rank's reciprocal. It can still beat weaker direct matches
        // (and reach the top-k), but not a strong one.
        let unanchored = detail.vector_rank.is_none() && detail.bm25_rank.is_none();
        if unanchored && self.graph_unanchored_floor > 0 {
            term = term.min(reciprocal_rank(Some(self.graph_unanchored_floor)));
        }

        term
    }

    fn recency_multiplier(&self, created_at: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
        let age_days = (now - created_at).num_seconds() as f32 / 86_400.0;
        // A memory created "in the future" (clock skew) gets no bonus.
        let age_days = age_days.max(0.0);

        let decay = 0.5f32.powf(age_days / self.recency_half_life_days);
        floored(decay)
    }
}

/// The canonical result ordering: best score first, and ties broken by
/// recency then id so the same inputs always produce the same list —
/// pagination depends on it, whether the tie came from the fusion or from
/// the rerank stage re-sorting it.
fn sort_ranked(scored: &mut [ScoredMemory]) {
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Ties must not depend on the order rows came back from
            // SQLite, or the same query would paginate differently
            // between calls. Newest first, then id.
            .then_with(|| b.memory.created_at().cmp(&a.memory.created_at()))
            .then_with(|| a.memory.id().to_string().cmp(&b.memory.id().to_string()))
    });
}

fn reciprocal_rank(rank: Option<usize>) -> f32 {
    match rank {
        Some(rank) => 1.0 / (RRF_K + rank as f32),
        None => 0.0,
    }
}

/// The BM25 weight at which the keyword leg is considered to have fully
/// matched.
///
/// BM25 is unbounded and its scale moves with the corpus, so it cannot be
/// read as a probability directly. Saturating it with `x / (x + k)` maps
/// `0..∞` onto `0..1` monotonically, which is all the fusion needs, and
/// leaves the *ordering within* the leg exactly as it was. `k = 8` is picked
/// from what tantivy actually produces here over short memories: a single
/// distinctive term lands near `k`, so it reads as "half relevant", while a
/// query whose rare terms all appear pushes past it.
const BM25_SATURATION: f32 = 8.0;

/// The relevance floor an agent-facing read applies unless configured
/// otherwise.
///
/// Lives here rather than in the config defaults because the domain owns the
/// scale it is a threshold on: the two have to move together, and a test
/// below asserts that they still do against measured numbers.
pub const DEFAULT_RELEVANCE_FLOOR: f32 = 0.25;

/// The relevance credited to a memory that entered recall *only* through a
/// graph hop.
///
/// Neither leg measured it — the hop reached it over a relation from a
/// memory the query did match — so there is no similarity to report, only
/// the strength of that indirect evidence. It has to sit above whatever
/// floor is configured, or the floor would silently delete relational
/// recall, which is the one thing the graph leg exists to add.
pub const GRAPH_HOP_RELEVANCE: f32 = 0.35;

/// Turns the two legs' raw signals into an absolute relevance in `0.0..=1.0`.
///
/// # Why the vector leg needs a baseline
///
/// Cosine similarity from a sentence-embedding model does not start at zero.
/// Two *unrelated* English sentences routinely measure 0.4–0.55, because
/// they share the same bag of function words, the same document-level
/// statistics, and a great deal of the model's representational space. Only
/// the top of the range means anything: measured against the default model,
/// a genuinely unrelated query topped out at 0.54 while real answers started
/// at 0.68.
///
/// Read raw, then, "0.5 relevant" is noise and "0.6 relevant" is a good
/// answer, which is not a scale anyone can set a threshold on or read without
/// a histogram beside them. Subtracting the baseline rescales so that
/// *no-evidence* lands at zero:
///
/// ```text
/// corrected = (similarity - baseline) / (1 - baseline)
/// ```
///
/// That turns the measured spread into noise ≤ 0.09 and real matches
/// 0.36–0.82 — a gap a floor can be placed inside with room on both sides.
///
/// # Why it is a constant rather than something measured per query
///
/// Because it belongs to the *model*, not to the query or the corpus: it is a
/// property of how the embedding space is laid out. Measuring it at query
/// time would make relevance depend on what else was in the result set, which
/// reintroduces exactly the rank-relative meaning this field exists to
/// remove.
///
/// It is configurable because it is not universal: a different embedding
/// model has a different baseline, and one that adds a retrieval instruction
/// to queries (`bge` models expect one) will shift it again. Because it
/// belongs to the model, the shipping default is looked up per model
/// ([`SIMILARITY_BASELINES`]) rather than hardcoded to one model's number;
/// a deployment that changes `[embeddings].model` and finds recall too
/// strict or too loose should still look here before anywhere else.
#[derive(Debug, Clone, Copy)]
pub struct RelevanceCalibration {
    /// The cosine at which the vector leg is treated as saying nothing.
    pub similarity_baseline: f32,
}

impl Default for RelevanceCalibration {
    fn default() -> Self {
        Self {
            similarity_baseline: DEFAULT_SIMILARITY_BASELINE,
        }
    }
}

/// The fallback baseline: used for a model that is not in
/// [`SIMILARITY_BASELINES`], and the historical default — measured
/// against `bge-small-en-v1.5`, which the map pins at the same number.
///
/// See [`RelevanceCalibration`] for what the number means; see the map
/// for why one constant cannot serve every model.
pub const DEFAULT_SIMILARITY_BASELINE: f32 = 0.5;

/// The measured no-evidence baseline per embedding model, as
/// `(model_id, cosine)`, matched against [`Embedder::model_id`].
///
/// # Why a map rather than one constant
///
/// The baseline is a property of the *model* — how its embedding space
/// spreads unrelated text — not of the query or the corpus (see the
/// [`RelevanceCalibration`] docs). Measured once with the same method the
/// `MEASURED_NOISE_CEILING` test constants pin for bge-small (the maximum
/// raw cosine of the eval set's `negative` queries against the corpus, and
/// the minimum raw cosine of each case's expected memory), it differs by
/// more than `0.15` between the two shipped models: what reads as noise
/// in Gemini space would be subtracting away most of a real answer's
/// relevance under bge-small's constant, and vice versa — noise would
/// sail past the floor. So `[retrieval].similarity_baseline` gained per-model
/// defaults (reranking-plan.md Phase 2.2); an explicit config value
/// still wins over the map.
///
/// The rows, measured by `recuerdos-ai calibrate` over `eval/cases.toml`
/// (reranking-plan.md Phase 2.1/2.2):
///
/// - `bge-small-en-v1.5` → `0.5`, the historical constant. The larger
///   eval corpus puts this model's noise ceiling at `0.599` — above the
///   `0.542` the constant was originally measured against (a smaller
///   working set; the `MEASURED_*` tests below pin that older
///   measurement, which the shipped floor still agrees with). The
///   corpus shows an infeasible gap here too (floor-gated answers down
///   to `0.528`), but `0.5` stands: it is every existing pin's
///   calibration, and changing the default model's number on a thin
///   eval corpus is noisier than keeping it.
/// - `gemini-embedding-2` → `0.46`, measured noise ceiling `0.580` (two
///   `negative` queries reach it). Suppressing that noise under the
///   shipped `0.25` floor requires `> 0.44`; answers down to raw
///   `0.611` survive it while `b ≤ 0.481`, so `0.46` sits mid-window:
///   noise corrects to `0.222`, a `0.611` answer to `0.280`.
///
/// What the measurement *also* says, and `0.46` cannot fix: two
/// floor-gated answers sit **below** the noise ceiling — raw `0.562` for
/// *"health, fitness or family"*, and `0.572` for the already-
/// recall-failing dietary needle (reranking-plan.md §12, Phase 2). No
/// baseline can both suppress every negative case and keep those answers
/// above the floor; `0.46` chooses the negatives' side. Those weak
/// answers then land below the floor — which is where §1's third row
/// really lives too (there they were noise-*inflated* to `0.33` and
/// ranked #5; now they are honestly sub-floor instead). The thin gemini
/// gap is itself one of the numbers Phase 3 weighs. (Earlier live-store
/// relevances of `0.84–0.91` under this model were BM25-inflated noise,
/// not raw cosine — §12.)
///
/// Re-measure and add a row when adding a model row to the embedder,
/// with `recuerdos-ai calibrate --config <that config>`; an unknown
/// model falls back to [`DEFAULT_SIMILARITY_BASELINE`] with a `warn!` at
/// startup, which is deliberately loud because every relevance threshold
/// in the system silently shifts if this number is wrong.
pub const SIMILARITY_BASELINES: &[(&str, f32)] =
    &[("bge-small-en-v1.5", 0.5), ("gemini-embedding-2", 0.46)];

/// The measured baseline for `model_id`, or `None` when the model has
/// never been calibrated (the caller falls back to
/// [`DEFAULT_SIMILARITY_BASELINE`] and warns).
pub fn default_similarity_baseline_for(model_id: &str) -> Option<f32> {
    SIMILARITY_BASELINES
        .iter()
        .find(|(calibrated, _)| *calibrated == model_id)
        .map(|(_, baseline)| *baseline)
}

impl RelevanceCalibration {
    /// Calibration at a given vector baseline. `0.0` reports raw cosine.
    pub fn new(similarity_baseline: f32) -> Self {
        Self {
            similarity_baseline,
        }
    }

    /// Fuses the legs into `0.0..=1.0`.
    ///
    /// `vector_similarity` is the leg's cosine in `0.0..=1.0`; `bm25` is its
    /// raw, unbounded weight. They are combined with a noisy-OR —
    /// `1 - (1 - v)·(1 - b)` — rather than averaged, because they are
    /// independent evidence for the same claim: a memory both legs love
    /// should be more relevant than either alone suggests, and a memory only
    /// one leg loves keeps that leg's opinion at full strength instead of
    /// having it halved by the other's silence. Averaging would cap a
    /// vector-perfect paraphrase at 0.5 purely because it shares no words
    /// with the query, which is the exact case hybrid search is for.
    pub fn relevance(&self, vector_similarity: Option<f32>, bm25: f32, graph_only: bool) -> f32 {
        let bm25 = bm25.max(0.0);
        let keyword = bm25 / (bm25 + BM25_SATURATION);

        let combined = match vector_similarity {
            Some(similarity) => {
                let similarity = self.correct(similarity);
                1.0 - (1.0 - similarity) * (1.0 - keyword)
            }
            None => keyword,
        };

        // A hop is weak evidence, but it is *the* evidence for a memory
        // nothing else found, so it raises a zero rather than competing with
        // a real measurement.
        if combined > 0.0 {
            combined
        } else if graph_only {
            GRAPH_HOP_RELEVANCE
        } else {
            0.0
        }
    }

    /// Subtracts the no-evidence baseline and stretches the remainder back
    /// over `0.0..=1.0`.
    fn correct(&self, similarity: f32) -> f32 {
        let baseline = self.similarity_baseline.clamp(0.0, 0.99);
        ((similarity - baseline) / (1.0 - baseline)).clamp(0.0, 1.0)
    }
}

/// How much a memory is worth regardless of the query: how sure we are
/// of it, and how much it is actually used. One band for both — see the
/// module docs.
fn quality(memory: &Memory) -> f32 {
    floored(memory.confidence() * memory.importance())
}

/// Maps `0.0..=1.0` onto `MULTIPLIER_FLOOR..=1.0`.
fn floored(value: f32) -> f32 {
    MULTIPLIER_FLOOR + (1.0 - MULTIPLIER_FLOOR) * value.clamp(0.0, 1.0)
}

fn rank_of(list: &RankedIds, id: MemoryId) -> Option<usize> {
    list.0
        .iter()
        .position(|candidate| *candidate == id)
        .map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory::{MemorySource, NewMemory};
    use crate::shared::ids::UserId;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn memory_aged(days: i64, confidence: f32) -> Memory {
        Memory::create(
            UserId::new(),
            NewMemory {
                content: "x".to_string(),
                category: Category::PreferenceCoding,
                subcategory: None,
                tags: vec![],
                entities: vec![],
                confidence,
                source: MemorySource::default(),
                expires_at: None,
            },
            now() - chrono::Duration::days(days),
        )
        .unwrap()
    }

    fn memory() -> Memory {
        memory_aged(0, 1.0)
    }

    fn ranker() -> RecallRanker {
        RecallRanker::new(90)
    }

    fn ids(memories: &[&Memory]) -> RankedIds {
        RankedIds(memories.iter().map(|m| m.id()).collect())
    }

    fn ordered(scored: &[ScoredMemory]) -> Vec<MemoryId> {
        scored.iter().map(|s| s.memory.id()).collect()
    }

    #[test]
    fn a_decayed_memory_loses_a_tie_to_one_in_active_use() {
        // What importance decay buys: two equally relevant memories,
        // and the one people actually read comes first.
        let used = memory().with_importance(1.0);
        let ignored = memory().with_importance(crate::consolidation::domain::decay::MIN_IMPORTANCE);

        let result = ranker().rank(
            &ids(&[&ignored, &used]),
            &ids(&[&ignored, &used]),
            vec![used.clone(), ignored.clone()],
            now(),
        );

        assert_eq!(
            ordered(&result),
            [used.id(), ignored.id()],
            "the decayed memory outranked the one in active use"
        );
    }

    #[test]
    fn adding_decay_did_not_widen_the_multiplier_band() {
        // The load-bearing property. Each floored band spans 1.25x and
        // they compound; the bands are calibrated against RRF's rank
        // spacing, so an extra one silently changes which memory wins
        // every close call in the system. Confidence and importance
        // therefore share a band: the worst case a memory can suffer for
        // being uncertain *and* unused is the same MULTIPLIER_FLOOR it
        // already suffered for being uncertain alone.
        let best = memory_aged(0, 1.0).with_importance(1.0);
        let worst_confidence = memory_aged(0, 0.0).with_importance(1.0);
        let worst_both = memory_aged(0, 0.0).with_importance(0.0);

        assert!((quality(&best) - 1.0).abs() < 1e-6);
        assert!((quality(&worst_confidence) - MULTIPLIER_FLOOR).abs() < 1e-6);
        assert!(
            (quality(&worst_both) - MULTIPLIER_FLOOR).abs() < 1e-6,
            "decay opened a second band: quality fell to {}, below the floor {MULTIPLIER_FLOOR}",
            quality(&worst_both)
        );
    }

    #[test]
    fn decay_reorders_neighbours_without_leapfrogging_many_ranks() {
        // The band is meant to break ties, not to re-sort the corpus.
        // A decayed memory one rank better loses; a decayed memory many
        // ranks better still wins.
        let decayed = memory().with_importance(0.0);
        let used = memory().with_importance(1.0);

        let adjacent = ranker().rank(
            &ids(&[&decayed, &used]),
            &ids(&[&decayed, &used]),
            vec![decayed.clone(), used.clone()],
            now(),
        );
        assert_eq!(
            ordered(&adjacent)[0],
            used.id(),
            "decay should break a near-tie"
        );

        // Now put twenty places between them.
        let filler: Vec<Memory> = (0..19).map(|_| memory()).collect();
        let mut ranking = vec![&decayed];
        ranking.extend(filler.iter());
        ranking.push(&used);
        let mut candidates = vec![decayed.clone(), used.clone()];
        candidates.extend(filler.iter().cloned());

        // Compared to each other rather than to the whole list: the
        // filler sits one rank behind the decayed memory, and a 1.25x
        // band does swamp a single rank — that is the band doing its job.
        // What it must not do is swamp twenty.
        let distant = ordered(&ranker().rank(&ids(&ranking), &ids(&ranking), candidates, now()));
        let place = |id| distant.iter().position(|found| *found == id).unwrap();

        assert!(
            place(decayed.id()) < place(used.id()),
            "decay leapfrogged twenty ranks and buried the better match"
        );
    }

    #[test]
    fn a_memory_nothing_has_measured_yet_ranks_as_it_always_did() {
        // Importance defaults to 1.0, so a memory saved between two
        // nightly runs is not penalised for not having been scored.
        assert_eq!(memory().importance(), 1.0);
    }

    #[test]
    fn a_memory_both_legs_rank_beats_one_only_either_leg_ranks() {
        let both = memory();
        let vector_only = memory();
        let keyword_only = memory();

        let result = ranker().rank(
            &ids(&[&vector_only, &both]),
            &ids(&[&keyword_only, &both]),
            vec![both.clone(), vector_only.clone(), keyword_only.clone()],
            now(),
        );

        assert_eq!(
            ordered(&result)[0],
            both.id(),
            "agreement between the legs should win"
        );
    }

    #[test]
    fn a_better_rank_in_one_leg_wins_when_the_other_leg_is_silent() {
        let first = memory();
        let second = memory();

        let result = ranker().rank(
            &ids(&[&first, &second]),
            &RankedIds::default(),
            vec![second.clone(), first.clone()],
            now(),
        );

        assert_eq!(ordered(&result), vec![first.id(), second.id()]);
    }

    #[test]
    fn memories_in_neither_leg_are_dropped() {
        let found = memory();
        let unrelated = memory();

        let result = ranker().rank(
            &ids(&[&found]),
            &RankedIds::default(),
            vec![found.clone(), unrelated.clone()],
            now(),
        );

        assert_eq!(ordered(&result), vec![found.id()]);
    }

    #[test]
    fn match_detail_reports_both_ranks() {
        let a = memory();
        let b = memory();

        let result = ranker().rank(
            &ids(&[&a, &b]),
            &ids(&[&b, &a]),
            vec![a.clone(), b.clone()],
            now(),
        );

        let for_a = result.iter().find(|s| s.memory.id() == a.id()).unwrap();
        assert_eq!(for_a.match_detail.vector_rank, Some(1));
        assert_eq!(for_a.match_detail.bm25_rank, Some(2));
    }

    #[test]
    fn match_detail_reports_a_missing_leg_as_none() {
        let only_vector = memory();

        let result = ranker().rank(
            &ids(&[&only_vector]),
            &RankedIds::default(),
            vec![only_vector.clone()],
            now(),
        );

        assert_eq!(result[0].match_detail.vector_rank, Some(1));
        assert_eq!(result[0].match_detail.bm25_rank, None);
    }

    #[test]
    fn an_empty_graph_leg_leaves_the_two_leg_ranking_identical() {
        // The load-bearing guarantee of Task 7.3.4: adding the graph leg
        // cannot regress non-relational recall. With no graph hits,
        // `rank_with_graph` must return the same order *and* the same
        // scores as the two-leg `rank` — which is why `rank` delegates to
        // it over an empty leg rather than keeping a parallel body.
        let a = memory_aged(0, 1.0);
        let b = memory_aged(30, 0.5);
        let c = memory_aged(365, 0.9);
        let vector = ids(&[&a, &b, &c]);
        let keyword = ids(&[&c, &a]);
        let candidates = || vec![a.clone(), b.clone(), c.clone()];

        let two_leg = ranker().rank(&vector, &keyword, candidates(), now());
        let three_leg = ranker().rank_with_graph(
            &vector,
            &keyword,
            &RankedIds::default(),
            candidates(),
            now(),
        );

        assert_eq!(
            ordered(&two_leg),
            ordered(&three_leg),
            "an empty graph leg reordered the results"
        );
        for (two, three) in two_leg.iter().zip(three_leg.iter()) {
            assert_eq!(two.score, three.score, "an empty graph leg changed a score");
            assert_eq!(three.match_detail.graph_rank, None);
        }
    }

    #[test]
    fn a_memory_only_the_graph_leg_found_is_kept_and_scored() {
        // The reason the leg exists: a memory neither the vector nor the
        // keyword leg ranks, reachable only over a relation, still earns a
        // place and a positive score.
        let hopped = memory();

        let result = ranker().rank_with_graph(
            &RankedIds::default(),
            &RankedIds::default(),
            &ids(&[&hopped]),
            vec![hopped.clone()],
            now(),
        );

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].match_detail.graph_rank, Some(1));
        assert!(result[0].score > 0.0);
    }

    fn score_of(result: &[ScoredMemory], id: MemoryId) -> f32 {
        result.iter().find(|s| s.memory.id() == id).unwrap().score
    }

    #[test]
    fn the_graph_weight_scales_the_leg_but_never_the_others() {
        // Option 1 (Task 7.3.7). A memory only the graph reached is scored
        // purely from the graph term, so halving the weight halves its
        // score — the dial that lets a hop add recall without shouting down
        // the vector and keyword legs. `new`'s default weight is 1.0, so the
        // graph leg is untouched unless wiring configures it.
        let hopped = memory();
        let graph = ids(&[&hopped]);
        let candidates = || vec![hopped.clone()];

        let full = RecallRanker::new(90) // weight 1.0
            .rank_with_graph(
                &RankedIds::default(),
                &RankedIds::default(),
                &graph,
                candidates(),
                now(),
            );
        let half = RecallRanker::new(90)
            .with_graph_ranking(0.5, 0)
            .rank_with_graph(
                &RankedIds::default(),
                &RankedIds::default(),
                &graph,
                candidates(),
                now(),
            );

        assert!(
            (score_of(&half, hopped.id()) - 0.5 * score_of(&full, hopped.id())).abs() < 1e-6,
            "halving the graph weight did not halve a graph-only memory's score"
        );
    }

    #[test]
    fn down_weighting_the_graph_leg_returns_a_direct_match_to_the_top() {
        // Option 1, at the ordering it exists to fix. `bridge` is reached by
        // a hop *and* ranked (weakly) by the keyword leg — two terms — so at
        // full weight it outscores `answer`, a memory the keyword leg ranked
        // first with no hop. That is the reorder the graph cost precision@1
        // for. Shrinking the hop's contribution returns the direct answer to
        // the top, and `bridge` is still returned — precision, not recall.
        let answer = memory();
        let bridge = memory();

        // `answer` is keyword rank 1; `bridge` is keyword rank 10 (the eight
        // fillers stand in for stronger keyword matches ahead of it) plus
        // graph rank 1. At rank 10 the keyword term alone can't catch rank 1,
        // so the hop is what puts `bridge` over the top at full weight.
        let mut keyword = vec![answer.id()];
        keyword.extend(std::iter::repeat_with(MemoryId::new).take(8));
        keyword.push(bridge.id());
        let keyword = RankedIds(keyword);
        let graph = ids(&[&bridge]);
        let candidates = || vec![answer.clone(), bridge.clone()];

        let full = RecallRanker::new(90).rank_with_graph(
            &RankedIds::default(),
            &keyword,
            &graph,
            candidates(),
            now(),
        );
        assert_eq!(
            ordered(&full)[0],
            bridge.id(),
            "at full weight the bridge should lead"
        );

        let damped = RecallRanker::new(90)
            .with_graph_ranking(0.1, 0)
            .rank_with_graph(&RankedIds::default(), &keyword, &graph, candidates(), now());
        assert_eq!(
            ordered(&damped)[0],
            answer.id(),
            "down-weighting the graph leg should return the direct answer to the top"
        );
        assert!(
            damped.iter().any(|s| s.memory.id() == bridge.id()),
            "the bridge must still be returned — precision, not recall, is the lever"
        );
    }

    #[test]
    fn the_unanchored_floor_caps_a_graph_only_hit_below_a_strong_direct_match() {
        // Option 2 (Task 7.3.7), isolated at full weight. `unanchored` is
        // reached only by a hop (no vector or keyword rank); `direct` is a
        // top keyword hit. Without the floor a rank-1 hop ties a rank-1
        // keyword match; a floor of 3 caps the hop at a rank-3 term, so the
        // direct match wins — yet the hop is still returned, keeping the
        // top-k that recall is measured over.
        let direct = memory();
        let unanchored = memory();
        let keyword = ids(&[&direct]);
        let graph = ids(&[&unanchored]);
        let candidates = || vec![direct.clone(), unanchored.clone()];

        let no_floor = RecallRanker::new(90).rank_with_graph(
            &RankedIds::default(),
            &keyword,
            &graph,
            candidates(),
            now(),
        );
        assert!(
            (score_of(&no_floor, direct.id()) - score_of(&no_floor, unanchored.id())).abs() < 1e-6,
            "with no floor a rank-1 hop and a rank-1 keyword match should tie on score"
        );

        let floored = RecallRanker::new(90)
            .with_graph_ranking(1.0, 3)
            .rank_with_graph(&RankedIds::default(), &keyword, &graph, candidates(), now());
        assert!(
            score_of(&floored, direct.id()) > score_of(&floored, unanchored.id()),
            "the floor should let the direct match outrank the graph-only hit"
        );
        assert_eq!(ordered(&floored)[0], direct.id());
        assert!(
            floored.iter().any(|s| s.memory.id() == unanchored.id()),
            "the graph-only hit must still be returned"
        );
    }

    #[test]
    fn the_unanchored_floor_leaves_an_anchored_hop_untouched() {
        // The floor targets *only* memories the graph alone found. A memory
        // both the vector leg and a hop reached is anchored, so its graph
        // term is not capped — the cap must not punish agreement between
        // the legs.
        let anchored = memory();
        let leg = ids(&[&anchored]);
        let candidates = || vec![anchored.clone()];

        let without = RecallRanker::new(90).rank_with_graph(
            &leg,
            &RankedIds::default(),
            &leg,
            candidates(),
            now(),
        );
        let with = RecallRanker::new(90)
            .with_graph_ranking(1.0, 3)
            .rank_with_graph(&leg, &RankedIds::default(), &leg, candidates(), now());

        assert_eq!(
            score_of(&without, anchored.id()),
            score_of(&with, anchored.id()),
            "the floor changed the score of an anchored hop"
        );
    }

    #[test]
    fn recency_breaks_ties_between_equally_ranked_memories() {
        let fresh = memory_aged(0, 1.0);
        let old = memory_aged(365, 1.0);

        // Same rank in each leg — only age differs.
        let result = ranker().rank(
            &ids(&[&old, &fresh]),
            &ids(&[&fresh, &old]),
            vec![old.clone(), fresh.clone()],
            now(),
        );

        assert_eq!(ordered(&result)[0], fresh.id());
    }

    #[test]
    fn recency_never_erases_a_far_more_relevant_old_memory() {
        // The motivating case: a two-year-old architecture decision that
        // both legs rank *first* must not lose to a fresh note both legs
        // rank 20th. This is the invariant that pins MULTIPLIER_FLOOR —
        // at a 2x band, the fresh marginal result wins here.
        let old_and_relevant = memory_aged(730, 1.0);
        let fresh_and_marginal = memory_aged(0, 1.0);

        // Ranks 2..19 are other results we don't score here — they exist
        // only to push the fresh memory down to rank 20.
        let mut leg = vec![old_and_relevant.id()];
        leg.extend((0..18).map(|_| MemoryId::new()));
        leg.push(fresh_and_marginal.id());
        let leg = RankedIds(leg);

        let result = ranker().rank(
            &leg,
            &leg,
            vec![old_and_relevant.clone(), fresh_and_marginal.clone()],
            now(),
        );

        assert_eq!(
            ordered(&result)[0],
            old_and_relevant.id(),
            "age must not delete relevance"
        );
    }

    #[test]
    fn recency_may_reorder_near_adjacent_results() {
        // The flip side of the invariant above: when two memories are
        // effectively equally relevant (adjacent ranks, ~1.6% apart),
        // recency is *allowed* to decide. That is the tiebreak the
        // multiplier exists for.
        let old = memory_aged(730, 1.0);
        let fresh = memory_aged(0, 1.0);

        let result = ranker().rank(
            &ids(&[&old, &fresh]),
            &ids(&[&old, &fresh]),
            vec![old.clone(), fresh.clone()],
            now(),
        );

        assert_eq!(ordered(&result)[0], fresh.id());
    }

    #[test]
    fn the_recency_multiplier_is_bounded() {
        let ranker = ranker();

        assert_eq!(ranker.recency_multiplier(now(), now()), 1.0);
        assert!(
            ranker.recency_multiplier(now() - chrono::Duration::days(36_500), now())
                >= MULTIPLIER_FLOOR
        );
    }

    #[test]
    fn a_memory_at_one_half_life_sits_midway() {
        let ranker = ranker();
        let multiplier = ranker.recency_multiplier(now() - chrono::Duration::days(90), now());

        // floored(0.5) = 0.8 + 0.2*0.5 = 0.9
        assert!((multiplier - 0.9).abs() < 0.01, "got {multiplier}");
    }

    #[test]
    fn a_future_timestamp_gets_no_bonus() {
        let ranker = ranker();
        let skewed = ranker.recency_multiplier(now() + chrono::Duration::days(30), now());

        assert_eq!(skewed, 1.0, "clock skew must not out-rank everything");
    }

    #[test]
    fn a_zero_half_life_disables_decay_instead_of_dividing_by_zero() {
        let ranker = RecallRanker::new(0);
        let multiplier = ranker.recency_multiplier(now() - chrono::Duration::days(1_000), now());

        assert_eq!(multiplier, 1.0);
        assert!(multiplier.is_finite());
    }

    #[test]
    fn confidence_breaks_ties_between_equally_ranked_memories() {
        let confident = memory_aged(0, 1.0);
        let unsure = memory_aged(0, 0.1);

        let result = ranker().rank(
            &ids(&[&unsure, &confident]),
            &ids(&[&confident, &unsure]),
            vec![unsure.clone(), confident.clone()],
            now(),
        );

        assert_eq!(ordered(&result)[0], confident.id());
    }

    #[test]
    fn confidence_never_erases_a_memory() {
        let zero_confidence = memory_aged(0, 0.0);

        let result = ranker().rank(
            &ids(&[&zero_confidence]),
            &RankedIds::default(),
            vec![zero_confidence.clone()],
            now(),
        );

        assert_eq!(result.len(), 1);
        assert!(result[0].score > 0.0, "a memory must never score zero");
    }

    #[test]
    fn ties_are_broken_deterministically_not_by_input_order() {
        // Identical age, confidence and ranks: the only distinguishing
        // feature is the id. The same inputs in a different order must
        // produce the same output order, or pagination breaks.
        let a = memory_aged(0, 1.0);
        let b = memory_aged(0, 1.0);

        let forwards = ranker().rank(
            &ids(&[&a, &b]),
            &ids(&[&a, &b]),
            vec![a.clone(), b.clone()],
            now(),
        );
        let backwards = ranker().rank(
            &ids(&[&a, &b]),
            &ids(&[&a, &b]),
            vec![b.clone(), a.clone()],
            now(),
        );

        assert_eq!(ordered(&forwards), ordered(&backwards));
    }

    #[test]
    fn newer_wins_before_the_id_tiebreak() {
        let older = memory_aged(10, 1.0);
        let newer = memory_aged(0, 1.0);

        // Same rank in both legs; recency multiplier differs only
        // slightly, but the ordering must still be stable and sensible.
        let result = ranker().rank(
            &ids(&[&older, &newer]),
            &ids(&[&newer, &older]),
            vec![older.clone(), newer.clone()],
            now(),
        );

        assert_eq!(ordered(&result)[0], newer.id());
    }

    /// Calibration with the baseline switched off, for the tests that are
    /// about how the two legs combine rather than where the floor of the
    /// vector leg sits.
    fn raw() -> RelevanceCalibration {
        RelevanceCalibration::new(0.0)
    }

    #[test]
    fn relevance_is_bounded_between_zero_and_one() {
        // Everything that thresholds on relevance assumes this range,
        // including the configured floor.
        for calibration in [
            raw(),
            RelevanceCalibration::default(),
            RelevanceCalibration::new(0.9),
        ] {
            for vector in [-1.0, 0.0, 0.3, 1.0, 5.0] {
                for bm25 in [-2.0, 0.0, 1.0, 100.0, f32::MAX] {
                    let relevance = calibration.relevance(Some(vector), bm25, false);
                    assert!(
                        (0.0..=1.0).contains(&relevance) && relevance.is_finite(),
                        "combine({vector}, {bm25}) escaped the range: {relevance}"
                    );
                }
            }
        }
    }

    #[test]
    fn relevance_rises_with_evidence_from_either_leg() {
        let mut previous = -1.0;
        for similarity in [0.0, 0.2, 0.4, 0.6, 0.8, 1.0] {
            let relevance = raw().relevance(Some(similarity), 0.0, false);
            assert!(
                relevance > previous,
                "a better cosine {similarity} did not raise relevance ({relevance} vs {previous})"
            );
            previous = relevance;
        }

        let mut previous = -1.0;
        for bm25 in [0.0, 2.0, 8.0, 20.0, 100.0] {
            let relevance = raw().relevance(None, bm25, false);
            assert!(
                relevance > previous,
                "a stronger BM25 {bm25} did not raise relevance ({relevance} vs {previous})"
            );
            previous = relevance;
        }
    }

    #[test]
    fn both_legs_agreeing_beats_either_alone() {
        // Why noisy-OR rather than an average: averaging would let the
        // keyword leg's silence about a pure paraphrase halve a
        // vector-perfect match, which is the exact case hybrid exists for.
        let vector_only = raw().relevance(Some(0.9), 0.0, false);
        let keyword_only = raw().relevance(None, 20.0, false);
        let both = raw().relevance(Some(0.9), 20.0, false);

        assert!(both > vector_only, "{both} vs {vector_only}");
        assert!(both > keyword_only, "{both} vs {keyword_only}");
        assert!(
            both > (vector_only + keyword_only) / 2.0,
            "agreement should not be discounted to a mean"
        );
    }

    #[test]
    fn a_graph_hop_is_credited_without_erasing_a_real_measurement() {
        // The hop number is an estimate, so it must never outvote evidence:
        // it shows up only where nothing else measured anything.
        assert_eq!(
            raw().relevance(None, 0.0, true),
            GRAPH_HOP_RELEVANCE,
            "a hop with no other evidence should be credited the hop estimate"
        );
        assert!(
            raw().relevance(Some(0.9), 0.0, true) > GRAPH_HOP_RELEVANCE,
            "a measured match was overridden by the hop estimate"
        );
        assert_eq!(
            raw().relevance(Some(0.0), 0.0, false),
            0.0,
            "no evidence and no hop is not relevance"
        );
    }

    #[test]
    fn similarity_at_the_baseline_is_no_evidence_at_all() {
        // The whole point of the baseline: without it, "0.5 relevant" is
        // what a completely unrelated memory scores, and no floor can be
        // placed above that without cutting the real answers too.
        let calibration = RelevanceCalibration::new(0.5);

        assert_eq!(calibration.relevance(Some(0.5), 0.0, false), 0.0);
        assert_eq!(calibration.relevance(Some(0.3), 0.0, false), 0.0);
        assert_eq!(
            calibration.relevance(Some(0.3), 0.0, true),
            GRAPH_HOP_RELEVANCE,
            "a sub-baseline similarity is no evidence, so the hop estimate still applies"
        );
        assert_eq!(raw().relevance(Some(0.5), 0.0, false), 0.5);
    }

    /// Lookup on the map (reranking-plan.md Phase 2.2): the default
    /// model's entry must equal the fallback constant, so the fallback
    /// stays exactly the pre-map behaviour for that model.
    #[test]
    fn the_baseline_map_holds_the_default_model_at_the_default_constant() {
        assert_eq!(
            default_similarity_baseline_for("bge-small-en-v1.5"),
            Some(DEFAULT_SIMILARITY_BASELINE)
        );
    }

    /// A model nobody measured has *no* entry — the absence is what the
    /// startup `warn!` in the wiring keys on. Inventing "close enough"
    /// defaults for unmeasured models is how miscalibration gets shipped.
    #[test]
    fn an_uncalibrated_model_is_absent_from_the_map() {
        assert_eq!(default_similarity_baseline_for("text-embedding-004"), None);
        assert_eq!(default_similarity_baseline_for(""), None);
        // Not a prefix/fuzzy match: the map is exact ids only.
        assert_eq!(default_similarity_baseline_for("bge-small"), None);
    }

    /// The numbers that set the default calibration, measured against the
    /// real default model over a small working set. Every genuinely relevant
    /// answer scored at or above the low end; every unrelated query —
    /// including nonsense like "qqq xyzzy 12345 bloop" — topped out at the
    /// high end. Pinned here so that a future change to the fusion, or a
    /// silent change to the model's behaviour, fails a test instead of
    /// quietly shifting every threshold in the system.
    const MEASURED_NOISE_CEILING: f32 = 0.542;
    const MEASURED_ANSWER_FLOOR: f32 = 0.681;

    #[test]
    fn the_default_calibration_separates_measured_noise_from_measured_answers() {
        let calibration = RelevanceCalibration::default();

        let noise = calibration.relevance(Some(MEASURED_NOISE_CEILING), 0.0, false);
        let answer = calibration.relevance(Some(MEASURED_ANSWER_FLOOR), 0.0, false);

        assert!(
            noise < DEFAULT_RELEVANCE_FLOOR,
            "the default floor of {DEFAULT_RELEVANCE_FLOOR} would return noise ({noise})"
        );
        assert!(
            answer > DEFAULT_RELEVANCE_FLOOR,
            "the default floor of {DEFAULT_RELEVANCE_FLOOR} would delete a real answer ({answer})"
        );
        // Room on both sides, not a hairline: a calibration this tight would
        // flip on the next model update.
        assert!(
            DEFAULT_RELEVANCE_FLOOR - noise > 0.1 && answer - DEFAULT_RELEVANCE_FLOOR > 0.1,
            "noise {noise}, floor {DEFAULT_RELEVANCE_FLOOR}, answer {answer}"
        );
    }

    /// The Gemini numbers behind the map row, measured by
    /// `recuerdos-ai calibrate --config recuerdos-ai.toml` (reranking-plan.md
    /// Phase 2.1/2.4):
    ///
    /// - noise ceiling `0.580` — the largest raw cosine any `negative`
    ///   query reaches against the corpus;
    /// - cluster floor `0.611` — the weakest raw cosine among the
    ///   floor-gated kinds (filter kinds are excluded: the handler lifts
    ///   the floor for them; relational targets arrive over a hop with
    ///   `GRAPH_HOP_RELEVANCE`);
    /// - weak answers `0.562` (`nl-category: health, fitness or family`)
    ///   and `0.572` (the dietary needle, which fails recall at any
    ///   threshold) — measured *below* the noise ceiling, which is what
    ///   makes this model's gap genuinely infeasible: **no** baseline
    ///   suppresses every negative case at `0.25` and keeps both of
    ///   those above it. The map row chooses silencing the negatives;
    ///   these assertions pin both halves of the trade-off, so any
    ///   future re-measure that changes who lands under the floor fails
    ///   here and forces the choice to be re-recorded in §12 rather
    ///   than quietly drifted into.
    const GEMINI_MEASURED_NOISE_CEILING: f32 = 0.580;
    const GEMINI_MEASURED_CLUSTER_FLOOR: f32 = 0.611;
    const GEMINI_MEASURED_WEAK_ANSWERS: [f32; 2] = [0.562, 0.572];

    #[test]
    fn the_gemini_map_row_suppresses_measured_noise_and_pins_its_tradeoff() {
        let baseline = default_similarity_baseline_for("gemini-embedding-2")
            .expect("gemini-embedding-2 is in the baseline map");
        let calibration = RelevanceCalibration::new(baseline);

        let noise = calibration.relevance(Some(GEMINI_MEASURED_NOISE_CEILING), 0.0, false);
        assert!(
            noise < DEFAULT_RELEVANCE_FLOOR,
            "a baseline of {baseline} would report noise as relevance ({noise})"
        );
        assert!(
            DEFAULT_RELEVANCE_FLOOR - noise > 0.02,
            "noise {noise} is a hair under the floor — re-measure"
        );

        let cluster = calibration.relevance(Some(GEMINI_MEASURED_CLUSTER_FLOOR), 0.0, false);
        assert!(
            cluster - DEFAULT_RELEVANCE_FLOOR > 0.02,
            "the weakest cluster answer ({cluster}) no longer clears the floor with room — re-measure"
        );

        // And the chosen trade-off itself: both measured weak answers sit
        // under the corrected noise, i.e. they are below the floor
        // wherever it sensibly sits. Pinning `below the noise` rather than
        // `below the floor` says the actual reason they fall.
        for weak in GEMINI_MEASURED_WEAK_ANSWERS {
            let relevance = calibration.relevance(Some(weak), 0.0, false);
            assert!(
                relevance < noise,
                "measured weak answer {weak} corrected to {relevance}, expected under noise {noise}"
            );
        }
    }

    #[test]
    fn an_empty_search_returns_nothing() {
        let result = ranker().rank(
            &RankedIds::default(),
            &RankedIds::default(),
            vec![memory()],
            now(),
        );

        assert!(result.is_empty());
    }

    // --- the rerank stage (reranking-plan.md Phase 4) -------------------

    fn reranked(a: &Memory, b: &Memory, c: &Memory) -> Vec<(MemoryId, f32)> {
        // A leg that puts `c` first, then `b`, then `a` — arbitrary raw
        // scores on a probability-like scale.
        vec![(c.id(), 0.9), (b.id(), 0.5), (a.id(), 0.1)]
    }

    /// The empty-list identity: `rank_with_legs` over no rerank scores
    /// must be the pre-rerank ranking — same order *and* the same
    /// `score`s — which is what makes the old wrappers' guarantee a code
    /// path rather than arithmetic.
    #[test]
    fn an_empty_rerank_leg_leaves_the_fused_ranking_identical() {
        let a = memory_aged(0, 1.0);
        let b = memory_aged(30, 0.5);
        let vector = ids(&[&a, &b]);
        let keyword = ids(&[&b, &a]);
        let candidates = || vec![a.clone(), b.clone()];

        let fused = ranker().rank(&vector, &keyword, candidates(), now());
        let with_stage = ranker().rank_with_legs(
            &vector,
            &keyword,
            &RankedIds::default(),
            &[],
            candidates(),
            now(),
        );

        assert_eq!(ordered(&fused), ordered(&with_stage));
        for (before, after) in fused.iter().zip(with_stage.iter()) {
            assert_eq!(before.score, after.score, "an empty leg changed a score");
            assert_eq!(after.match_detail.rerank_rank, None);
            assert_eq!(after.rerank_score, None);
        }
    }

    #[test]
    fn a_zero_weight_rerank_leg_shows_its_work_but_moves_nothing() {
        // `Rrf { weight: 0 }` ≡ off for the *ordering* — the leg records
        // ranks and scores but contributes no term to the sum.
        let (a, b, c) = (memory(), memory(), memory());
        let vector = ids(&[&a, &b, &c]);
        let keyword = ids(&[&a, &b, &c]);
        let candidates = || vec![a.clone(), b.clone(), c.clone()];

        let fused = ranker().rank(&vector, &keyword, candidates(), now());
        let zero = RecallRanker::new(90)
            .with_rerank_strategy(RerankStrategy::Rrf { weight: 0.0 })
            .rank_with_legs(
                &vector,
                &keyword,
                &RankedIds::default(),
                &reranked(&a, &b, &c),
                candidates(),
                now(),
            );

        assert_eq!(
            ordered(&fused),
            ordered(&zero),
            "a zero-weight leg reordered the results"
        );
        for (before, after) in fused.iter().zip(zero.iter()) {
            assert!(
                (before.score - after.score).abs() < 1e-6,
                "a zero-weight leg changed a score: {} → {}",
                before.score,
                after.score
            );
        }
        // And the stage's work is visible: b was ranked 2nd *by the leg*,
        // between the leg's top pick c and its last pick a.
        let b_in_stage = zero.iter().find(|s| s.memory.id() == b.id()).unwrap();
        assert_eq!(b_in_stage.match_detail.rerank_rank, Some(2));
        assert_eq!(b_in_stage.rerank_score, Some(0.5));
    }

    #[test]
    fn a_weight_above_one_lets_the_reranker_overrule_both_retrieval_legs() {
        // `answer` is rank 1 in both retrieval legs — their agreement.
        // `promoted` sits deep in one leg. At weight 1 the rerank leg adds
        // exactly one leg-rank-1's worth, lifting `promoted` at best to a
        // single-leg's standing, never over two-leg agreement; at weight 4
        // it overrules it — which is the point of allowing weights above
        // 1 (reranking-plan.md §2 row 4).
        let answer = memory();
        let promoted = memory();
        let mut vector = vec![answer.id()];
        vector.extend(std::iter::repeat_with(MemoryId::new).take(28));
        vector.push(promoted.id());
        let vector = RankedIds(vector);
        let keyword = ids(&[&answer]);
        let candidates = || vec![answer.clone(), promoted.clone()];
        // The leg covers only `promoted` — the recaller's own head shape,
        // and what keeps the arithmetic unambiguous: `answer` gets no rerank
        // term of its own to trade against.
        let leg = vec![(promoted.id(), 0.9)];

        let one = RecallRanker::new(90) // Rrf weight 1.0
            .rank_with_legs(
                &vector,
                &keyword,
                &RankedIds::default(),
                &leg,
                candidates(),
                now(),
            );
        assert_eq!(
            ordered(&one)[0],
            answer.id(),
            "weight 1 must not overrule two-leg agreement"
        );

        let four = RecallRanker::new(90)
            .with_rerank_strategy(RerankStrategy::Rrf { weight: 4.0 })
            .rank_with_legs(
                &vector,
                &keyword,
                &RankedIds::default(),
                &leg,
                candidates(),
                now(),
            );
        assert_eq!(
            ordered(&four)[0],
            promoted.id(),
            "weight 4 must be able to overrule vector+BM25 agreement"
        );
        // Precision, not recall: the demoted answer is still returned.
        assert!(four.iter().any(|s| s.memory.id() == answer.id()));
    }

    #[test]
    fn replace_reorders_the_head_by_score_and_leaves_the_tail_in_fused_order() {
        // The leg covers only two of the four candidates; the other two
        // keep their fused relative order directly behind the head.
        let a = memory();
        let b = memory();
        let c = memory();
        let d = memory();
        let vector = ids(&[&a, &b, &c, &d]);
        let candidates = || vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let leg = vec![(c.id(), 0.1), (b.id(), 0.9)]; // ranks: c 1st, b 2nd — scores say otherwise

        let result = RecallRanker::new(90)
            .with_rerank_strategy(RerankStrategy::Replace)
            .rank_with_legs(
                &vector,
                &RankedIds::default(),
                &RankedIds::default(),
                &leg,
                candidates(),
                now(),
            );

        assert_eq!(
            ordered(&result),
            vec![b.id(), c.id(), a.id(), d.id()],
            "Replace must order the head by score (b's 0.9 beats c's 0.1 despite the leg's own order) and preserve the tail"
        );
        let for_b = result.iter().find(|s| s.memory.id() == b.id()).unwrap();
        assert_eq!(for_b.rerank_score, Some(0.9));
        assert_eq!(for_b.match_detail.rerank_rank, Some(2)); // position in the leg, not the final place
        assert_eq!(
            result
                .iter()
                .find(|s| s.memory.id() == a.id())
                .unwrap()
                .rerank_score,
            None
        );
    }

    #[test]
    fn replace_bands_the_score_so_recency_decides_ties_but_cannot_bury_gaps() {
        // Same mechanism as the module's other multipliers: fresh and old
        // share the rerank score, recency decides; a clear score gap the
        // 1.25× band cannot cross stays with the clear winner.
        let fresh = memory_aged(0, 1.0);
        let old = memory_aged(730, 1.0);
        let vector = ids(&[&old, &fresh]);
        let candidates = || vec![old.clone(), fresh.clone()];

        let tie = RecallRanker::new(90)
            .with_rerank_strategy(RerankStrategy::Replace)
            .rank_with_legs(
                &vector,
                &RankedIds::default(),
                &RankedIds::default(),
                &[(old.id(), 1.0), (fresh.id(), 1.0)],
                candidates(),
                now(),
            );
        assert_eq!(
            ordered(&tie)[0],
            fresh.id(),
            "the band should let recency decide a rerank-score tie"
        );

        let gap = RecallRanker::new(90)
            .with_rerank_strategy(RerankStrategy::Replace)
            .rank_with_legs(
                &vector,
                &RankedIds::default(),
                &RankedIds::default(),
                &[(old.id(), 2.0), (fresh.id(), 1.0)],
                candidates(),
                now(),
            );
        assert_eq!(
            ordered(&gap)[0],
            old.id(),
            "a 2× rerank-score gap must not be inverted by a 1.25× band"
        );
    }

    #[test]
    fn scores_are_finite_for_every_input_combination() {
        for (days, confidence) in [(0, 0.0), (0, 1.0), (36_500, 0.0), (36_500, 1.0)] {
            let memory = memory_aged(days, confidence);
            let result = ranker().rank(
                &ids(&[&memory]),
                &ids(&[&memory]),
                vec![memory.clone()],
                now(),
            );
            assert!(
                result[0].score.is_finite() && result[0].score > 0.0,
                "score for ({days}d, {confidence}) was {}",
                result[0].score
            );
        }
    }
}
