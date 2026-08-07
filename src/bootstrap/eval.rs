//! `recuerdos-ai eval` — the retrieval-quality harness.
//!
//! Every other test asserts that a component behaves as written. This one
//! asserts the thing the service is actually for: that asking a question
//! returns the memory that answers it.
//!
//! That property is not protected by any type or any unit test. It
//! emerges from the embedding model, the BM25 tokenizer, the RRF
//! constant, the recency multiplier and the candidate depth — and it can
//! be destroyed by a one-line change to any of them without a single
//! existing test going red. Phase 2 already saw this happen once: a
//! recency floor of 0.5 quietly made freshness outrank relevance.
//!
//! So it runs against the **real** embedding model. Substituting the fake
//! one would make every score here meaningless, since the fake exists
//! precisely to be deterministic rather than good.
//!
//! Deliberately not a `#[test]`: it takes seconds, needs a model on disk,
//! and reports a score rather than a pass/fail. CI runs it as a gate with
//! `--baseline`; a human runs it bare while changing the ranker.

use crate::bootstrap::memories_wiring::Memories;
use crate::bootstrap::wiring::Identity;
use crate::identity::domain::scope::Scope;
use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::category::Category;
use crate::memories::domain::memory::{MemorySource, NewMemory};
use crate::memories::domain::recall_query::RecallQuery;
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::SqliteDatabase;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// The cut-off the gate is defined at.
///
/// Five because that is roughly what fits in an agent's context budget
/// alongside an actual conversation — a memory ranked eighth is not
/// wrong, it is just never seen.
const K: usize = 5;

#[derive(Debug, Deserialize)]
struct EvalSet {
    #[serde(default, rename = "memory")]
    memories: Vec<SeedMemory>,
    #[serde(default, rename = "case")]
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct SeedMemory {
    content: String,
    category: String,
    #[serde(default)]
    subcategory: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    kind: String,
    query: String,
    expect: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    subcategories: Vec<String>,
}

/// What one case scored.
#[derive(Debug)]
struct CaseResult {
    name: String,
    kind: String,
    /// Fraction of expected memories that appeared in the top K.
    recall: f64,
    /// Whether the single best-ranked result was one of the expected ones.
    top_hit: bool,
    /// Expected memories that did not appear, for the report.
    missed: Vec<String>,
}

/// The machine-readable score, and the shape of `eval/baseline.json`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    /// Mean recall@5 across all cases, 0–100.
    pub recall_at_k: f64,
    /// Fraction of cases whose top result was correct, 0–100.
    pub precision_at_1: f64,
    pub cases: usize,
    /// Mean recall@5 per `kind`, so a regression says *what* got worse.
    pub by_kind: BTreeMap<String, f64>,
}

/// Runs the eval set and returns the report.
pub fn run(cases_path: &Path, model_cache_dir: Option<&Path>) -> Result<Report> {
    let raw = std::fs::read_to_string(cases_path).map_err(|e| {
        RaError::Validation(format!("could not read {}: {e}", cases_path.display()))
    })?;
    let set: EvalSet = toml::from_str(&raw)
        .map_err(|e| RaError::Validation(format!("{} is not valid: {e}", cases_path.display())))?;

    if set.cases.is_empty() {
        return Err(RaError::Validation(format!(
            "{} contains no cases",
            cases_path.display()
        )));
    }

    let (memories, context, _scratch) = seed(&set, model_cache_dir)?;

    let mut results = Vec::with_capacity(set.cases.len());
    for case in &set.cases {
        results.push(score(&memories, &context, case)?);
    }

    print_report(&results);
    Ok(summarise(&results))
}

/// Builds a throwaway instance and fills it with the corpus.
///
/// The `TempDir` comes back so the caller holds it: dropping it deletes
/// the database and the tantivy indexes out from under the run.
fn seed(
    set: &EvalSet,
    model_cache_dir: Option<&Path>,
) -> Result<(Memories, UserContext, tempfile::TempDir)> {
    let scratch = tempfile::tempdir()
        .map_err(|e| RaError::Internal(format!("could not create a scratch directory: {e}")))?;

    let mut config = crate::bootstrap::config::AppConfig::default();
    config.storage.path = scratch.path().to_string_lossy().to_string();
    if let Some(cache) = model_cache_dir {
        config.embeddings.cache_dir = cache.to_string_lossy().to_string();
    }

    let database = Arc::new(SqliteDatabase::open(
        &scratch.path().join(crate::bootstrap::wiring::DATABASE_FILE),
    )?);
    let identity = Identity::from_database(Arc::clone(&database))?;
    let memories = Memories::build(&config, database)?;

    // A real key, authenticated for real: the eval must exercise the same
    // scoping every request does, or it could pass against a store no
    // client could actually read.
    identity.user_creator.execute("eval", None)?;
    let issued =
        identity
            .api_key_issuer
            .execute("eval", vec![Scope::Read, Scope::Write], "eval")?;
    let context = identity.key_authenticator.execute(&issued.token.render())?;

    for seed in &set.memories {
        memories.saver.execute(
            &context,
            NewMemory {
                content: seed.content.clone(),
                category: Category::parse(&seed.category)?,
                subcategory: seed.subcategory.clone(),
                tags: seed.tags.clone(),
                entities: Vec::new(),
                confidence: 1.0,
                source: MemorySource {
                    client: Some("eval".to_string()),
                    session_id: None,
                },
                expires_at: None,
            },
            "eval",
        )?;
    }

    Ok((memories, context, scratch))
}

fn score(memories: &Memories, context: &UserContext, case: &Case) -> Result<CaseResult> {
    let categories = case
        .categories
        .iter()
        .map(|raw| Category::parse(raw))
        .collect::<Result<Vec<_>>>()?;

    let query = RecallQuery::new(&case.query, K)?
        .with_categories(categories)
        .with_tags(case.tags.clone())
        .with_subcategories(case.subcategories.clone());

    let hits = memories.recaller.execute(context, &query)?;
    let returned: Vec<&str> = hits.iter().map(|hit| hit.memory.content()).collect();

    let missed: Vec<String> = case
        .expect
        .iter()
        .filter(|wanted| !returned.iter().any(|got| got == &wanted.as_str()))
        .cloned()
        .collect();

    let found = case.expect.len() - missed.len();

    Ok(CaseResult {
        name: case.name.clone(),
        kind: case.kind.clone(),
        recall: found as f64 / case.expect.len() as f64,
        top_hit: returned
            .first()
            .is_some_and(|top| case.expect.iter().any(|wanted| wanted == top)),
        missed,
    })
}

fn summarise(results: &[CaseResult]) -> Report {
    let mean = |values: &[f64]| -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        values.iter().sum::<f64>() / values.len() as f64 * 100.0
    };

    let mut by_kind: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for result in results {
        by_kind
            .entry(result.kind.clone())
            .or_default()
            .push(result.recall);
    }

    Report {
        recall_at_k: mean(&results.iter().map(|r| r.recall).collect::<Vec<_>>()),
        precision_at_1: mean(
            &results
                .iter()
                .map(|r| if r.top_hit { 1.0 } else { 0.0 })
                .collect::<Vec<_>>(),
        ),
        cases: results.len(),
        by_kind: by_kind
            .into_iter()
            .map(|(kind, values)| (kind, mean(&values)))
            .collect(),
    }
}

fn print_report(results: &[CaseResult]) {
    println!("{:<42} {:>16} {:>7}  top", "case", "kind", "recall");
    println!("{}", "─".repeat(76));

    for result in results {
        println!(
            "{:<42} {:>16} {:>6.0}% {:>4}",
            truncate(&result.name, 42),
            result.kind,
            result.recall * 100.0,
            if result.top_hit { "✓" } else { "·" },
        );
        // Naming what was missed is the difference between "the score
        // dropped" and knowing which change to look at.
        for missed in &result.missed {
            println!("    missed: {}", truncate(missed, 68));
        }
    }

    let report = summarise(results);
    println!("{}", "─".repeat(76));
    println!(
        "recall@{K}: {:.1}%   precision@1: {:.1}%   ({} cases)",
        report.recall_at_k, report.precision_at_1, report.cases
    );
    for (kind, score) in &report.by_kind {
        println!("  {kind:>16}: {score:.1}%");
    }
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

/// Compares against a committed baseline and fails on a regression.
///
/// A threshold rather than exact equality: these scores move a little
/// with model versions and tokenizer changes, and a gate that fails on
/// noise gets disabled within a week. `max_drop` is in percentage points.
pub fn compare(report: &Report, baseline_path: &Path, max_drop: f64) -> Result<()> {
    let raw = std::fs::read_to_string(baseline_path).map_err(|e| {
        RaError::Validation(format!("could not read {}: {e}", baseline_path.display()))
    })?;
    let baseline: Report = serde_json::from_str(&raw).map_err(|e| {
        RaError::Validation(format!("{} is not valid: {e}", baseline_path.display()))
    })?;

    let drop = baseline.recall_at_k - report.recall_at_k;
    println!(
        "\nbaseline recall@{K}: {:.1}%   now: {:.1}%   change: {:+.1} points",
        baseline.recall_at_k, report.recall_at_k, -drop
    );

    // Named per kind as well: an overall score that held steady while one
    // kind collapsed and another improved is exactly the regression a
    // single number hides.
    for (kind, before) in &baseline.by_kind {
        if let Some(after) = report.by_kind.get(kind) {
            let kind_drop = before - after;
            if kind_drop > max_drop {
                println!("  {kind}: {before:.1}% → {after:.1}% ({kind_drop:.1} points worse)");
            }
        }
    }

    if drop > max_drop {
        return Err(RaError::Validation(format!(
            "recall@{K} dropped {drop:.1} points (limit {max_drop:.1}). Either the change \
             made retrieval worse, or the eval set needs updating — decide which, and if \
             it is the latter, re-record the baseline with `--write-baseline`."
        )));
    }

    Ok(())
}

/// Records the current scores as the new baseline.
pub fn write_baseline(report: &Report, path: &Path) -> Result<()> {
    let encoded = serde_json::to_string_pretty(report)
        .map_err(|e| RaError::Internal(format!("could not encode the baseline: {e}")))?;

    std::fs::write(path, format!("{encoded}\n"))
        .map_err(|e| RaError::Internal(format!("could not write {}: {e}", path.display())))?;

    println!("wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(kind: &str, recall: f64, top_hit: bool) -> CaseResult {
        CaseResult {
            name: format!("{kind} case"),
            kind: kind.to_string(),
            recall,
            top_hit,
            missed: vec![],
        }
    }

    #[test]
    fn recall_is_the_mean_across_cases() {
        let report = summarise(&[
            result("paraphrase", 1.0, true),
            result("paraphrase", 0.5, false),
        ]);

        assert_eq!(report.recall_at_k, 75.0);
        assert_eq!(report.precision_at_1, 50.0);
        assert_eq!(report.cases, 2);
    }

    #[test]
    fn scores_are_broken_down_by_kind() {
        // An overall number that held steady while one kind collapsed is
        // exactly the regression a single figure hides.
        let report = summarise(&[
            result("paraphrase", 1.0, true),
            result("exact-token", 0.0, false),
        ]);

        assert_eq!(report.by_kind["paraphrase"], 100.0);
        assert_eq!(report.by_kind["exact-token"], 0.0);
        assert_eq!(report.recall_at_k, 50.0, "the mean hides the collapse");
    }

    #[test]
    fn a_small_drop_is_tolerated_and_a_large_one_is_not() {
        let baseline = tempfile::NamedTempFile::new().unwrap();
        write_baseline(
            &Report {
                recall_at_k: 90.0,
                precision_at_1: 80.0,
                cases: 10,
                by_kind: BTreeMap::new(),
            },
            baseline.path(),
        )
        .unwrap();

        let slightly_worse = Report {
            recall_at_k: 87.0,
            precision_at_1: 80.0,
            cases: 10,
            by_kind: BTreeMap::new(),
        };
        assert!(
            compare(&slightly_worse, baseline.path(), 5.0).is_ok(),
            "a gate that fails on noise gets disabled within a week"
        );

        let much_worse = Report {
            recall_at_k: 80.0,
            precision_at_1: 80.0,
            cases: 10,
            by_kind: BTreeMap::new(),
        };
        let error = compare(&much_worse, baseline.path(), 5.0).unwrap_err();
        assert!(error.to_string().contains("dropped 10.0 points"), "{error}");
    }

    #[test]
    fn an_improvement_never_fails_the_gate() {
        let baseline = tempfile::NamedTempFile::new().unwrap();
        write_baseline(
            &Report {
                recall_at_k: 80.0,
                precision_at_1: 70.0,
                cases: 10,
                by_kind: BTreeMap::new(),
            },
            baseline.path(),
        )
        .unwrap();

        let better = Report {
            recall_at_k: 95.0,
            precision_at_1: 90.0,
            cases: 10,
            by_kind: BTreeMap::new(),
        };
        assert!(compare(&better, baseline.path(), 5.0).is_ok());
    }

    #[test]
    fn the_committed_eval_set_parses_and_has_cases() {
        // Cheap, and it means a typo in cases.toml surfaces in `cargo
        // test` rather than only when someone runs the eval.
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("eval/cases.toml"),
        )
        .expect("eval/cases.toml should exist");

        let set: EvalSet = toml::from_str(&raw).expect("eval/cases.toml should parse");

        assert!(
            set.memories.len() >= 10,
            "the corpus is too small to be hard"
        );
        assert!(!set.cases.is_empty());

        // Every expectation must name a memory that is actually seeded,
        // or the case can never pass and the score is a lie.
        for case in &set.cases {
            for wanted in &case.expect {
                assert!(
                    set.memories.iter().any(|m| &m.content == wanted),
                    "case {:?} expects a memory that is not in the corpus: {wanted:?}",
                    case.name
                );
            }
        }

        // And every seeded category must be a real one.
        for memory in &set.memories {
            Category::parse(&memory.category)
                .unwrap_or_else(|e| panic!("{:?}: {e}", memory.content));
        }
    }
}
