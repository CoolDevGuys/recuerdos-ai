//! `recuerdos-ai rerank distribution` — the task 7.4 measurement
//! (reranking-plan.md): what the cross-encoder's scores are actually worth
//! on expected pairs versus noise, and whether they can carry `relevance`.
//!
//! # The question, and why fusion must stay out of it
//!
//! Task 7.4 asks whether the model's scores separate (a) the memories a
//! case *expects* from (b) the best the corpus has to offer a `negative`
//! query. Only if the two are cleanly separable may `relevance` be remapped
//! onto them (invariant I-4); anything else and the plan's answer is
//! documented inaction.
//!
//! Measuring that through recall would bias it in the model's favour in
//! one direction and against it in the other: recall only shows memories
//! fusion already placed in the head (so hard-to-retrieve expected pairs —
//! the `dietary` needle — never contribute a score), and the floor only
//! shows negatives that leaked (a suppressed negative reports no score
//! rather than a low one). So this command scores directly: every case
//! query against every corpus memory, no indexes, no fusion, no floor.
//! That is also the *strict* reading — a mapping defined on these numbers
//! is tested against the worst the corpus can do, not against the head
//! fusion happened to offer.
//!
//! The corpus and queries are the eval set, so the measurement covers the
//! same text every other number in §12 reads.

use crate::bootstrap::config::AppConfig;
use crate::bootstrap::eval::{self, EvalSet, percentile};
use crate::bootstrap::memories_wiring::load_reranker_for_audit;
use crate::memories::domain::reranker::{RerankCandidate, Reranker};
use crate::shared::error::{RaError, Result};
use serde::Serialize;
use std::path::Path;

/// One case's measurement.
#[derive(Debug, Clone, Serialize)]
pub struct CaseDistribution {
    pub name: String,
    pub kind: String,
    /// Score of the case's best-scoring expected memory (a multi-expect
    /// case takes its best answer, like `top_hit` does). Absent only if no
    /// expectation names a corpus memory — the eval's own validator
    /// rejects that, so for the committed set it is always present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_score: Option<f32>,
    /// 1-based rank the expected memory achieved *among the corpus for
    /// this query* — a cross-encoder can score an expected pair low and
    /// still rank it first, and the two failures are different.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_rank: Option<usize>,
    /// A `negative` case's number: the highest score anything in the
    /// corpus earned against it. Absent on scored cases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noise_score: Option<f32>,
}

/// The two distributions and the verdict task 7.4 hangs on them.
#[derive(Debug, Clone, Serialize)]
pub struct DistributionSummary {
    pub expectations: usize,
    pub expected_min: Option<f64>,
    pub expected_p25: Option<f64>,
    pub expected_median: Option<f64>,
    pub expected_p75: Option<f64>,
    pub expected_max: Option<f64>,
    /// Expected memories that ranked first for their query on raw score.
    pub expected_rank_one: usize,
    pub negatives: usize,
    /// Max of the per-negative maxima: where a relevance scale mapped onto
    /// these scores would have to keep `negative` queries silent.
    pub noise_ceiling: Option<f64>,
    /// Expected scores at or below the noise ceiling — the overlap,
    /// counted. Zero means cleanly separable.
    pub expected_at_or_below_ceiling: usize,
    pub separable: bool,
}

#[derive(Debug, Serialize)]
pub struct DistributionReport {
    pub reranker: String,
    pub corpus_unique: usize,
    pub cases: Vec<CaseDistribution>,
    pub summary: DistributionSummary,
}

/// Measures every case directly against the corpus. Takes the reranker as
/// a parameter so tests run it over a fake without a model on disk, and
/// `run` hands it the configured one.
pub(crate) fn measure(reranker: &dyn Reranker, set: &EvalSet) -> Result<DistributionReport> {
    // Deduplicate: the eval corpus carries a few memories twice (same
    // content, different subcategory), and scoring them twice only
    // doubles one point in the distribution.
    let mut corpus: Vec<&str> = Vec::with_capacity(set.memories.len());
    for memory in &set.memories {
        if !corpus.contains(&memory.content.as_str()) {
            corpus.push(&memory.content);
        }
    }
    if corpus.is_empty() {
        return Err(RaError::Validation(
            "the eval set has no corpus memories to score against".to_string(),
        ));
    }

    let mut rows = Vec::with_capacity(set.cases.len());
    let mut expected_scores: Vec<f64> = Vec::new();
    let mut noise_scores: Vec<f64> = Vec::new();
    let mut rank_one = 0_usize;

    for case in &set.cases {
        let candidates: Vec<RerankCandidate> = corpus
            .iter()
            .map(|content| RerankCandidate {
                // Ids are required by the contract and unread here: the
                // score of a memory is found back by its position in
                // `corpus`, which is the order the candidates were built in.
                id: crate::shared::ids::MemoryId::new(),
                text: content,
            })
            .collect();
        let scores = reranker.score(&case.query, &candidates)?;
        if scores.len() != corpus.len() {
            return Err(RaError::Internal(format!(
                "reranker returned {} scores for {} candidates on case {:?}",
                scores.len(),
                corpus.len(),
                case.name
            )));
        }

        if case.expect_empty {
            let noise = scores.iter().copied().reduce(f32::max);
            noise_scores.extend(noise.map(f64::from));
            rows.push(CaseDistribution {
                name: case.name.clone(),
                kind: case.kind.clone(),
                expected_score: None,
                expected_rank: None,
                noise_score: noise,
            });
            continue;
        }

        // The best-scoring expected memory, like `top_hit`'s "one of the
        // expected ones wins".
        let expected = corpus
            .iter()
            .enumerate()
            .filter(|&(_, text)| case.expect.iter().any(|wanted| wanted == text))
            .map(|(index, _)| scores[index])
            .max_by(|a, b| a.total_cmp(b));
        let Some(score) = expected else {
            // Not possible in the committed set — its validator rejects a
            // case expecting a memory the corpus lacks. A hand-made set
            // should still run and say which case it was.
            rows.push(CaseDistribution {
                name: case.name.clone(),
                kind: case.kind.clone(),
                expected_score: None,
                expected_rank: None,
                noise_score: None,
            });
            continue;
        };
        // Rank among the corpus: strictly-better scores + 1. Ties count
        // as not-beaten, so a rank of 1 means "at least as good as
        // everything else", which is the claim a top-1 actually makes.
        let better = scores.iter().filter(|other| **other > score).count();
        if better == 0 {
            rank_one += 1;
        }
        expected_scores.push(score as f64);
        rows.push(CaseDistribution {
            name: case.name.clone(),
            kind: case.kind.clone(),
            expected_score: Some(score),
            expected_rank: Some(better + 1),
            noise_score: None,
        });
    }

    let noise_ceiling = percentile(&noise_scores, 100.0);
    let at_or_below = noise_ceiling.map_or(0, |ceiling| {
        expected_scores
            .iter()
            .filter(|score| **score <= ceiling)
            .count()
    });
    let separable = match (
        expected_scores.iter().copied().reduce(f64::min),
        noise_ceiling,
    ) {
        (Some(floor_of_expected), Some(ceiling)) => floor_of_expected > ceiling,
        // No expectations or no negatives: "separable" would be a verdict
        // built on an empty sample, so it is not one.
        _ => false,
    };

    Ok(DistributionReport {
        reranker: String::new(),
        corpus_unique: corpus.len(),
        cases: rows,
        summary: DistributionSummary {
            expectations: expected_scores.len(),
            expected_min: expected_scores.iter().copied().reduce(f64::min),
            expected_p25: percentile(&expected_scores, 25.0),
            expected_median: percentile(&expected_scores, 50.0),
            expected_p75: percentile(&expected_scores, 75.0),
            expected_max: percentile(&expected_scores, 100.0),
            expected_rank_one: rank_one,
            negatives: noise_scores.len(),
            noise_ceiling,
            expected_at_or_below_ceiling: at_or_below,
            separable,
        },
    })
}

/// Runs the measurement with `[rerank].model` from `config`, whatever
/// `[rerank].mode` says — like the audit, this judges the stage precisely
/// where the honest config answer is still `off`.
pub fn run(cases_path: &Path, config: &AppConfig, json: bool) -> Result<DistributionReport> {
    let set = eval::load_set(cases_path)?;
    eval::validate(&set, cases_path)?;
    let reranker = load_reranker_for_audit(config)?;
    let reranker_name = reranker.model_id().to_string();

    let mut report = measure(&reranker, &set)?;
    report.reranker = reranker_name;

    if json {
        return Ok(report);
    }
    print_human(&report);
    Ok(report)
}

pub fn print_human(report: &DistributionReport) {
    println!(
        "reranker {} · {} unique corpus memories · {} cases scored directly (no fusion, no floor)",
        report.reranker,
        report.corpus_unique,
        report.cases.len()
    );
    println!();
    println!(
        "{:<44} {:>10} {:>6} {:>10}",
        "case", "expected", "rank", "noise"
    );
    println!("{}", "─".repeat(76));
    for row in &report.cases {
        println!(
            "{:<44} {:>10} {:>6} {:>10}",
            crate::bootstrap::eval::truncate(&row.name, 44),
            row.expected_score
                .map_or("—".to_string(), |score| format!("{score:.3}")),
            row.expected_rank
                .map_or("—".to_string(), |rank| rank.to_string()),
            row.noise_score
                .map_or("—".to_string(), |score| format!("{score:.3}")),
        );
    }

    let summary = &report.summary;
    let fmt =
        |value: Option<f64>| value.map_or_else(|| "—".to_string(), |value| format!("{value:.3}"));
    println!("{}", "─".repeat(76));
    println!(
        "expected ({}):  min {}   p25 {}   median {}   p75 {}   max {}   rank-1 {} of {}",
        summary.expectations,
        fmt(summary.expected_min),
        fmt(summary.expected_p25),
        fmt(summary.expected_median),
        fmt(summary.expected_p75),
        fmt(summary.expected_max),
        summary.expected_rank_one,
        summary.expectations,
    );
    println!(
        "negative ({}):  ceiling {}",
        summary.negatives,
        fmt(summary.noise_ceiling)
    );
    if summary.separable {
        println!(
            "=> cleanly separable: every expected pair outscores every negative's best. Task 7.4 \
             may consider a mapping to relevance — with the negative and floor-regression cases \
             as its gate, never as an afterthought."
        );
    } else {
        println!(
            "=> NOT separable: {} expected score(s) sit at or below the negative ceiling. \
             Task 7.4's answer is to keep relevance on the index-signal scale (I-4) and record \
             the numbers.",
            summary.expected_at_or_below_ceiling
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::eval::{EvalSet, test_case, test_memory};
    use crate::memories::application::test_doubles::FakeReranker;

    fn set_with(contents: &[&str], cases: Vec<crate::bootstrap::eval::Case>) -> EvalSet {
        EvalSet {
            memories: contents
                .iter()
                .map(|content| test_memory(content, "fact.project"))
                .collect(),
            cases,
        }
    }

    #[test]
    fn separable_distributions_say_so_and_rank_the_expected() {
        // Token-overlap fake: the expected memory shares two query tokens,
        // the distractor one, and the negative shares none.
        let set = set_with(
            &[
                "user prefers pnpm packages",
                "user prefers npm packages fast fast",
            ],
            vec![
                test_case(
                    "expected wins",
                    "near-miss",
                    "pnpm packages",
                    &["user prefers pnpm packages"],
                    false,
                ),
                test_case(
                    "silent negative",
                    "negative",
                    "coffee roast level",
                    &[],
                    true,
                ),
            ],
        );

        let report = measure(&FakeReranker::by_token_overlap(), &set).expect("measurement runs");
        let scored = &report.cases[0];
        assert_eq!(scored.expected_score, Some(2.0));
        assert_eq!(scored.expected_rank, Some(1));
        assert_eq!(report.cases[1].noise_score, Some(0.0));

        let summary = &report.summary;
        assert_eq!(summary.expectations, 1);
        assert_eq!(summary.negatives, 1);
        assert_eq!(summary.noise_ceiling, Some(0.0));
        assert_eq!(summary.expected_at_or_below_ceiling, 0);
        assert!(summary.separable, "2.0 expected vs a 0.0 ceiling");
    }

    #[test]
    fn an_expected_pair_under_the_ceiling_is_counted_and_blocks_the_verdict() {
        // The negative query out-matches the expected pair — the exact
        // shape task 7.4 must refuse to remap relevance onto.
        let set = set_with(
            &[
                "user prefers pnpm packages",
                "coffee coffee coffee roast level deep",
            ],
            vec![
                test_case(
                    "weak expected",
                    "needle",
                    "pnpm",
                    &["user prefers pnpm packages"],
                    false,
                ),
                test_case(
                    "loud negative",
                    "negative",
                    "coffee roast level deep",
                    &[],
                    true,
                ),
            ],
        );

        let report = measure(&FakeReranker::by_token_overlap(), &set).unwrap();
        let summary = &report.summary;
        assert_eq!(summary.expected_min, Some(1.0));
        // The fake counts the *candidate's* token occurrences present in
        // the query, so the corpus memory's three "coffee"s score 3.0 by
        // themselves — the ceiling is where its best noise pair landed,
        // not where the query's vocabulary is.
        assert_eq!(summary.noise_ceiling, Some(6.0));
        assert_eq!(summary.expected_at_or_below_ceiling, 1);
        assert!(!summary.separable);
    }

    #[test]
    fn duplicate_corpus_text_is_scored_once() {
        let set = set_with(
            &["user prefers pnpm packages", "user prefers pnpm packages"],
            vec![test_case(
                "dupes",
                "paraphrase",
                "pnpm packages",
                &["user prefers pnpm packages"],
                false,
            )],
        );
        let report = measure(&FakeReranker::by_token_overlap(), &set).unwrap();
        assert_eq!(report.corpus_unique, 1);
        assert_eq!(report.summary.expected_rank_one, 1);
    }

    #[test]
    fn a_misaligned_reranker_is_an_error_not_a_panic() {
        struct Misaligned;
        impl Reranker for Misaligned {
            fn score(&self, _query: &str, candidates: &[RerankCandidate<'_>]) -> Result<Vec<f32>> {
                Ok(vec![1.0; candidates.len() + 1])
            }
        }
        let set = set_with(
            &["a memory"],
            vec![test_case(
                "any",
                "paraphrase",
                "memory",
                &["a memory"],
                false,
            )],
        );
        let error = measure(&Misaligned, &set).unwrap_err();
        assert!(error.to_string().contains("scores for"), "{error}");
    }
}
