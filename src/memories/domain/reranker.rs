//! The second-stage ranker contract (reranking-plan.md Phase 4).
//!
//! # Why a second stage exists
//!
//! The three retrieval legs score *independently*: the vector leg never
//! sees the query and the memory's text together, BM25 sees only word
//! overlap, the graph sees only relations. Reciprocal rank fusion can
//! combine their opinions but cannot manufacture one neither leg had —
//! which is exactly the measured failure: the right memory in the top 5
//! but not first (eval/baseline: recall@5 ≫ precision@1). A cross-encoder
//! reads query and memory *together* and is the standard fix; it is a
//! second stage because it is too expensive to run over the whole corpus,
//! so it re-scores only the fused head.
//!
//! # Why scores come back aligned, not sorted
//!
//! [`Reranker::score`] returns one value per candidate, in the candidate
//! order it was given. A sorted `(id, score)` list would force the caller
//! to trust the adapter's ordering contract, and it would throw away the
//! raw value — which the shadow mode logs, the audit command prints, and
//! the `Replace` strategy multiplies by the bounded recency/quality band.
//! Aligning by position costs nothing and keeps every option open.
//!
//! # Why the scale is "model-specific" and relevance is untouched
//!
//! Cross-encoder outputs vary by model: sigmoid probabilities
//! (`Jina`, `BGE` rerankers), unbounded logits (`MS-MARCO MiniLM`),
//! similarities. The scale is deliberately unspecified here because the
//! fusion (`RerankStrategy` in [`super::recall_ranker`]) only ever
//! *ranks* the values or multiplies them by a bounded band — no threshold,
//! no floor, no unit assumption. And by invariant I-4 of the plan, the
//! cross-encoder score is carried as a **separate** field
//! ([`ScoredMemory::rerank_score`](super::recall_ranker::ScoredMemory)),
//! never folded into `relevance`: cross-encoder sigmoids saturate and are
//! trained on question→passage pairs, while memories are declarative
//! statements — a correct match can score low, and a floor built on it
//! would delete real memories until a Phase 7 measurement says otherwise.
//!
//! # Failure mode
//!
//! A recall request must never fail because the reranker failed or was
//! too slow (invariant I-2): [`super::super::application::memory_recaller`]
//! warns and serves the pre-rerank order. An *enabled* reranker that
//! cannot load at startup is a different matter and fails loudly there —
//! "you asked for this and it is broken" is not a question for the first
//! query to answer.

use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use std::time::Instant;

/// One candidate for the reranker to judge: its id, and the text it is
/// scored on. The id lets [`Reranker::score`] answers be aligned by
/// position without the adapter having to echo anything back.
// The fields are read by the model adapters that *implement* this
// contract (the Phase 5 fastembed adapter scores on `text`; the test
// doubles record `id`), while the recaller on this side constructs and
// aligns over them — `id` has no *production* reader until the Phase 6
// audit command echoes candidates back.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct RerankCandidate<'a> {
    pub id: MemoryId,
    pub text: &'a str,
}

/// A model that scores `(query, candidate)` pairs *together*.
///
/// Implementations are synchronous — like [`Embedder`](super::embedder),
/// the real ones are CPU-bound local inference — and the caller bounds
/// the call with a deadline and falls back on expiry, so implementations
/// do not police their own inference. What a pooled implementation *does*
/// police is its queueing: see [`score_before`](Self::score_before), which
/// stops a caller waiting for a session once the request's budget is gone.
///
/// The returned `Vec` must be the same length as `candidates`, aligned
/// with it by position. A mismatched length is treated as a failure of
/// the leg and degrades the request, not corrupts it.
pub trait Reranker: Send + Sync {
    /// Scores each `(query, text)` pair. The returned scores are aligned
    /// with `candidates` (same length, same order). Higher is better;
    /// the scale is model-specific.
    fn score(&self, query: &str, candidates: &[RerankCandidate<'_>]) -> Result<Vec<f32>>;

    /// Scores within `deadline`, and gives up on **waiting** for the model
    /// when the deadline passes: `Err(RaError::RerankBusy)` when no session
    /// is free by then, having scored nothing.
    ///
    /// The deadline is an absolute instant rather than a duration because
    /// the caller's budget covers waiting *and* judging: a request that
    /// queued for 200 ms of its 250 ms must not then hand the model the full
    /// 250 ms. That distinction is the point — without it a busy host turns
    /// every recall into a thread that parks on a session, scores a head
    /// nobody is waiting for any more, and makes the next request wait for
    /// it. Giving up before inference is what keeps abandoned work off the
    /// machine instead of merely off the response.
    ///
    /// A model that cannot tell queueing from inference — the doubles, and
    /// any implementation without a session pool — keeps the default, which
    /// is [`score`](Self::score) and therefore never reports busy.
    fn score_before(
        &self,
        query: &str,
        candidates: &[RerankCandidate<'_>],
        _deadline: Instant,
    ) -> Result<Vec<f32>> {
        self.score(query, candidates)
    }
}

/// What the recaller does with a configured reranker — the plan's
/// `off` / `shadow` / `on` (reranking-plan.md Phase 4.4) in the graph's
/// "inert until asked for" shape: the default
/// is [`RerankMode::Off`], and a recaller with `mode = Off` — even one
/// holding a loaded reranker — takes the pre-existing code path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RerankMode {
    /// No second stage. Identical behaviour to a build without the
    /// feature: this is the mode every deployment ships with until the
    /// Phase 7/8 gate says otherwise.
    #[default]
    Off,
    /// Score, log the comparison (ids and ranks only — never memory
    /// content or the raw query), but return the **pre-rerank**
    /// ordering. This is how the feature is judged on real traffic
    /// before any client sees it change (Phase 6).
    Shadow,
    /// Score and return the reranked ordering.
    On,
}
