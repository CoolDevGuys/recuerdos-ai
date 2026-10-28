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
//! # Why relevance finally gets a multiplier, and how wide it may be
//!
//! Relevance used to be reported, thresholded, and nothing else: it was
//! computed after fusion and never entered the score, so a memory that matched
//! the question *better* lost every close call to one that happened to be a day
//! newer. `affinity` gives it a voice, in the shape the other two signals
//! already use — a bounded multiplier:
//!
//! ```text
//! affinity(r) = 1 − δ · (1 − r)        δ = [retrieval].relevance_weight ∈ [0, 0.5]
//! score'      = score × affinity(relevance)
//! ```
//!
//! How wide δ may go is the same calibration question the other bands answer,
//! and the answer here is less comfortable than theirs. Read δ as the *spread*
//! relevance is allowed across, and compare it with RRF's spacing at `k = 60`
//! (the gap between two memories ranked like this on both legs):
//!
//! | Rank gap | How far apart | Largest δ that cannot reverse it |
//! |---|---|---|
//! | rank 1 vs 2 | 1.6% | 0.016 |
//! | rank 1 vs 5 | 6.6% | 0.061 |
//! | rank 1 vs 10 | 14.8% | 0.129 |
//! | rank 1 vs 20 | 31.1% | 0.238 |
//! | rank 1 vs 60 | 96.7% | 0.492 |
//!
//! The last column is `1 − 1/R` for gap ratio `R`, and it assumes the worst
//! pair the range allows: the better-ranked memory at relevance `0`, the
//! worse-ranked one at `1`. Real lists are kinder, because the floor has
//! usually removed the bottom of the relevance range before affinity sees it.
//!
//! The comparison is asymmetric in one more way, and it favours relevance more
//! than the half-life suggests. Recency's own multiplier is
//! [`MULTIPLIER_FLOOR`]-bounded, so a day of age is worth 0.15% of a score here,
//! not the 0.77% raw decay implies, and no age whatsoever is worth more than the
//! band's 1.25×. Affinity gets its full `δ · Δr`. In a personal store, where the
//! memories competing for a rank are days apart rather than years, δ is therefore
//! doing more work per point than the table's rank gaps imply — which is a second
//! reason the grid, not the bound, is what picks the value.
//!
//! Which says out loud what the ceiling is really for: **δ = 0.5 overrules any
//! gap nearer than about 60 ranks** — nearly a whole result list, not a large
//! gap. So "bounded by 0.5" is not what keeps relevance from becoming the
//! primary sort key, and any doc comment that claimed it was is wrong arithmetic
//! rather than a wide band. What keeps it honest is measurement, and there are
//! now two independent ones behind [`DEFAULT_RELEVANCE_WEIGHT`] = 0.5.
//!
//! On the age-aware eval, against the baseline δ = 0.05 this replaces: precision@1
//! 53.8 → 67.3, MRR 68.4 → 75.9, recall@5 87.2 → 87.8, complete@5 and abstention
//! unchanged. Against δ = 0 on that corpus the ladder reads +17.3 precision@1. On a
//! deployed personal store — 18 questions written by the person whose memories they
//! are, replayed by `scripts/field-measure.sh` against the shipped binary with the
//! rerank stage off — precision@1 goes 27.8 → 50.0 and MRR 47.7 → 63.4 over the same
//! range. On neither instrument did anything holding first place ever lose it, at
//! any δ.
//!
//! Phase 2 shipped 0.05 instead, with the ceiling in view, and the reason is worth
//! keeping next to the changed number: this corpus rewards relevance partly by
//! construction (its expected memories are the topically nearest ones), so a
//! monotone line says little about δ once it is large enough to move ranks rather
//! than ties. That caution was tested rather than dropped, and the field store —
//! data nobody wrote for the test — read the same way. What neither instrument
//! settles is the one structural consequence of a δ this size: affinity is applied
//! before truncation, so it moves the top-`limit` *window* and not only the order
//! inside it. Both instruments saw that movement go towards better answers (the
//! field store's complete@5 rose 83.3 → 88.9 between δ = 0 and 0.05), which is a
//! measurement and not a guarantee — so the audit command, not this comment, is what
//! an operator should trust on their own store. §17 of the plan carries both tables
//! and the readings that could still reverse this.
//!
//! Because affinity is already multiplied into `score` when the rerank stage
//! recovers each candidate's band, the rerank leg lands *inside* it: a second
//! stage refines the ordering relevance helped choose rather than resetting the
//! weighting and serving back the model's list with recency pasted on. The two
//! stages commute, which is pinned — that is what makes "δ then rerank" and
//! "rerank then δ" the same arithmetic.
//!
//! Relevance leads; recency and quality break ties among comparably relevant
//! results. Since Phase 2 the first clause is arithmetic rather than aspiration:
//! relevance is in the score, not only in the report. All of it is pinned by
//! tests below.
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
    /// How the legs' raw signals are read as relevance.
    ///
    /// This decides the *number*; what decides its weight in the order is
    /// [`relevance_weight`](Self::relevance_weight), which turns it into a
    /// bounded multiplier after fusion. The fusion itself never consults either,
    /// so recalibrating the scale still cannot quietly become a re-ranker: the
    /// most a different baseline can do is move `relevance`, and δ is what
    /// decides how far that moves a result.
    relevance: RelevanceCalibration,
    /// Weight on relevance as a third bounded multiplier, in `0.0..=0.5`
    /// (Task 2, `[retrieval].relevance_weight`). Inert until
    /// [`apply_relevance`](Self::apply_relevance) is called — a ranker that
    /// fuses and never applies relevance orders exactly as it did before this
    /// knob existed.
    relevance_weight: f32,
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
            relevance_weight: DEFAULT_RELEVANCE_WEIGHT,
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

    /// Sets how strongly relevance counts inside the bounded bands
    /// (`[retrieval].relevance_weight`, applied by
    /// [`apply_relevance`](Self::apply_relevance)).
    ///
    /// Defaults to [`DEFAULT_RELEVANCE_WEIGHT`]. Zero is not a small weight but a
    /// separate path — the apply step returns before touching the list — so a
    /// build that never sets this knob cannot reorder anything even through a
    /// rounding error. The module docs' table is what says what each part of the
    /// range costs; the range itself is validated in `[retrieval]`.
    pub fn with_relevance_weight(mut self, weight: f32) -> Self {
        self.relevance_weight = weight;
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

    /// Lets relevance compete with recency and quality inside the fused bands
    /// (recall-quality-plan.md Phase 2).
    ///
    /// `score ×= affinity(relevance)` for every candidate, then re-sort. The
    /// multiplier spans `[1 − δ, 1]`, so it can decide a near-tie and — depending
    /// on δ — more than that: the module docs carry the rank gaps it can and
    /// cannot reverse, and the grid is what chooses δ against them.
    ///
    /// **The caller owns the position**, and it is not arbitrary. This reads
    /// `relevance`, so it has to run after the legs' raw scores have been turned
    /// into it; and it has to run before the floor and the rerank stage, so the
    /// head the second stage judges and the list the client's `limit` truncates
    /// are the ones this ordering chose.
    ///
    /// At δ = 0 it returns before touching anything — identity by code path, the
    /// same promise `apply_rerank` makes for an empty rerank list, for the same
    /// reason: "knob at zero, no change" must not depend on `1 − 0 · (1 − r)`
    /// evaluating to exactly `1.0` for every `r` in the list.
    pub fn apply_relevance(&self, ranked: &mut [ScoredMemory]) {
        // `<= 0.0` and `is_nan()` rather than a negated `>`: same set of
        // weights, and clippy's point about negated partial comparisons
        // generally. A NaN δ that escaped `[retrieval]`'s validation turns the
        // stage off instead of multiplying every score in the list into a NaN —
        // the failure `highest_first` was written to survive, and one this stage
        // should not manufacture.
        if self.relevance_weight <= 0.0 || self.relevance_weight.is_nan() {
            return;
        }

        for scored in ranked.iter_mut() {
            scored.score *= self.affinity(scored.relevance);
        }
        sort_ranked_with_relevance(ranked);
    }

    /// Relevance as a multiplier in `[1 − δ, 1]`.
    ///
    /// Clamped because a relevance outside `0..=1` — which only a broken
    /// calibration can produce — would widen the band by however far it strayed,
    /// silently undoing the bound the module docs calibrate.
    fn affinity(&self, relevance: f32) -> f32 {
        1.0 - self.relevance_weight * (1.0 - relevance.clamp(0.0, 1.0))
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

/// Reserves the head slots a graph hop may take (recall-quality-plan.md Phase 3).
///
/// The hop cannot win this fight arithmetically, which is why this is a rule
/// about *positions*. Fusion sums one term per leg, so a memory only the graph
/// reached brings one term to a fight where any direct match brings two: it loses
/// to it at every weight short of inverting the ranking (G1), and raising the
/// weight measured as raising the *bridge* — the memory naming both entities, the
/// one the indexes already love — while the answer at the end of the chain stayed
/// outside the client's `limit` (§1.3). So the graph reserves seats instead:
///
/// ```text
/// limit = 5, slots = 1, graph leg = [h]
/// before  [direct, direct, direct, direct, direct │ h, …]
/// after   [direct, h,      direct, direct, direct │ …]
/// ```
///
/// For each slot `1..=slots`, the first memory still standing in graph-leg order
/// at or below that slot's visibility edge moves into it. What that promises:
///
///  - **the top result is untouched.** Slot 0 is not on offer, so precision@1
///    cannot move. The edge arithmetic would include index 0 when `limit` is
///    smaller than the slot, and `.max(1)` below is what closes that.
///  - **filters and the floor hold.** The candidate has to be in `ranked` at all,
///    so a hop the category filter dropped or `[retrieval].min_relevance` refused
///    is not here to be promoted.
///  - **`slots = 0`, or no graph hits, is identity** — returned before the list
///    is read, the same escape `apply_relevance` and `apply_rerank` make.
///  - **it costs one visible memory.** Promoting from below the edge evicts the
///    row that was last in the window. That is the trade, and the grid measures
///    it: `complete@5` for `relational` and `chain` against everything else.
///
/// Positions are read from the list as it stands, so slot 2 sees the shift slot 1
/// caused, and a promoted memory now sits above every later slot's edge and so
/// cannot be promoted twice.
///
/// The row keeps its own `score` and `match_detail`: a slot is a decision about
/// placement, not new evidence about the match. Which also means a client that
/// re-sorted by `score` would undo it — `score` has always meant "how this recall
/// ordered them", and the answer to *why is this one here* is the leg's own
/// [`MatchDetail::graph_rank`], which this leaves pointing at the hop that found
/// it.
///
/// Returns how many memories were promoted, which is what the caller logs: an
/// operator reviewing a result list that changed needs to know the stage ran.
pub fn apply_expansion(
    ranked: &mut Vec<ScoredMemory>,
    graph: &RankedIds,
    limit: usize,
    slots: usize,
) -> usize {
    if slots == 0 || graph.0.is_empty() {
        return 0;
    }

    let mut promoted = 0;
    for slot in 1..=slots {
        // The visibility edge for this slot. Slot 1 wants what `truncate(limit)`
        // would cut off; each slot already filled has pushed one more row out of
        // the window, so the next asks one nearer. Reaching the edge is enough
        // rather than going past it because a candidate sitting exactly on it
        // costs nothing to promote: the row evicted from the window is the one
        // below it, which the client never saw either.
        let edge = limit.saturating_sub(slot).max(1);
        let found = graph.0.iter().find_map(|id| {
            ranked
                .iter()
                .position(|scored| scored.memory.id() == *id)
                // Strictly below the slot it would take: a memory already that
                // high is where it belongs, and this is the other half of what
                // keeps index 0 in place however the arithmetic falls.
                .filter(|&position| position > slot && position >= edge)
        });
        let Some(position) = found else { continue };

        let hit = ranked.remove(position);
        ranked.insert(slot, hit);
        promoted += 1;
    }
    promoted
}

/// Descending order for a score-like number, with NaN last.
///
/// `total_cmp` rather than `partial_cmp` because the `Equal` a partial
/// comparison reports for a NaN is not a consistent answer: it calls a NaN
/// tied with one value and unordered against the next, inside the same sort,
/// and a comparator that contradicts itself is not merely wrong — since Rust
/// 1.81 the sort may panic on it. `total_cmp` always answers.
///
/// It is wrapped rather than used bare because a *total* order puts a positive
/// NaN's bit pattern above `+infinity`: compared straight, a score that could
/// not be computed would win the list and be served first. A NaN means "no
/// number", which is the least of everything, so it is pinned to the bottom —
/// and pinning it keeps the order total, since every pair is answered the same
/// way no matter what the sort asks in between.
fn highest_first(a: f32, b: f32) -> std::cmp::Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => b.total_cmp(&a),
    }
}

/// `sort_ranked`'s order with one extra key: score, then the *relevance* that
/// produced it, then recency and id.
///
/// Only [`RecallRanker::apply_relevance`] sorts this way, which is why it is not
/// simply the canonical comparator. Promoting relevance to the second key
/// everywhere would also decide the exact-tie pairs that exist *today* — and this
/// phase's anchor is that δ = 0 changes nothing, in a corpus whose exact ties are
/// common enough to have their own noise floor (§17, Phase 0). Here it covers the
/// one situation affinity itself creates: two candidates whose post-affinity
/// scores come out equal, where preferring the better match is the entire point
/// of the knob.
fn sort_ranked_with_relevance(scored: &mut [ScoredMemory]) {
    scored.sort_by(|a, b| {
        highest_first(a.score, b.score)
            .then_with(|| highest_first(b.relevance, a.relevance))
            .then_with(|| b.memory.created_at().cmp(&a.memory.created_at()))
            .then_with(|| a.memory.id().to_string().cmp(&b.memory.id().to_string()))
    });
}

fn sort_ranked(scored: &mut [ScoredMemory]) {
    scored.sort_by(|a, b| {
        highest_first(a.score, b.score)
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

/// How strongly relevance competes with recency and quality inside the fused
/// bands (`[retrieval].relevance_weight`, recall-quality-plan.md Phase 2).
///
/// `0.0` is not "a very small weight": [`RecallRanker::apply_relevance`]
/// returns before touching the list at zero, so relevance cannot move a result
/// by any amount. That is what every eval baseline before Phase 2 was recorded
/// with, and what a deployment that wants the pre-Phase-2 order should set.
///
/// **`0.5`** is what the two instruments agreed on (plan §17). Against the baseline
/// this replaces (δ = 0.05, same build, re-recorded in `eval/baseline.json`):
/// precision@1 53.8 → 67.3 and MRR 68.4 → 75.9, with recall@5 up slightly
/// (87.2 → 87.8), complete@5 unchanged at 84.6, abstention unchanged at 100.0, and
/// no kind's precision@1 falling — four rose (`near-miss` 57.1 → 100.0,
/// `statement` 25.0 → 75.0, `needle` 33.3 → 66.7, `nl-category` 50.0 → 62.5).
/// Against δ = 0 on the same corpus the ladder reads +17.3 precision@1, rising
/// monotonically over 0.05 / 0.2 / 0.3 / 0.5 with nothing regressing at any step.
///
/// Two things that is not claiming, both worth keeping beside the number. Affinity
/// is applied **before** the list is cut to `limit`, so a δ this large moves which
/// memories are in the window as well as their order — on both instruments it moved
/// towards better answers (the field store's complete@5 rose 83.3 → 88.9 between
/// δ = 0 and 0.05), but that is a measurement rather than a guarantee, and
/// `recall audit` is how you check it on yours. And δ cannot move abstention at all:
/// the floor compares [`ScoredMemory::relevance`] while this multiplies `score`,
/// which is why 100.0 survived every value in the ladder.
///
/// It was not shipped on the eval alone, and that restraint was the right call when
/// it was made: Phase 2 left the ceiling unshipped because this corpus rewards
/// relevance partly by construction — its expected memories are the topically
/// nearest ones — so a monotone line says little about δ once it is big enough to
/// move ranks rather than ties. The instrument that could falsify that caution was
/// `scripts/field-measure.sh`: 18 real questions written by the owner of the store
/// they query, replayed against that store on the shipped binary with the rerank
/// stage off. It read the same direction with a larger effect (p@1 27.8 → 38.9 →
/// 44.4 → 50.0 across δ = 0 / 0.05 / 0.2 / 0.5, MRR 47.7 → 63.4) and, on both
/// instruments, no query ever lost first place at any value.
///
/// Which is how a default sits at the top of the permitted range `0.0..=0.5`
/// rather than below it. That shape is deliberate. The ceiling is not a safety
/// margin but the point the table above identifies — at δ = 0.5 the largest
/// relevance gap can reverse a sixty-rank separation, which is nearly a whole
/// result list, i.e. relevance as primary sort key. Reaching past it is a
/// different pipeline rather than a bigger number, so exploring beyond this
/// requires arguing for a new bound instead of nudging one.
pub const DEFAULT_RELEVANCE_WEIGHT: f32 = 0.5;

/// The relevance credited to a memory that entered recall *only* through a
/// graph hop.
///
/// Neither leg measured it — the hop reached it over a relation from a
/// memory the query did match — so there is no similarity to report, only
/// the strength of that indirect evidence. It has to sit above whatever
/// floor is configured, or the floor would silently delete relational
/// recall, which is the one thing the graph leg exists to add.
///
/// **The constraint that keeps this number honest.** It is a flat credit, so it
/// is only safe for a hop that started from a name the *store* confirmed:
/// `MemoryRecaller::hop` asks [`EntityGraph::seeds`] which of the query's
/// candidate mentions some memory of this user actually declared, and hops only
/// from the answer. A seed source that is not exact — embedding similarity
/// against relation text, say, which is what the §1.3 experiment tried — would
/// hand this same 0.35 to whatever a fuzzy match reached, and the eval measured
/// what that does: abstention 100.0 → **0.0**, every negative answering with
/// something (plan §17, Phase 0). So: any seed source that is not an exact
/// canonical match must carry its own similarity threshold, or relevance for a
/// graph-only hit must be derived from the path it came along rather than from
/// this constant.
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
/// - `bge-small-en-v1.5+enriched-v1` → `0.53`: the same model over
///   `[embeddings].document_text = "enriched"` vectors (the key is the
///   collection pin, see `memories::domain::document_text`). The shared
///   category header lifts every cosine by about `0.02` — noise ceiling
///   `0.611 → 0.631`, floor-gated answers `0.528 → 0.551` — so the bare
///   model's `0.5` would let one `negative` case through the `0.25` floor
///   (noise corrects to `0.263`). `0.53` restores the margin the content
///   row has: noise corrects to `0.215`.
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
pub const SIMILARITY_BASELINES: &[(&str, f32)] = &[
    ("bge-small-en-v1.5", 0.5),
    ("gemini-embedding-2", 0.46),
    ("bge-small-en-v1.5+enriched-v1", 0.53),
];

/// The measured baseline for `model_id` — a model id, or a collection pin
/// naming a model and its document format — or `None` when it has
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

    #[test]
    fn a_nan_score_cannot_break_the_ordering() {
        // A degenerate calibration can hand `relevance` a NaN, and affinity
        // multiplies that into the score. The sort itself must not fail:
        // `partial_cmp(…).unwrap_or(Equal)` calls a NaN tied with one value
        // and unordered against another in the same sort, and a comparator
        // that contradicts itself may panic (Rust ≥ 1.81). So the promise is
        // the boring one — every row survives, the real scores keep their
        // order, and the number that means nothing arrives last.
        let scored = |score: f32| ScoredMemory {
            memory: memory(),
            score,
            relevance: 0.5,
            rerank_score: None,
            match_detail: MatchDetail {
                vector_rank: Some(1),
                bm25_rank: Some(1),
                graph_rank: None,
                rerank_rank: None,
            },
        };
        let nan = scored(f32::NAN);
        let high = scored(0.02);
        let low = scored(0.01);

        let mut forwards = vec![nan.clone(), high.clone(), low.clone()];
        let mut backwards = vec![low.clone(), high.clone(), nan.clone()];
        sort_ranked(&mut forwards);
        sort_ranked(&mut backwards);

        assert_eq!(
            ordered(&forwards),
            ordered(&backwards),
            "a NaN made the order depend on the input order"
        );
        assert!(
            forwards[0].score > forwards[1].score,
            "the real scores should still be sorted, got {:?} then {:?}",
            forwards[0].score,
            forwards[1].score
        );
        assert!(
            forwards.last().unwrap().score.is_nan(),
            "a NaN sorts behind every real score, not into the middle"
        );
    }

    // — affinity: relevance inside the score (recall-quality-plan.md Phase 2) —

    /// Two memories ranked 1 in both legs, so their rank evidence is identical
    /// and only age, quality and relevance can separate them.
    fn symmetric_pair() -> (Memory, Memory) {
        let a = memory_aged(0, 1.0);
        let b = memory_aged(0, 1.0);
        (a, b)
    }

    fn with_relevance(
        mut ranked: Vec<ScoredMemory>,
        relevance: &[(MemoryId, f32)],
    ) -> Vec<ScoredMemory> {
        for scored in ranked.iter_mut() {
            scored.relevance = relevance
                .iter()
                .find(|(id, _)| *id == scored.memory.id())
                .map(|(_, value)| *value)
                .unwrap_or_default();
        }
        ranked
    }

    #[test]
    fn zero_weight_leaves_the_fused_order_and_scores_bit_identical() {
        // The phase's anchor at unit level. With the knob at zero the list is
        // not "recomputed to the same thing": `apply_relevance` returns before
        // reading it, so relevance cannot move a result by one bit. Relevance is
        // deliberately pointed the other way from the tie-breaks — the first
        // memory is the *less* relevant of the pair — so any affinity that ran
        // anyway would show up as a swap.
        let (first, second) = symmetric_pair();
        let built = || {
            ranker().rank(
                &ids(&[&first, &second]),
                &ids(&[&first, &second]),
                vec![first.clone(), second.clone()],
                now(),
            )
        };
        let fingerprint = |ranked: &[ScoredMemory]| {
            ranked
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

        let attached = || with_relevance(built(), &[(first.id(), 0.0), (second.id(), 1.0)]);
        let fingerprinted = fingerprint(&attached());
        assert_eq!(fingerprinted[0].0, first.id(), "the fixture itself moved");

        // `0.0` spelled out rather than taken from `ranker()`: the shipped
        // default is no longer zero, so a default-constructed ranker is not the
        // identity case any more — it is the case that moves the list.
        let mut ranked = attached();
        ranker()
            .with_relevance_weight(0.0)
            .apply_relevance(&mut ranked);
        assert_eq!(
            fingerprint(&ranked),
            fingerprinted,
            "δ = 0 touched the list"
        );
    }

    #[test]
    fn affinity_spans_exactly_the_band_the_module_docs_promise() {
        for delta in [0.05_f32, 0.2, 0.5] {
            let rank = ranker().with_relevance_weight(delta);
            for relevance in [-1.0_f32, 0.0, 0.25, 0.5, 1.0, 2.0, f32::MAX] {
                let affinity = rank.affinity(relevance);
                assert!(
                    (1.0 - delta..=1.0).contains(&affinity),
                    "δ = {delta}, relevance {relevance} escaped [1−δ, 1]: {affinity}"
                );
            }
            // The ends of the range are the ends of the band, and it is monotone:
            // a better match never gets the smaller multiplier.
            assert_eq!(rank.affinity(0.0), 1.0 - delta);
            assert_eq!(rank.affinity(1.0), 1.0);
            assert!(rank.affinity(0.0) < rank.affinity(0.5));
            assert!(rank.affinity(0.5) < rank.affinity(1.0));
        }
    }

    #[test]
    fn a_nan_weight_switches_affinity_off_rather_than_poisoning_every_score() {
        // `[retrieval]` refuses a NaN, so this only arrives from a ranker built
        // in code. Guarding it anyway keeps the worst case "changes nothing";
        // the alternative is multiplying every score in the list into a NaN and
        // leaving `highest_first` to sort a list that means nothing.
        let (a, b) = symmetric_pair();
        let mut ranked = with_relevance(
            ranker().rank(
                &ids(&[&a, &b]),
                &ids(&[&a, &b]),
                vec![a.clone(), b.clone()],
                now(),
            ),
            &[(a.id(), 0.1), (b.id(), 0.9)],
        );
        let fingerprint = |list: &[ScoredMemory]| {
            list.iter()
                .map(|scored| (scored.memory.id(), scored.score.to_bits()))
                .collect::<Vec<_>>()
        };
        let untouched = fingerprint(&ranked);

        ranker()
            .with_relevance_weight(f32::NAN)
            .apply_relevance(&mut ranked);

        assert_eq!(
            fingerprint(&ranked),
            untouched,
            "a NaN weight moved or poisoned a score"
        );
    }

    #[test]
    fn an_exact_symmetric_tie_goes_to_the_more_relevant_memory() {
        // v1/k2 against v2/k1: the rank sums are the same number, the ages and
        // confidences are equal, so before Phase 2 this was decided by id.
        // Relevance decides it now — in both assignments, which is what makes it
        // relevance rather than a tie-break that happens to agree.
        let (a, b) = symmetric_pair();
        let fuse = |weight: f32| {
            ranker().with_relevance_weight(weight).rank(
                &ids(&[&a, &b]),
                &ids(&[&b, &a]),
                vec![a.clone(), b.clone()],
                now(),
            )
        };

        let mut b_is_better = with_relevance(fuse(0.2), &[(a.id(), 0.4), (b.id(), 0.9)]);
        ranker()
            .with_relevance_weight(0.2)
            .apply_relevance(&mut b_is_better);
        assert_eq!(
            ordered(&b_is_better),
            [b.id(), a.id()],
            "the more relevant memory lost an exact rank tie"
        );

        let mut a_is_better = with_relevance(fuse(0.2), &[(a.id(), 0.9), (b.id(), 0.4)]);
        ranker()
            .with_relevance_weight(0.2)
            .apply_relevance(&mut a_is_better);
        assert_eq!(
            ordered(&a_is_better),
            [a.id(), b.id()],
            "the more relevant memory lost an exact rank tie"
        );
    }

    #[test]
    fn relevance_and_age_are_worth_this_much_against_each_other() {
        // The exchange rate, in both directions. Recency's multiplier is
        // `MULTIPLIER_FLOOR`-bounded, so three days at a 90-day half-life cost
        // 0.46% of a score — not the 1.5% raw decay suggests, and a day only
        // 0.15%. A relevance gap of 0.06 is worth 0.06·δ: 1.2% at δ = 0.2 and
        // 0.3% at δ = 0.05. So the same pair goes opposite ways at the two
        // settings, which is the knob's whole design in one test — relevance
        // competes with age, and what it wins is arithmetic an operator can read
        // off the config rather than guess at.
        let wiser = memory_aged(3, 1.0);
        let fresher = memory_aged(0, 1.0);
        let fuse = |weight: f32| {
            ranker().with_relevance_weight(weight).rank(
                &ids(&[&wiser, &fresher]),
                &ids(&[&fresher, &wiser]),
                vec![wiser.clone(), fresher.clone()],
                now(),
            )
        };

        for (delta, expected) in [(0.2_f32, wiser.id()), (0.05, fresher.id())] {
            let mut ranked =
                with_relevance(fuse(delta), &[(wiser.id(), 0.80), (fresher.id(), 0.74)]);
            ranker()
                .with_relevance_weight(delta)
                .apply_relevance(&mut ranked);
            assert_eq!(
                ordered(&ranked)[0],
                expected,
                "δ = {delta} should let {} win against a three-day age gap",
                if delta > 0.1 { "relevance" } else { "recency" }
            );
        }
    }

    #[test]
    fn how_far_relevance_can_overrule_a_rank_gap() {
        // Rank 1 against rank 10 on both legs is 14.8% apart, so the maximum
        // relevance gap (the better-ranked memory read as if it answered
        // nothing, the worse one as perfect) starts to reverse it just above
        // δ = 1 − 61/70 = 0.129. Pinned on both sides of the documented number,
        // because the table in the module docs is what an operator sizes a
        // config against — including the part of it that says a δ of 0.5 is a
        // measuring stick and not a shippable weight.
        let strong = memory_aged(0, 1.0);
        let weak = memory_aged(0, 1.0);
        let padding: Vec<Memory> = (0..8).map(|_| memory_aged(0, 1.0)).collect();
        let mut leg = vec![strong.clone()];
        leg.extend(padding.iter().cloned());
        leg.push(weak.clone());
        let leg_ids = RankedIds(leg.iter().map(|memory| memory.id()).collect());

        for (delta, expected) in [
            (0.05_f32, strong.id()),
            (0.10, strong.id()),
            (0.20, weak.id()),
            (0.50, weak.id()),
        ] {
            let rank = ranker().with_relevance_weight(delta);
            let mut ranked = rank.rank(&leg_ids, &leg_ids, leg.clone(), now());
            for scored in ranked.iter_mut() {
                scored.relevance = if scored.memory.id() == weak.id() {
                    1.0
                } else {
                    0.0
                };
            }
            rank.apply_relevance(&mut ranked);
            assert_eq!(
                ordered(&ranked)[0],
                expected,
                "δ = {delta}: the maximum relevance gap {} a nine-rank gap it should not",
                if expected == weak.id() {
                    "overruled"
                } else {
                    "held"
                }
            );
        }
    }

    #[test]
    fn a_graph_only_hit_is_penalised_exactly_as_the_formula_says() {
        // A hop's relevance is an estimate (`GRAPH_HOP_RELEVANCE`), not a
        // measured number, and affinity reads it like any other. So a δ taxes
        // every graph-only hit by δ·(1 − 0.35) — worth writing down before
        // Phase 3 gives the hop a slot of its own instead of a score.
        for delta in [0.05_f32, 0.2] {
            let hop = memory_aged(0, 1.0);
            let mut ranked = vec![ScoredMemory {
                memory: hop.clone(),
                score: 0.03,
                relevance: GRAPH_HOP_RELEVANCE,
                rerank_score: None,
                match_detail: MatchDetail {
                    vector_rank: None,
                    bm25_rank: None,
                    graph_rank: Some(1),
                    rerank_rank: None,
                },
            }];
            let rank = ranker().with_relevance_weight(delta);
            rank.apply_relevance(&mut ranked);
            assert_eq!(
                ranked[0].score,
                0.03 * rank.affinity(GRAPH_HOP_RELEVANCE),
                "δ = {delta} taxed the hop by something other than the formula"
            );
        }
    }

    #[test]
    fn a_nan_relevance_costs_the_row_its_number_not_the_request() {
        // The same hazard as the sort, arrived at from the other side: affinity
        // multiplies relevance into the score, so a NaN relevance becomes a NaN
        // score. The apply step must neither panic nor hand the top of the list
        // to the row whose number could not be computed.
        let sound = memory_aged(0, 1.0);
        let broken = memory_aged(0, 1.0);
        let detail = MatchDetail {
            vector_rank: Some(1),
            bm25_rank: Some(1),
            graph_rank: None,
            rerank_rank: None,
        };
        let mut ranked = vec![
            ScoredMemory {
                memory: sound.clone(),
                score: 0.03,
                relevance: 0.5,
                rerank_score: None,
                match_detail: detail,
            },
            ScoredMemory {
                memory: broken.clone(),
                score: 0.04,
                relevance: f32::NAN,
                rerank_score: None,
                match_detail: detail,
            },
        ];

        ranker()
            .with_relevance_weight(0.2)
            .apply_relevance(&mut ranked);

        assert_eq!(ordered(&ranked)[0], sound.id(), "a NaN score took the top");
        assert!(ranked[1].score.is_nan());
    }

    #[test]
    fn the_rerank_leg_is_added_inside_affinity_not_on_top_of_it() {
        // `apply_rerank` knows nothing about affinity: it rebuilds each
        // candidate's leg sum from the recorded ranks, recovers the band as
        // `score / sum`, and adds its own term inside that band. With affinity
        // already in `score`, the recovered band is recency × quality × affinity
        // — which is the intended reading (the stage refines the ordering it was
        // handed), and it makes the two stages commute.
        //
        // The pair is built so they genuinely disagree: affinity promotes `a`,
        // the reranker prefers `b`. Order them both ways round and the numbers
        // must agree, or one of the two stages is silently overwriting the other.
        let (a, b) = symmetric_pair();
        let fuse = |weight: f32| {
            ranker().with_relevance_weight(weight).rank(
                &ids(&[&a, &b]),
                &ids(&[&b, &a]),
                vec![a.clone(), b.clone()],
                now(),
            )
        };
        let relevance = [(a.id(), 0.9), (b.id(), 0.4)];
        let leg = [(b.id(), 0.9), (a.id(), 0.1)];
        let scores = |ranked: &[ScoredMemory]| {
            ranked
                .iter()
                .map(|scored| (scored.memory.id(), scored.score))
                .collect::<Vec<_>>()
        };

        // Affinity then rerank: what the pipeline actually does.
        let mut affinity_first = with_relevance(fuse(0.2), &relevance);
        let rank = ranker()
            .with_relevance_weight(0.2)
            .with_rerank_strategy(RerankStrategy::Rrf { weight: 1.0 });
        rank.apply_relevance(&mut affinity_first);
        rank.apply_rerank(&mut affinity_first, &leg, now());

        // Rerank then affinity: the same two multiplications in the other order.
        let mut rerank_first = with_relevance(fuse(0.0), &relevance);
        ranker()
            .with_rerank_strategy(RerankStrategy::Rrf { weight: 1.0 })
            .apply_rerank(&mut rerank_first, &leg, now());
        ranker()
            .with_relevance_weight(0.2)
            .apply_relevance(&mut rerank_first);

        let (want, got) = (scores(&affinity_first), scores(&rerank_first));
        assert_eq!(
            want.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            got.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            "the two orders disagreed about which memory comes first"
        );
        for ((id, want), (_, got)) in want.iter().zip(got.iter()) {
            assert!(
                (want - got).abs() <= want.abs() * 1e-6 + f32::EPSILON,
                "{id}: affinity-then-rerank scored {want}, rerank-then-affinity {got}"
            );
        }
    }

    #[test]
    fn a_zero_weight_rerank_leg_leaves_an_affinity_ordered_list_alone() {
        // The rerank stage's own no-op promise, restated where affinity has
        // already moved the head: a leg weighted 0 records what the model said
        // and changes no order. Without it, "turn the reranker off" and "turn
        // relevance off" would be two different escapes from a second stage that
        // did not help.
        let (a, b) = symmetric_pair();
        let mut ranked = with_relevance(
            ranker().rank(
                &ids(&[&a, &b]),
                &ids(&[&b, &a]),
                vec![a.clone(), b.clone()],
                now(),
            ),
            &[(a.id(), 0.4), (b.id(), 0.9)],
        );
        ranker()
            .with_relevance_weight(0.2)
            .apply_relevance(&mut ranked);
        let before = ordered(&ranked);
        assert_eq!(
            before[0],
            b.id(),
            "the fixture: affinity should have put the better match first"
        );

        ranker()
            .with_relevance_weight(0.2)
            .with_rerank_strategy(RerankStrategy::Rrf { weight: 0.0 })
            .apply_rerank(&mut ranked, &[(a.id(), 0.9), (b.id(), 0.1)], now());

        assert_eq!(
            ordered(&ranked),
            before,
            "a zero-weight rerank leg reordered an affinity-ordered list"
        );
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

    // — expansion: graph hits placed by position, not by score
    //   (recall-quality-plan.md Phase 3) —

    /// Six rows, `a` first, with the graph-only candidate `hop` parked at
    /// `position` and everything else a plain direct match. Returns the ranked
    /// list, the graph leg (deliberately *not* in fused order: `hop` is first
    /// there and last here, which is the whole reason the stage exists), and the
    /// hop itself.
    fn head_with_hop_at(position: usize) -> (Vec<ScoredMemory>, RankedIds, Memory) {
        let direct: Vec<Memory> = (0..5).map(|_| memory()).collect();
        let hop = memory();
        let mut all = direct.clone();
        all.insert(position.min(all.len()), hop.clone());

        let mut ranked: Vec<ScoredMemory> = all
            .iter()
            .enumerate()
            .map(|(index, m)| {
                let graph_only = m.id() == hop.id();
                ScoredMemory {
                    memory: m.clone(),
                    // A score that says "the leg order", so a move is visible as
                    // a position change and not as a score change.
                    score: 0.1 - index as f32 * 0.01,
                    relevance: if graph_only { GRAPH_HOP_RELEVANCE } else { 0.9 },
                    rerank_score: None,
                    match_detail: MatchDetail {
                        vector_rank: (!graph_only).then_some(index + 1),
                        bm25_rank: (!graph_only).then_some(index + 1),
                        graph_rank: graph_only.then_some(1),
                        rerank_rank: None,
                    },
                }
            })
            .collect();
        // Keep the list's own order (the fixture is already best-first).
        ranked.sort_by(|a, b| b.score.total_cmp(&a.score));
        (ranked, RankedIds(vec![hop.id()]), hop)
    }

    #[test]
    fn a_slot_moves_the_hop_into_the_window_the_client_would_otherwise_see() {
        // The phase's whole claim, at the smallest size that shows it: the hop is
        // below `limit`, and after the stage it is second.
        let (mut ranked, leg, hop) = head_with_hop_at(5);
        let before = ordered(&ranked);

        assert_eq!(apply_expansion(&mut ranked, &leg, 5, 1), 1);

        assert_eq!(ordered(&ranked)[1], hop.id(), "the slot is where it said");
        assert_eq!(
            ordered(&ranked)[0],
            before[0],
            "the top result is not on offer"
        );
        // Six candidates, five visible: promoting one from outside evicts exactly
        // the row that was last in the window — the trade, stated as a count.
        assert_eq!(ranked.len(), before.len(), "the stage must not duplicate");
        assert_eq!(ordered(&ranked)[5], before[4]);
    }

    #[test]
    fn no_number_of_slots_touches_index_zero() {
        // `limit < slot` is the case where "at or below the visibility edge"
        // arithmetic reaches position 0 on its own. p@1 is the metric this phase
        // promises cannot move, at every limit and every legal slot count.
        for limit in 0..=6 {
            for slots in 1..=4 {
                let (mut ranked, leg, _) = head_with_hop_at(3);
                let top = ordered(&ranked)[0];
                apply_expansion(&mut ranked, &leg, limit, slots);
                assert_eq!(
                    ordered(&ranked)[0],
                    top,
                    "limit {limit}, slots {slots} moved the top result"
                );
            }
        }
    }

    #[test]
    fn a_hop_the_edge_reaches_only_reordered_the_window_it_stayed_in() {
        // Sitting exactly on the edge costs nothing to promote: the row removed
        // was itself inside the window, so nothing visible is traded away. Worth a
        // test because it is the boundary `limit − slot` selects, so a reader
        // should know which way it falls.
        let (mut ranked, leg, hop) = head_with_hop_at(4);
        let before = ordered(&ranked);

        assert_eq!(apply_expansion(&mut ranked, &leg, 5, 1), 1);

        let after = ordered(&ranked);
        assert_eq!(after[1], hop.id());
        assert!(
            before[..5].iter().all(|id| after[..5].contains(id)),
            "a hop already in the window must not push one of its rows out"
        );
        assert_eq!(after[5], before[5], "the invisible rows simply slid down");
    }

    #[test]
    fn a_hop_already_inside_the_head_stays_put() {
        // Not a no-op-by-accident: the filter is "below the slot it would take",
        // so a memory the legs already ranked fourth is left alone — the stage
        // adds recall, it does not shuffle what ranks well.
        let (mut ranked, leg, hop) = head_with_hop_at(2);
        let before = ordered(&ranked);

        assert_eq!(apply_expansion(&mut ranked, &leg, 5, 1), 0);
        assert_eq!(ordered(&ranked), before);
        assert!(
            ranked.iter().any(|s| s.memory.id() == hop.id()),
            "and it is still there, merely not promoted"
        );
    }

    #[test]
    fn slots_fill_in_graph_order_and_never_promote_the_same_hop_twice() {
        // Two hops, one below the edge and one inside the head: slot 1 takes the
        // first in leg order, and slot 2 must not re-pick the winner — it either
        // finds a second hop below its own edge or leaves the list alone.
        let (mut ranked, leg, hop) = head_with_hop_at(9);
        let before = ordered(&ranked);

        assert_eq!(apply_expansion(&mut ranked, &leg, 5, 2), 1);
        let after = ordered(&ranked);
        assert_eq!(after[1], hop.id());
        assert_eq!(
            after.iter().filter(|id| **id == hop.id()).count(),
            1,
            "a promoted hop appeared twice"
        );
        // Nothing else moved except by the one insertion.
        let without_hop: Vec<_> = before
            .iter()
            .copied()
            .filter(|id| *id != hop.id())
            .collect();
        let after_without: Vec<_> = after.iter().copied().filter(|id| *id != hop.id()).collect();
        assert_eq!(after_without, without_hop);
    }

    #[test]
    fn zero_slots_or_an_empty_leg_return_before_reading_the_list() {
        // The anchor: `slots = 0` is the pipeline before this phase, bit for bit —
        // scores included, which a re-sort would have left alone anyway, so the
        // fingerprint is deliberately the whole row rather than the ids.
        let fingerprint = |list: &[ScoredMemory]| {
            list.iter()
                .map(|s| (s.memory.id(), s.score.to_bits()))
                .collect::<Vec<_>>()
        };

        let (mut ranked, leg, _) = head_with_hop_at(5);
        let untouched = fingerprint(&ranked);

        assert_eq!(apply_expansion(&mut ranked, &leg, 5, 0), 0);
        assert_eq!(fingerprint(&ranked), untouched, "slots = 0 touched it");

        assert_eq!(apply_expansion(&mut ranked, &RankedIds::default(), 5, 3), 0);
        assert_eq!(fingerprint(&ranked), untouched, "an empty leg touched it");
    }

    #[test]
    fn a_promotion_leaves_the_score_and_the_provenance_of_its_row_alone() {
        // The slot is placement, not evidence: nothing is rewritten, so a client
        // reading `matched.graph_rank` still learns which leg found it, and a
        // client re-sorting by score would undo the slot — the documented
        // consequence of not inventing a number to represent it.
        let (mut ranked, leg, hop) = head_with_hop_at(5);
        let before = ranked
            .iter()
            .find(|s| s.memory.id() == hop.id())
            .unwrap()
            .match_detail;
        let score_before = ranked
            .iter()
            .find(|s| s.memory.id() == hop.id())
            .unwrap()
            .score;

        apply_expansion(&mut ranked, &leg, 5, 1);

        let moved = ranked.iter().find(|s| s.memory.id() == hop.id()).unwrap();
        assert_eq!(moved.match_detail, before);
        assert_eq!(moved.match_detail.graph_rank, Some(1));
        assert_eq!(moved.score, score_before);
    }

    #[test]
    fn slots_wider_than_the_list_are_harmless() {
        // `limit` and `slots` come from a request and a config file, so the
        // combinations that look nonsensical must not panic or lose a row.
        let (mut ranked, leg, hop) = head_with_hop_at(1);
        let rows = ranked.len();
        apply_expansion(&mut ranked, &leg, 1, 8);
        assert_eq!(ranked.len(), rows);
        assert!(ranked.iter().any(|s| s.memory.id() == hop.id()));
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
