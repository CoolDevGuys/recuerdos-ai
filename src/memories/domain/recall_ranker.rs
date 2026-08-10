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
}

#[derive(Debug, Clone)]
pub struct ScoredMemory {
    pub memory: Memory,
    pub score: f32,
    pub match_detail: MatchDetail,
}

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
        }
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
                };
                let score = self.score(&memory, detail, now);
                ScoredMemory {
                    memory,
                    score,
                    match_detail: detail,
                }
            })
            .filter(|scored| {
                scored.match_detail.vector_rank.is_some()
                    || scored.match_detail.bm25_rank.is_some()
                    || scored.match_detail.graph_rank.is_some()
            })
            .collect();

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

        scored
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

fn reciprocal_rank(rank: Option<usize>) -> f32 {
    match rank {
        Some(rank) => 1.0 / (RRF_K + rank as f32),
        None => 0.0,
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
    fn an_empty_search_returns_nothing() {
        let result = ranker().rank(
            &RankedIds::default(),
            &RankedIds::default(),
            vec![memory()],
            now(),
        );

        assert!(result.is_empty());
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
