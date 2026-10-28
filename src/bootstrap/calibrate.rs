//! `recuerdos-ai calibrate` — the measurement behind the relevance scale.
//!
//! # Why this exists as a command rather than a comment
//!
//! `[retrieval].min_relevance` is only meaningful relative to
//! `[retrieval].similarity_baseline`: relevance is
//! `(cosine − baseline) / (1 − baseline)`, so a baseline that is wrong for
//! the *configured embedding model* turns the floor into either a filter
//! that never fires (noise reported as relevance — reranking-plan.md §1's
//! five confident results for a stopword soup) or a maw that eats real
//! answers. The baseline is a property of the model, and the only honest
//! way to know it for a given model is to measure that model against a
//! corpus: the **noise ceiling** (how large the raw cosine of a query with
//! no answer gets) and the **signal floor** (how small the raw cosine of a
//! genuinely expected memory gets). Every number the calibration in
//! `memories::domain::recall_ranker` cites was produced this way.
//!
//! It was previously produced once by hand, and both failures of that
//! model show up in the plan: bge-small's constants went stale against a
//! larger corpus, and Gemini's were never measured at all. A hidden
//! command that recomputes them for *whatever config you point it at*
//! (reranking-plan.md Phase 2.1) makes the measurement — and therefore a
//! recalibration — repeatable.
//!
//! # What is measured
//!
//! Nothing is indexed or stored: memories embed as `Document` and queries
//! as `Query`, exactly as recall does, and the comparison is a true
//! cosine. Two by-products are reported because the same numbers answer
//! two `VERIFY` items of the plan:
//!
//! - **Vector norms** — `similarity_from_l2` assumes unit-length vectors;
//!   norms away from 1 say an embedder must normalise before storing.
//! - **Corrected values at the baseline in force** — the floor's room on
//!   both sides, i.e. whether the configured `min_relevance` still sits
//!   inside the measured gap.
//!
//! The corpus and queries are the eval set, so the measurement covers the
//! same text the quality gates score against.

use crate::bootstrap::config::AppConfig;
use crate::bootstrap::eval::EvalSet;
use crate::bootstrap::memories_wiring::{build_embedder, resolve_similarity_baseline};
use crate::memories::domain::category::Category;
use crate::memories::domain::embedder::EmbeddingTask;
use crate::memories::domain::recall_ranker::{
    DEFAULT_RELEVANCE_FLOOR, RelevanceCalibration, default_similarity_baseline_for,
};
use crate::shared::error::{RaError, Result};
use std::path::Path;

/// The kinds whose recall does *not* run behind the floor in production,
/// excluded from the gap-sensitive signal floor (both are still reported
/// per case, and the all-cases floor is printed alongside):
///
/// - `category/tag/subcategory-filter`: a request that names a category
///   has already said what it wants, and the handler lifts the floor for
///   it to `0.0` (`a_structural_filter_is_not_gated_behind_the_relevance_floor`).
/// - `relational`: its expected memories share no vocabulary with the
///   query *by construction*, and in recall they arrive over a graph hop,
///   where relevance is the hop estimate (`GRAPH_HOP_RELEVANCE`).
///
/// Letting either set the floor would demand a baseline that keeps the
/// vector leg from saying anything at all.
const FLOOR_EXEMPT_KINDS: &[&str] = &[
    "relational",
    "category-filter",
    "tag-filter",
    "subcategory-filter",
];

/// Runs the measurement for `config`'s embedder over `cases_path`'s corpus
/// and prints the report. Returns `Ok(())` unless an embed call failed;
/// the numbers are for a human (or the plan's §12) to read, not to gate.
pub fn run(cases_path: &Path, config: &AppConfig) -> Result<()> {
    let raw = std::fs::read_to_string(cases_path).map_err(|e| {
        RaError::Validation(format!("could not read {}: {e}", cases_path.display()))
    })?;
    let set: EvalSet = toml::from_str(&raw)
        .map_err(|e| RaError::Validation(format!("{} is not valid: {e}", cases_path.display())))?;
    if set.memories.is_empty() || set.cases.is_empty() {
        return Err(RaError::Validation(format!(
            "{} needs memories and cases",
            cases_path.display()
        )));
    }

    let embedder = build_embedder(config)?;
    // The exact precedence recall uses, so the corrected numbers below are
    // the corrected numbers the floor actually lives inside.
    let baseline = resolve_similarity_baseline(config, embedder.model_id());
    let calibration = RelevanceCalibration::new(baseline);
    let floor = config.retrieval.min_relevance;
    // `corpus` is the memories' content — what a case's `expect` names and
    // what the report prints. `documents` is what gets embedded, as the saver
    // would store it: under `enriched` the labels shift every cosine, so the
    // ceiling and floor must be read on the text recall compares against.
    let corpus: Vec<String> = set.memories.iter().map(|m| m.content.clone()).collect();
    let document_text = config.document_text();
    let documents: Vec<String> = set
        .memories
        .iter()
        .map(|m| {
            document_text.render(
                &Category::parse(&m.category)
                    .unwrap_or_else(|_| Category::Custom(m.category.clone())),
                m.subcategory.as_deref(),
                &m.tags,
                &m.content,
            )
        })
        .collect();

    println!(
        "embedder: {} ({} dimensions)",
        embedder.model_id(),
        embedder.dimensions()
    );
    let source = match config.retrieval.similarity_baseline {
        Some(_) => "explicit [retrieval].similarity_baseline",
        None if default_similarity_baseline_for(
            &config.document_text().pin(embedder.model_id()),
        )
        .is_some() =>
        {
            "per-model default (SIMILARITY_BASELINES)"
        }
        None => "UNCALIBRATED fallback — measure this model and set the value",
    };
    println!("baseline in force: {baseline}  ({source})");
    println!("floor in force:    {floor}    ([retrieval].min_relevance)\n");

    let memories = embedder.embed(&documents, EmbeddingTask::Document)?;
    report_norms("documents", &memories);
    let queries = embedder.embed(
        &set.cases
            .iter()
            .map(|c| c.query.clone())
            .collect::<Vec<_>>(),
        EmbeddingTask::Query,
    )?;
    report_norms("queries   ", &queries);
    println!();

    // ── noise ceiling ────────────────────────────────────────────────
    println!(
        "{:<46} {:>10}   best-matching corpus memory",
        "negative query", "max cos"
    );
    println!("{}", "─".repeat(96));
    let mut noise_ceiling = f32::MIN;
    for (case, query) in set.cases.iter().zip(&queries) {
        if !case.expect_empty {
            continue;
        }
        let (best, matched) = best_match(query, &corpus, &memories);
        noise_ceiling = noise_ceiling.max(best);
        println!(
            "{:<46} {:>10.3}   {}",
            truncate(&case.query, 46),
            best,
            truncate(matched, 38)
        );
    }
    println!(
        "\nnoise ceiling (max raw cosine of any negative over the corpus): {noise_ceiling:.3}  \
         → corrected at {baseline}: {}\n",
        calibration.relevance(Some(noise_ceiling), 0.0, false)
    );

    // ── signal floor ─────────────────────────────────────────────────
    println!(
        "{:<46} {:>10}   expected memory (min of its expectations)",
        "case", "min cos"
    );
    println!("{}", "─".repeat(96));
    let mut signal_floor = f32::MAX;
    let mut signal_floor_gated = f32::MAX;
    for (case, query) in set.cases.iter().zip(&queries) {
        if case.expect_empty {
            continue;
        }
        // The *weakest* expectation is the one a too-high baseline would
        // delete, so the floor of the floors is what matters.
        let (worst, wanted) = weakest_expectation(query, &case.expect, &corpus, &memories);
        signal_floor = signal_floor.min(worst);
        if !FLOOR_EXEMPT_KINDS.contains(&case.kind.as_str()) {
            signal_floor_gated = signal_floor_gated.min(worst);
        }
        println!(
            "{:<46} {:>10.3}   {}",
            truncate(&case.name, 46),
            worst,
            wanted.map(|w| truncate(w, 38)).unwrap_or_default()
        );
    }
    println!(
        "\nsignal floor, all scored cases:          {signal_floor:.3}  → corrected: {}",
        calibration.relevance(Some(signal_floor), 0.0, false)
    );
    println!(
        "signal floor, floor-gated kinds only:  {signal_floor_gated:.3}  → corrected: {}\n",
        calibration.relevance(Some(signal_floor_gated), 0.0, false)
    );

    // ── the floor's position ────────────────────────────────────────
    let noise = calibration.relevance(Some(noise_ceiling), 0.0, false);
    let answer = calibration.relevance(Some(signal_floor_gated), 0.0, false);
    let separates = noise < floor && answer > floor;
    println!(
        "floor {floor}: {}  (room under: {:+.3}, room over: {:+.3})",
        if separates {
            "inside the gap"
        } else {
            "NOT inside the gap"
        },
        floor - noise,
        answer - floor,
    );
    println!(
        "the shipped default floor {DEFAULT_RELEVANCE_FLOOR} would read noise {noise} / answer {answer}"
    );
    Ok(())
}

/// True cosine — does not assume unit vectors; that assumption is one of
/// the things this command reports on rather than makes.
pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

fn norms(vectors: &[Vec<f32>]) -> (f32, f32, f32) {
    let values: Vec<f32> = vectors
        .iter()
        .map(|v| v.iter().map(|x| x * x).sum::<f32>().sqrt())
        .collect();
    let min = values.iter().copied().fold(f32::INFINITY, f32::min);
    let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mean = values.iter().sum::<f32>() / values.len().max(1) as f32;
    (min, max, mean)
}

fn report_norms(label: &str, vectors: &[Vec<f32>]) {
    let (min, max, mean) = norms(vectors);
    let unit = (min - 1.0).abs().max((max - 1.0).abs()) < 1e-3;
    println!(
        "vector norms {label}: min {min:.4}  max {max:.4}  mean {mean:.4}  {}",
        if unit {
            "(unit-length)"
        } else {
            "(NOT unit-length — similarity_from_l2's assumption is violated)"
        }
    );
}

/// The corpus memory closest to `query`, with its raw cosine. `corpus` and
/// `vectors` are parallel (the order `embed` returned them in).
fn best_match<'a>(query: &[f32], corpus: &'a [String], vectors: &[Vec<f32>]) -> (f32, &'a str) {
    corpus
        .iter()
        .zip(vectors)
        .map(|(content, vector)| (cosine(query, vector), content.as_str()))
        .max_by(|(a, _), (b, _)| a.total_cmp(b))
        .expect("non-empty corpus, checked in run")
}

/// The expectation a query matches *least* — the pair a too-high baseline
/// would delete first — and the memory it names. `None` when no
/// expectation matches the corpus at all (a broken case), and the cosine
/// reads `NaN` so it cannot silently set the floor.
fn weakest_expectation<'a>(
    query: &[f32],
    expectations: &'a [String],
    corpus: &[String],
    vectors: &[Vec<f32>],
) -> (f32, Option<&'a str>) {
    expectations
        .iter()
        .filter_map(|wanted| {
            corpus
                .iter()
                .zip(vectors)
                .find(|(content, _)| content.as_str() == wanted.as_str())
                .map(|(_, vector)| (cosine(query, vector), wanted.as_str()))
        })
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(cosine, wanted)| (cosine, Some(wanted)))
        .unwrap_or((f32::NAN, None))
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars()
        .take(limit.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(x: f32, y: f32) -> Vec<f32> {
        let d = (x * x + y * y).sqrt();
        vec![x / d, y / d]
    }

    #[test]
    fn cosine_is_rotation_sensitive_but_length_insensitive() {
        let a = unit(1.0, 0.0);
        let b = unit(0.0, 1.0);
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-6);
        assert!(cosine(&a, &b).abs() < 1e-6);
        // Unnormalised inputs still compare as directions — this is the
        // property the norms report says `similarity_from_l2` assumes.
        let long = vec![3.0, 0.0];
        assert!((cosine(&a, &long) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_zero_vector_never_divides() {
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn norms_are_reported_over_the_whole_sample() {
        let (min, max, mean) = norms(&[vec![2.0, 0.0], vec![0.0, -4.0]]);
        assert_eq!((min, max), (2.0, 4.0));
        assert!((mean - 3.0).abs() < 1e-6);
    }

    fn corpus() -> Vec<String> {
        vec!["a".to_string(), "b".to_string()]
    }

    #[test]
    fn the_noise_ceiling_is_the_best_of_the_best() {
        let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let corpus = corpus();
        let (best, matched) = best_match(&[1.0, 1.0], &corpus, &vectors);
        assert!((best - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        // Ties pick *a* maximum; both are 45°, so assert on the value and
        // on *a* name from the corpus, which is what the report reads.
        assert!(matched == "a" || matched == "b");
    }

    #[test]
    fn the_signal_floor_is_the_weakest_expectation() {
        let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let corpus = corpus();
        let expectations = ["a".to_string(), "b".to_string()];
        let (worst, wanted) = weakest_expectation(&[1.0, 1.0], &expectations, &corpus, &vectors);
        assert!((worst - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!(wanted == Some("a") || wanted == Some("b"));
        // Expectation not in the corpus (a typo'd case): NaN, visible.
        let missing = ["missing".to_string()];
        let (worst, wanted) = weakest_expectation(&[1.0, 0.0], &missing, &corpus, &vectors);
        assert!(worst.is_nan());
        assert!(wanted.is_none());
    }

    #[test]
    #[ignore = "requires the ONNX model; run with --ignored"]
    fn random_text_stays_under_the_floor_with_the_calibration_in_force() {
        // The Phase 2 property, stated as the plan does: with the
        // calibrated baseline, queries that share nothing with the corpus
        // produce relevance below the floor for *every* corpus memory.
        // Random nonsense rather than the four negative cases on purpose —
        // `hiking trails` is topically adjacent noise on bge-small (recorded
        // in reranking-plan.md §12), not text with no direction at all.
        let cache = std::env::var("RECUERDOS_AI_MODEL_CACHE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("/models"));
        let config = AppConfig {
            embeddings: crate::bootstrap::config::EmbeddingsConfig {
                cache_dir: cache.to_string_lossy().to_string(),
                ..Default::default()
            },
            ..AppConfig::default()
        };
        let set_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("eval/cases.toml");
        let raw = std::fs::read_to_string(&set_path).unwrap();
        let set: EvalSet = toml::from_str(&raw).unwrap();

        let embedder = build_embedder(&config).unwrap();
        let baseline = resolve_similarity_baseline(&config, embedder.model_id());
        let calibration = RelevanceCalibration::new(baseline);

        let memories = embedder
            .embed(
                &set.memories
                    .iter()
                    .map(|m| m.content.clone())
                    .collect::<Vec<_>>(),
                EmbeddingTask::Document,
            )
            .unwrap();
        let questions = [
            "qqq xyzzy 12345 bloop",
            "what and or the for do",
            "the the the a an of and to",
        ];
        for question in questions {
            let query = embedder.embed_one(question, EmbeddingTask::Query).unwrap();
            for memory in &memories {
                let relevance = calibration.relevance(Some(cosine(&query, memory)), 0.0, false);
                assert!(
                    relevance < DEFAULT_RELEVANCE_FLOOR,
                    "{question:?} measured {relevance} against a corpus memory at baseline {baseline}"
                );
            }
        }
    }
}
