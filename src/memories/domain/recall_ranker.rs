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
//! Relevance leads; recency and confidence break ties among comparably
//! relevant results. Both invariants are pinned by tests below.

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
}

#[derive(Debug, Clone)]
pub struct ScoredMemory {
    pub memory: Memory,
    pub score: f32,
    pub match_detail: MatchDetail,
}

pub struct RecallRanker {
    recency_half_life_days: f32,
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
        }
    }

    /// Fuses both legs and returns memories best-first.
    ///
    /// `candidates` supplies the memory for every id either leg
    /// mentioned; ids without one are dropped (a row deleted between the
    /// index hit and the fetch).
    pub fn rank(
        &self,
        vector: &RankedIds,
        keyword: &RankedIds,
        candidates: Vec<Memory>,
        now: DateTime<Utc>,
    ) -> Vec<ScoredMemory> {
        let mut scored: Vec<ScoredMemory> = candidates
            .into_iter()
            .map(|memory| {
                let detail = MatchDetail {
                    vector_rank: rank_of(vector, memory.id()),
                    bm25_rank: rank_of(keyword, memory.id()),
                };
                let score = self.score(&memory, detail, now);
                ScoredMemory {
                    memory,
                    score,
                    match_detail: detail,
                }
            })
            .filter(|scored| {
                scored.match_detail.vector_rank.is_some() || scored.match_detail.bm25_rank.is_some()
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
        let relevance = reciprocal_rank(detail.vector_rank) + reciprocal_rank(detail.bm25_rank);

        relevance * self.recency_multiplier(memory.created_at(), now) * floored(memory.confidence())
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
