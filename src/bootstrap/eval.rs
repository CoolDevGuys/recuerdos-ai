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

use crate::bootstrap::config::AppConfig;
use crate::bootstrap::memories_wiring::Memories;
use crate::bootstrap::wiring::Identity;
use crate::identity::domain::scope::Scope;
use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::category::Category;
use crate::memories::domain::entity_graph::Relation;
use crate::memories::domain::memory::{Entity, MemorySource, NewMemory};
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
const K: i64 = 5;

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
    /// Named things the memory refers to. Projected into the graph on
    /// seeding (Task 7.3.6) so the graph leg has entities to seed a hop
    /// from — the same entities production's extraction pipeline attaches
    /// to a memory, supplied deterministically here instead of by a model.
    #[serde(default)]
    entities: Vec<SeedEntity>,
    /// Directed relations between this memory's entities
    /// (`subject —predicate→ object`). Projected into the graph alongside
    /// the entities (Task 7.3.6): they are the edges the `relational` cases
    /// hop over. In production these come from extraction (Task 7.3.2); the
    /// eval has no model, so the corpus carries them by hand — which is why
    /// they were seeded into the file back when the graph was still inert.
    #[serde(default)]
    relations: Vec<SeedRelation>,
}

#[derive(Debug, Deserialize)]
struct SeedEntity {
    name: String,
    #[serde(default)]
    kind: String,
}

/// A `subject —predicate→ object` edge asserted by a seed memory. Projected
/// into the graph on seeding so the `relational` cases have edges to hop.
#[derive(Debug, Deserialize)]
struct SeedRelation {
    subject: String,
    predicate: String,
    object: String,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    kind: String,
    query: String,
    #[serde(default)]
    expect: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    subcategories: Vec<String>,
    /// The correct answer is *nothing*: no memory in the corpus is about
    /// this query, so the recall must return an empty list at the
    /// configured `[retrieval].min_relevance` floor. Counted as passed
    /// only when nothing came back (reranking-plan.md Phase 0.3). Unlike
    /// the scored kinds, these cases *do* apply the floor — they measure
    /// exactly what an agent-facing read would show, floor included.
    #[serde(default)]
    expect_empty: bool,
    /// Reported everywhere but excluded from the numbers a committed
    /// baseline gates against, so a deliberately-failing regression case
    /// (reranking-plan.md Phase 0.5: `nl-*` and `negative` before the fix)
    /// documents the defect without reddening CI. Phase 2 removes the
    /// flag once the defects are fixed and the baseline re-recorded.
    #[serde(default)]
    informational: bool,
}

/// What one case scored.
#[derive(Debug, Clone)]
struct CaseResult {
    name: String,
    kind: String,
    /// Excluded from the gated summary a baseline is compared against;
    /// see `Case::informational`.
    informational: bool,
    /// Fraction of expected memories that appeared in the top K. For an
    /// `expect_empty` case this is 1.0 when nothing was returned, else 0.
    recall: f64,
    /// Whether the single best-ranked result was one of the expected ones.
    /// For an `expect_empty` case: whether the list was empty — the top
    /// result was "correct" exactly when there was no top result.
    top_hit: bool,
    /// 1 / (rank of the first expected memory found in the top K), None
    /// when none of them was found. Averaged into `Report::mrr`. For an
    /// `expect_empty` case: Some(1.0) when nothing was returned (the
    /// "answer" the case expected was found at the head of the empty list)
    /// and None when something was.
    reciprocal_rank: Option<f64>,
    /// Wall-clock milliseconds of the one `MemoryRecaller::execute` this
    /// case made — the number the reranker's latency cost (Phase 5–7) is
    /// measured against. Includes the embedder call: for a remote
    /// provider that is mostly network, so compare rerank-on against
    /// rerank-off on the same machine and embedder, never across them.
    latency_ms: f64,
    /// Whether this is an `expect_empty` case: excluded from the overall
    /// recall/precision/MRR means (a negative measures abstention, not
    /// ranking — the two must not move each other's gates), kept in
    /// `by_kind` and averaged into `abstention_rate`.
    expect_empty: bool,
    /// Expected memories that did not appear, for the report. For an
    /// `expect_empty` case that failed, the spurious results instead.
    missed: Vec<String>,
}

/// The machine-readable score, and the shape of `eval/baseline.json`.
///
/// Every field added after the first committed baseline carries
/// `#[serde(default)]` so old baselines still deserialise (invariant I-5
/// of reranking-plan.md), and `compare()` skips gating a field the old
/// baseline never recorded (see the "absent reads as zero" comment there).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Report {
    /// Mean recall@5 across the *scored* (non-`expect_empty`) cases,
    /// 0–100. Negatives are kept out of this mean on purpose: mixing
    /// "did we rank the right memory first" with "did we abstain
    /// correctly" would make the Phase 3 headroom figure and the Phase 7
    /// ship gate move whenever a negative flips, for reasons that have
    /// nothing to do with ranking.
    pub recall_at_k: f64,
    /// Fraction of *scored* cases whose top result was correct, 0–100.
    pub precision_at_1: f64,
    pub cases: usize,
    /// Mean recall@5 per `kind`, so a regression says *what* got worse.
    /// `expect_empty` kinds appear here too (pass rate), which is where
    /// negatives are read; the overall means above exclude them.
    pub by_kind: BTreeMap<String, f64>,
    /// Mean reciprocal rank of the first expected memory within the top K
    /// across the *scored* cases, 0–100 (reranking-plan.md Phase 0.2).
    /// Unlike recall@5, it notices "the right memory was in the top 5
    /// but at #4" — which is the defect the cross-encoder is proposed
    /// against.
    #[serde(default)]
    pub mrr: f64,
    /// Median / 95th-percentile wall-clock of a single recall, in
    /// milliseconds, across every case. Includes the embedder call (for
    /// a remote provider, mostly network), so it is only meaningful
    /// compared against a baseline recorded on the same embedder — which
    /// is what the `embedder` field lets `compare` check. The reranker's
    /// own cost (Phases 5–7) must be measured paired on/off, not out of
    /// this absolute.
    #[serde(default)]
    pub latency_ms_p50: f64,
    #[serde(default)]
    pub latency_ms_p95: f64,
    /// precision@1 per `kind`, so a ranking regression localises the same
    /// way `by_kind` localises a recall regression.
    #[serde(default)]
    pub precision_at_1_by_kind: BTreeMap<String, f64>,
    /// Fraction of `expect_empty` cases that returned nothing at the
    /// floor, 0–100; 0.0 when the run had no negatives. The negative
    /// cases' own number, kept off the ranking means above.
    #[serde(default)]
    pub abstention_rate: f64,
    /// The embedder that produced these scores (model id). A baseline
    /// recorded against one embedder and compared against another is
    /// apples to oranges (reranking-plan.md Phase 0.1); `compare` warns
    /// when the two differ. Empty in baselines recorded before the field
    /// existed, which suppresses the check rather than the run.
    #[serde(default)]
    pub embedder: String,
}

/// Runs the eval set and returns the *gated* report — the non-
/// `informational` subset that both `--baseline` and `--write-baseline`
/// act on, so a committed baseline can only ever record, and be compared
/// against, the cases the gate actually runs (reranking-plan.md Phase
/// 0.5: recording the full report would pin the deliberately-failing
/// cases into `baseline.json`). The full per-case picture, informational
/// kinds included, is printed by `print_report`. Phase 2 removes the
/// flags, collapsing the distinction.
///
/// The `config` decides which embedder the eval runs against: the eval
/// seeds its throwaway store itself, but otherwise assembles `Memories`
/// from this config — so `--config recuerdos-ai.toml` measures the
/// production embedding model, the production relevance floor, and
/// production retrieval tuning, not the built-in defaults. Measuring
/// ranking against a different embedder than production does not
/// transfer (reranking-plan.md Phase 0.1).
pub fn run(cases_path: &Path, config: &AppConfig) -> Result<Report> {
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
    validate(&set, cases_path)?;

    let (memories, context, _scratch) = seed(&set, config)?;

    // The scores mean nothing without knowing which model produced them:
    // baselines, floors and relevance all move with the embedder.
    let embedder = memories.embedder.model_id().to_string();
    println!(
        "embedder: {} ({} dimensions)\n",
        embedder,
        memories.embedder.dimensions()
    );

    // One untimed recall before timing any: the first call pays
    // connection setup (remote providers), index open and cache misses,
    // and with ~40 samples a single cold call lands straight in p95. The
    // store is a throwaway, so the touch this one makes costs nothing.
    let warmup = RecallQuery::new("warm up", 1)?;
    memories.recaller.execute(&context, &warmup)?;

    let mut results = Vec::with_capacity(set.cases.len());
    for case in &set.cases {
        results.push(score(&memories, &context, case)?);
    }

    print_report(&results);

    let mut gated = summarise(&gate(&results));
    gated.embedder = embedder;
    Ok(gated)
}

/// Reject cases that cannot be scored as written, at load time rather
/// than only in the committed-file parse test: an `expect`-less
/// non-negative case divides by zero and poisons every mean as NaN, and
/// an `expect_empty` case with expectations has contradicted itself.
fn validate(set: &EvalSet, path: &Path) -> Result<()> {
    let mut issues: Vec<String> = Vec::new();
    for case in &set.cases {
        match (case.expect_empty, case.expect.is_empty()) {
            (true, false) => issues.push(format!(
                "case {:?} is expect_empty but lists expectations — its expectation is emptiness",
                case.name
            )),
            (false, true) => issues.push(format!(
                "case {:?} lists no expectations and is not expect_empty — recall over zero expectations is NaN",
                case.name
            )),
            _ => {}
        }
    }
    if !issues.is_empty() {
        return Err(RaError::Validation(format!(
            "{} is invalid: {}",
            path.display(),
            issues.join("; ")
        )));
    }
    Ok(())
}

/// The subset a committed baseline gates against: everything not marked
/// `informational`, so a case that documents a known defect cannot
/// redden the pipeline (reranking-plan.md Phase 0.5).
fn gate(results: &[CaseResult]) -> Vec<CaseResult> {
    results
        .iter()
        .filter(|r| !r.informational)
        .cloned()
        .collect()
}

/// Builds a throwaway instance and fills it with the corpus.
///
/// The `TempDir` comes back so the caller holds it: dropping it deletes
/// the database and the tantivy indexes out from under the run.
///
/// The instance is assembled from the caller's `config` — embedder,
/// relevance floor, ranker tuning and all — with exactly two overrides:
/// the storage path, so the run can never touch a real store, and the
/// graph (see below).
fn seed(set: &EvalSet, config: &AppConfig) -> Result<(Memories, UserContext, tempfile::TempDir)> {
    let scratch = tempfile::tempdir()
        .map_err(|e| RaError::Internal(format!("could not create a scratch directory: {e}")))?;

    let mut config = config.clone();
    config.storage.path = scratch.path().to_string_lossy().to_string();
    // The eval measures the graph leg's contribution, so it runs with the
    // graph on regardless of the shipped default — that is the whole point
    // of the `relational` cases and the entities/relations the corpus
    // carries. Whether `[graph].enabled` *ships* on is a separate decision,
    // made from these numbers (Task 7.3.6), not read back from here.
    config.graph.enabled = true;

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

    // The graph is present because we turned it on above; a missing one
    // here would be a wiring bug, not a config choice, so unwrap loudly.
    let graph = memories
        .graph
        .as_ref()
        .expect("the eval enables the graph, so Memories must have built one");

    for seed in &set.memories {
        let entities: Vec<Entity> = seed
            .entities
            .iter()
            .map(|e| Entity {
                name: e.name.clone(),
                kind: e.kind.clone(),
            })
            .collect();

        let stored = memories.saver.execute(
            &context,
            NewMemory {
                content: seed.content.clone(),
                category: Category::parse(&seed.category)?,
                subcategory: seed.subcategory.clone(),
                tags: seed.tags.clone(),
                entities: entities.clone(),
                confidence: 1.0,
                source: MemorySource {
                    client: Some("eval".to_string()),
                    session_id: None,
                },
                expires_at: None,
            },
            "eval",
        )?;

        // Project the memory into the graph exactly as the reconciler does
        // on a real ingest (`MemoryReconciler::store`), minus the model:
        // its entities and relations, valid from the moment it was learned.
        // Without this the graph would know the corpus's entities but have
        // no edges, and every `relational` hop would dead-end at its seed.
        let relations: Vec<Relation> = seed
            .relations
            .iter()
            .map(|r| Relation {
                subject: r.subject.clone(),
                predicate: r.predicate.clone(),
                object: r.object.clone(),
            })
            .collect();

        graph.record(
            &context,
            stored.id(),
            &entities,
            &relations,
            stored.created_at(),
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

    let mut query = RecallQuery::new(&case.query, K)?
        .with_categories(categories)
        .with_tags(case.tags.clone())
        .with_subcategories(case.subcategories.clone());
    // A negative case asks "would an agent see anything here?", so it runs
    // behind the same floor every agent-facing read applies. The scored
    // kinds run floorless, as they always have: they measure what the
    // indexes ranked, and dropping weak results would hide near-misses the
    // report exists to show.
    if case.expect_empty {
        query = query.with_min_relevance(memories.min_relevance);
    }

    let started = std::time::Instant::now();
    let hits = memories.recaller.execute(context, &query)?;
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    let returned: Vec<&str> = hits.iter().map(|hit| hit.memory.content()).collect();

    // Ranker-tuning aid (added in Task 7.3.7): with RA_EVAL_DEBUG set, dump
    // the top few hits and how each leg ranked them, so a precision or recall
    // move can be traced to the exact memory that took a rank and why — which
    // is how the graph rank-weight was tuned. Off by default; costs nothing.
    if std::env::var("RA_EVAL_DEBUG").is_ok() {
        eprintln!("\n[{}] {}", case.kind, case.name);
        for (i, hit) in hits.iter().take(3).enumerate() {
            let d = hit.match_detail;
            eprintln!(
                "  {}. v={:?} k={:?} g={:?} relevance={:.3} score={:.5}  {}",
                i + 1,
                d.vector_rank,
                d.bm25_rank,
                d.graph_rank,
                hit.relevance,
                hit.score,
                truncate(hit.memory.content(), 60),
            );
        }
    }

    if case.expect_empty {
        // The pass condition is its own function: it is what a negative
        // case means, and the DoD pins it. Its recall/top_hit carry the
        // pass bit for the *kind-level* lines and `abstention_rate`; the
        // overall ranking means skip this case entirely.
        let passed = nothing_returned_passes(returned.len());
        return Ok(CaseResult {
            name: case.name.clone(),
            kind: case.kind.clone(),
            informational: case.informational,
            recall: if passed { 1.0 } else { 0.0 },
            top_hit: passed,
            reciprocal_rank: None,
            latency_ms,
            expect_empty: true,
            missed: if passed {
                Vec::new()
            } else {
                vec![format!(
                    "(expected no results at the {:?} floor; got {})",
                    memories.min_relevance,
                    returned.len()
                )]
            },
        });
    }

    let missed: Vec<String> = case
        .expect
        .iter()
        .filter(|wanted| !returned.iter().any(|got| got == &wanted.as_str()))
        .cloned()
        .collect();

    let found = case.expect.len() - missed.len();

    // The first expected memory's rank, if any: reciprocal rank is 1 over
    // how far down the *best* answer was buried.
    let reciprocal_rank = returned
        .iter()
        .position(|got| case.expect.iter().any(|wanted| wanted == got))
        .map(|index| 1.0 / (index as f64 + 1.0));

    Ok(CaseResult {
        name: case.name.clone(),
        kind: case.kind.clone(),
        informational: case.informational,
        recall: found as f64 / case.expect.len() as f64,
        top_hit: returned
            .first()
            .is_some_and(|top| case.expect.iter().any(|wanted| wanted == top)),
        reciprocal_rank,
        latency_ms,
        expect_empty: false,
        missed,
    })
}

/// Whether an `expect_empty` case passed: the whole point of a negative
/// case is that a query with no answer shows an agent *nothing*, so any
/// returned result fails it. Kept as a named function because it is the
/// definition of the `expect_empty` kind and the DoD pins its behaviour.
fn nothing_returned_passes(returned: usize) -> bool {
    returned == 0
}

fn summarise(results: &[CaseResult]) -> Report {
    let mean = |values: &[f64]| -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        values.iter().sum::<f64>() / values.len() as f64 * 100.0
    };

    // The ranking means cover the scored cases only (see `Report`);
    // negatives get `abstention_rate`, and both appear in `by_kind`.
    let scored: Vec<&CaseResult> = results.iter().filter(|r| !r.expect_empty).collect();

    let mut recall_by_kind: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut precision_by_kind: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for result in results {
        recall_by_kind
            .entry(result.kind.clone())
            .or_default()
            .push(result.recall);
        precision_by_kind
            .entry(result.kind.clone())
            .or_default()
            .push(if result.top_hit { 1.0 } else { 0.0 });
    }

    let latencies: Vec<f64> = results.iter().map(|r| r.latency_ms).collect();
    let abstentions: Vec<f64> = results
        .iter()
        .filter(|r| r.expect_empty)
        .map(|r| r.recall)
        .collect();

    Report {
        recall_at_k: mean(&scored.iter().map(|r| r.recall).collect::<Vec<_>>()),
        precision_at_1: mean(
            &scored
                .iter()
                .map(|r| if r.top_hit { 1.0 } else { 0.0 })
                .collect::<Vec<_>>(),
        ),
        cases: results.len(),
        by_kind: recall_by_kind
            .into_iter()
            .map(|(kind, values)| (kind, mean(&values)))
            .collect(),
        mrr: mean(
            &scored
                .iter()
                .map(|r| r.reciprocal_rank.unwrap_or(0.0))
                .collect::<Vec<_>>(),
        ),
        latency_ms_p50: percentile(&latencies, 50.0).unwrap_or(0.0),
        latency_ms_p95: percentile(&latencies, 95.0).unwrap_or(0.0),
        precision_at_1_by_kind: precision_by_kind
            .into_iter()
            .map(|(kind, values)| (kind, mean(&values)))
            .collect(),
        abstention_rate: mean(&abstentions),
        embedder: String::new(),
    }
}

/// Nearest-rank percentile over a sample, `p` in 0–100: the value at
/// rank `ceil(p% of n)`, which makes p50 of `[1, 1, 2, 2]` the second
/// `1` rather than an interpolation between the two — a latency figure a
/// human can reproduce from the sorted list. `None` for an empty sample,
/// so a summary line can say so instead of claiming a `0 ms` median.
fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len() as f64;
    let rank = (p / 100.0 * n).ceil().clamp(1.0, n) as usize;
    Some(sorted[rank - 1])
}

fn print_report(results: &[CaseResult]) {
    println!("{:<42} {:>16} {:>7}  top", "case", "kind", "recall");
    println!("{}", "─".repeat(76));

    for result in results {
        println!(
            "{:<42} {:>16} {:>6.0}% {:>4}{}",
            truncate(&result.name, 42),
            result.kind,
            result.recall * 100.0,
            if result.top_hit { "✓" } else { "·" },
            if result.informational { " *" } else { "" },
        );
        // Naming what was missed is the difference between "the score
        // dropped" and knowing which change to look at.
        for missed in &result.missed {
            println!("    missed: {}", truncate(missed, 68));
        }
    }

    let report = summarise(results);
    println!("{}", "─".repeat(76));
    let scored_cases = results.iter().filter(|r| !r.expect_empty).count();
    println!(
        "recall@{K}: {:.1}%   precision@1: {:.1}%   MRR: {:.1}%   latency p50: {:.1} ms p95: {:.1} ms   ({} scored cases, {} negative)",
        report.recall_at_k,
        report.precision_at_1,
        report.mrr,
        report.latency_ms_p50,
        report.latency_ms_p95,
        scored_cases,
        report.cases - scored_cases,
    );
    if report.cases > scored_cases {
        // The negatives' own number, printed where a reader looks for
        // them rather than hidden inside the ranking means above.
        println!(
            "abstention (negatives returning nothing at the floor): {:.1}%",
            report.abstention_rate
        );
    }
    let informational = results.iter().filter(|r| r.informational).count();
    if informational > 0 {
        // Say it out loud: these numbers include cases the baseline does
        // not gate on, and a reader comparing against `baseline.json`
        // must know which subset they are looking at.
        println!("  ({informational} marked * informational — excluded from the baseline gate)");
    }
    for (kind, score) in &report.by_kind {
        let precision = report.precision_at_1_by_kind.get(kind).unwrap_or(&0.0);
        println!("  {kind:>16}: recall {score:.1}%   p@1 {precision:.1}%");
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
///
/// The caller passes the *gated* report (reranking-plan.md Phase 0.5),
/// so informational cases — deliberately-failing regression cases the
/// baseline has never scored — can never redden the pipeline.
///
/// `precision_at_1` and `mrr` are gated too (Phase 0.2): recall says the
/// right memory was in the list, these say it was *first*, which is what
/// the cross-encoder is proposed to fix. An old baseline recorded before
/// a field existed deserialises it as `0.0`; a `0.0` there means "never
/// measured", not "the floor was reached", so an absent-and-zero baseline
/// value skips that gate (and says so). A field old baselines always
/// recorded — `precision_at_1` — is therefore still gated against them.
pub fn compare(report: &Report, baseline_path: &Path, max_drop: f64) -> Result<()> {
    let raw = std::fs::read_to_string(baseline_path).map_err(|e| {
        RaError::Validation(format!("could not read {}: {e}", baseline_path.display()))
    })?;
    let baseline: Report = serde_json::from_str(&raw).map_err(|e| {
        RaError::Validation(format!("{} is not valid: {e}", baseline_path.display()))
    })?;

    // A baseline recorded against one embedder and gated against another
    // is apples to oranges (reranking-plan.md Phase 0.1): the committed
    // baseline is bge-small, CI runs `just eval-native` with no
    // `--config`, and a production-like run is an ad-hoc measurement.
    // Warn loudly rather than fail — cross-embedder comparisons are
    // sometimes exactly what the operator means to do — but never let a
    // silent mismatch pass as a verdict.
    if baseline.embedder.is_empty() {
        println!(
            "note: the baseline records no embedder (it predates the field); \
             re-record it to enable the embedder-match check"
        );
    } else if baseline.embedder != report.embedder {
        println!(
            "warning: baseline recorded with {:?}, this run used {:?} — these numbers \
             are not comparable",
            baseline.embedder, report.embedder
        );
    }

    let mut failures: Vec<String> = Vec::new();

    let drop = baseline.recall_at_k - report.recall_at_k;
    println!(
        "\nbaseline recall@{K}: {:.1}%   now: {:.1}%   change: {:+.1} points",
        baseline.recall_at_k, report.recall_at_k, -drop
    );
    if drop > max_drop {
        failures.push(format!(
            "recall@{K} dropped {drop:.1} points (limit {max_drop:.1})"
        ));
    }

    for (metric, before, after) in [
        (
            "precision@1",
            baseline.precision_at_1,
            report.precision_at_1,
        ),
        ("MRR", baseline.mrr, report.mrr),
    ] {
        if before == 0.0 {
            // An old baseline never recorded MRR; treating the deserialised
            // zero as a score would either skip a real gate forever or fail
            // an honest zero, so the skip is explicit and printed.
            println!(
                "baseline {metric}: not recorded — gate skipped (re-record the baseline to enable it)"
            );
            continue;
        }
        let drop = before - after;
        println!(
            "baseline {metric}: {before:.1}%   now: {after:.1}%   change: {:+.1} points",
            -drop
        );
        if drop > max_drop {
            failures.push(format!(
                "{metric} dropped {drop:.1} points (limit {max_drop:.1})"
            ));
        }
    }

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

    if !failures.is_empty() {
        return Err(RaError::Validation(format!(
            "{}. Either the change made retrieval worse, or the eval set needs \
             updating — decide which, and if it is the latter, re-record the \
             baseline with `--write-baseline`.",
            failures.join("; ")
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
            informational: false,
            recall,
            top_hit,
            // A case with a top hit has its expected memory at rank 1.
            reciprocal_rank: if top_hit { Some(1.0) } else { None },
            latency_ms: 10.0,
            expect_empty: false,
            missed: vec![],
        }
    }

    /// A negative case's scored stand-in: pass/fail lives entirely in the
    /// recall bit, as `score` writes it.
    fn negative(passed: bool) -> CaseResult {
        CaseResult {
            expect_empty: true,
            recall: if passed { 1.0 } else { 0.0 },
            top_hit: passed,
            ..result("negative", 0.0, false)
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
        assert_eq!(report.precision_at_1_by_kind["paraphrase"], 100.0);
        assert_eq!(report.precision_at_1_by_kind["exact-token"], 0.0);
        assert_eq!(report.recall_at_k, 50.0, "the mean hides the collapse");
    }

    #[test]
    fn mrr_credits_rank_one_and_punishes_a_buried_answer() {
        // The number the reranker exists for: both cases found the memory,
        // recall says 100 for both, MRR separates rank 1 from rank 3.
        let first = CaseResult {
            reciprocal_rank: Some(1.0),
            ..result("paraphrase", 1.0, true)
        };
        let third = CaseResult {
            reciprocal_rank: Some(1.0 / 3.0),
            ..result("paraphrase", 1.0, false)
        };
        let missing = CaseResult {
            reciprocal_rank: None,
            ..result("paraphrase", 0.0, false)
        };

        let report = summarise(&[first, third, missing]);
        let expected = (1.0 + 1.0 / 3.0 + 0.0) / 3.0 * 100.0;
        assert!((report.mrr - expected).abs() < 1e-9, "{}", report.mrr);
    }

    #[test]
    fn percentiles_never_invent_a_number_for_no_data() {
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(percentile(&[42.0], 95.0), Some(42.0));
        // Nearest rank: ties are values in the sample, never interpolated.
        assert_eq!(percentile(&[2.0, 1.0, 1.0, 2.0], 50.0), Some(1.0));
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 95.0), Some(4.0));
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 0.0), Some(1.0));
    }

    #[test]
    fn a_negative_case_passes_only_on_an_empty_result() {
        assert!(nothing_returned_passes(0));
        assert!(!nothing_returned_passes(1));
        // The defect this kind exists to reproduce: the stopword query
        // returned five confident results — every one a failure.
        assert!(!nothing_returned_passes(5));
    }

    #[test]
    fn negatives_move_abstention_never_the_ranking_means() {
        // Must-fix from review: a negative flipping must not move recall,
        // precision@1 or MRR — otherwise the Phase 3 headroom and the
        // Phase 7 ship gate read abstention as ranking. Both reports here
        // have the same scored case; the negative differs.
        let scored = result("paraphrase", 0.5, false);
        let without = summarise(std::slice::from_ref(&scored));
        let with_pass = summarise(&[scored.clone(), negative(true)]);
        let with_fail = summarise(&[scored, negative(false)]);

        assert_eq!(without.recall_at_k, with_pass.recall_at_k);
        assert_eq!(without.recall_at_k, with_fail.recall_at_k);
        assert_eq!(without.precision_at_1, with_pass.precision_at_1);
        assert_eq!(without.precision_at_1, with_fail.precision_at_1);
        assert_eq!(without.mrr, with_pass.mrr);
        assert_eq!(without.mrr, with_fail.mrr);

        assert_eq!(with_pass.abstention_rate, 100.0);
        assert_eq!(with_fail.abstention_rate, 0.0);
        // Still visible per kind, which is where a negative is read.
        assert_eq!(with_pass.by_kind["negative"], 100.0);
        assert_eq!(with_fail.by_kind["negative"], 0.0);
    }

    #[test]
    fn a_run_of_pure_negatives_reports_no_ranking_mean_as_a_fake_zero() {
        // A cases file of only negatives: means over the empty scored set
        // are 0 by `mean`'s convention, and abstention carries the signal.
        let report = summarise(&[negative(true), negative(true)]);
        assert_eq!(report.recall_at_k, 0.0);
        assert_eq!(report.abstention_rate, 100.0);
        assert_eq!(report.cases, 2);
    }

    fn case(name: &str, expect: Vec<&str>, expect_empty: bool) -> Case {
        Case {
            name: name.to_string(),
            kind: "k".to_string(),
            query: "q".to_string(),
            expect: expect.iter().map(|s| s.to_string()).collect(),
            categories: vec![],
            tags: vec![],
            subcategories: vec![],
            expect_empty,
            informational: false,
        }
    }

    #[test]
    fn validate_rejects_both_kinds_of_unscorable_case() {
        // The `expect` field gained a serde default, so these reach run()
        // rather than the parser; they must die there, not as NaN means.
        let good = EvalSet {
            memories: vec![],
            cases: vec![
                case("scored", vec!["m"], false),
                case("negative", vec![], true),
            ],
        };
        validate(&good, Path::new("x.toml")).expect("valid set");

        let bad = EvalSet {
            memories: vec![],
            cases: vec![
                case("no expect", Vec::new(), false),
                case("contradictory", vec!["m"], true),
            ],
        };
        let error = validate(&bad, Path::new("x.toml")).unwrap_err().to_string();
        assert!(error.contains("\"no expect\""), "{error}");
        assert!(error.contains("NaN"), "{error}");
        assert!(error.contains("\"contradictory\""), "{error}");
    }

    #[test]
    fn write_baseline_records_the_gated_subset_not_the_full_report() {
        // Must-fix from review: `--write-baseline` writes outcome.gated.
        // Prove the gated report this feeds is what a mixed run produces:
        // informational cases never appear in its case count or kinds, so
        // the deliberate-defect cases cannot be pinned into baseline.json.
        let mixed = vec![
            result("paraphrase", 1.0, true),
            CaseResult {
                informational: true,
                ..result("nl-question", 0.0, false)
            },
            CaseResult {
                informational: true,
                ..negative(false)
            },
        ];
        let report = summarise(&gate(&mixed));

        let file = tempfile::NamedTempFile::new().unwrap();
        write_baseline(&report, file.path()).unwrap();
        let raw = std::fs::read_to_string(file.path()).unwrap();
        let back: Report = serde_json::from_str(&raw).unwrap();

        assert_eq!(back.cases, 1);
        assert_eq!(back.recall_at_k, 100.0);
        assert!(!back.by_kind.contains_key("nl-question"));
        assert!(!back.by_kind.contains_key("negative"));
        // The full report, by contrast, sees all three — the shape that
        // would have poisoned the baseline had it been written.
        let full = summarise(&mixed);
        assert_eq!(full.cases, 3);
        assert!(full.recall_at_k < 100.0);
    }

    #[test]
    fn compare_warns_across_embedders_without_failing() {
        // The warning is printed, not asserted here (no output capture in
        // this harness), but the gate must keep returning Ok across a
        // mismatch — a cross-embedder comparison is sometimes intended.
        let baseline = tempfile::NamedTempFile::new().unwrap();
        write_baseline(
            &Report {
                recall_at_k: 90.0,
                embedder: "bge-small-en-v1.5".to_string(),
                ..Report::default()
            },
            baseline.path(),
        )
        .unwrap();
        let other = Report {
            recall_at_k: 90.0,
            embedder: "gemini-embedding-2".to_string(),
            ..Report::default()
        };
        assert!(compare(&other, baseline.path(), 5.0).is_ok());
    }

    #[test]
    #[ignore = "requires the ONNX model; run with --ignored"]
    fn the_config_passed_to_seed_reaches_the_wired_instance() {
        // Task 0.1's wiring, as opposed to the eval-set parse test (which
        // only checks the committed file): the config decides the embedder
        // and the floor, exactly what compare() checks the baseline
        // against. Model-dependent, so ignored like the fastembed tests
        // and reading the same cache path.
        let cache = std::env::var("RECUERDOS_AI_MODEL_CACHE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("/models"));
        let config = AppConfig {
            embeddings: crate::bootstrap::config::EmbeddingsConfig {
                cache_dir: cache.to_string_lossy().to_string(),
                ..Default::default()
            },
            retrieval: crate::bootstrap::config::RetrievalConfig {
                min_relevance: 0.77,
                ..Default::default()
            },
            ..AppConfig::default()
        };
        let set = EvalSet {
            memories: vec![],
            cases: vec![],
        };

        let (memories, _context, _scratch) = seed(&set, &config).expect("seed with config");
        assert_eq!(memories.embedder.model_id(), "bge-small-en-v1.5");
        assert_eq!(
            memories.min_relevance, 0.77,
            "the config's floor must reach the instance the eval scores with"
        );
    }

    #[test]
    fn informational_cases_are_excluded_from_the_gate() {
        let known = result("paraphrase", 1.0, true);
        let regression = CaseResult {
            informational: true,
            recall: 0.0,
            top_hit: false,
            reciprocal_rank: None,
            ..result("nl-question", 0.0, false)
        };

        let all = summarise(&[known.clone(), regression.clone()]);
        let gated = summarise(&gate(&[known, regression]));

        assert_eq!(all.cases, 2);
        assert_eq!(gated.cases, 1);
        assert_eq!(gated.recall_at_k, 100.0);
        assert_eq!(gated.precision_at_1, 100.0);
        assert!(all.recall_at_k < 100.0);
    }

    #[test]
    fn the_committed_old_baseline_still_deserialises() {
        // Invariant I-5: `eval/baseline.json` predates mrr/latency/
        // precision_at_1_by_kind, and `eval --baseline` must keep running.
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("eval/baseline.json"),
        )
        .expect("eval/baseline.json should exist");

        let baseline: Report =
            serde_json::from_str(&raw).expect("the committed baseline must still deserialise");
        assert!(baseline.recall_at_k > 0.0);
        assert_eq!(baseline.mrr, 0.0, "absent in an old baseline");
        assert_eq!(baseline.latency_ms_p50, 0.0);
        assert!(baseline.precision_at_1_by_kind.is_empty());
    }

    #[test]
    fn an_old_baseline_without_mrr_skips_that_gate_rather_than_failing_it() {
        // MRR of 0.0 recorded as 0.0, and an mrr-less baseline over a
        // non-zero report: "never measured", so no verdict either way.
        let baseline = tempfile::NamedTempFile::new().unwrap();
        write_baseline(
            &Report {
                recall_at_k: 80.0,
                precision_at_1: 70.0,
                ..Report::default()
            },
            baseline.path(),
        )
        .unwrap();

        let measured = Report {
            recall_at_k: 80.0,
            precision_at_1: 70.0,
            mrr: 55.0,
            ..Report::default()
        };
        assert!(
            compare(&measured, baseline.path(), 5.0).is_ok(),
            "an unrecorded metric must not fail the gate"
        );
    }

    #[test]
    fn a_precision_or_mrr_drop_fails_the_gate_once_recorded() {
        let baseline = tempfile::NamedTempFile::new().unwrap();
        write_baseline(
            &Report {
                recall_at_k: 90.0,
                precision_at_1: 62.5,
                mrr: 75.0,
                ..Report::default()
            },
            baseline.path(),
        )
        .unwrap();

        // Recall holds, ranking collapses — exactly the regression the old
        // recall-only gate let through, and the reason Phase 0.2 exists.
        let re_ranked_worse = Report {
            recall_at_k: 90.0,
            precision_at_1: 50.0,
            mrr: 60.0,
            ..Report::default()
        };
        let error = compare(&re_ranked_worse, baseline.path(), 5.0).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("precision@1 dropped 12.5"), "{message}");
        assert!(message.contains("MRR dropped 15.0"), "{message}");

        // Within the threshold, tolerated like every other metric.
        let slight = Report {
            recall_at_k: 90.0,
            precision_at_1: 59.0,
            mrr: 71.0,
            ..Report::default()
        };
        assert!(compare(&slight, baseline.path(), 5.0).is_ok());
    }

    #[test]
    fn a_small_drop_is_tolerated_and_a_large_one_is_not() {
        let baseline = tempfile::NamedTempFile::new().unwrap();
        write_baseline(
            &Report {
                recall_at_k: 90.0,
                precision_at_1: 80.0,
                ..Report::default()
            },
            baseline.path(),
        )
        .unwrap();

        let slightly_worse = Report {
            recall_at_k: 87.0,
            precision_at_1: 80.0,
            ..Report::default()
        };
        assert!(
            compare(&slightly_worse, baseline.path(), 5.0).is_ok(),
            "a gate that fails on noise gets disabled within a week"
        );

        let much_worse = Report {
            recall_at_k: 80.0,
            precision_at_1: 80.0,
            ..Report::default()
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
                ..Report::default()
            },
            baseline.path(),
        )
        .unwrap();

        let better = Report {
            recall_at_k: 95.0,
            precision_at_1: 90.0,
            mrr: 90.0,
            ..Report::default()
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
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("eval/cases.toml");
        validate(&set, &path).expect("the committed set must pass its own validation");

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

        // Every relation endpoint must be a declared entity of the same
        // memory. This is the anchoring rule Task 7.3.2 enforces in the
        // pipeline; asserting it over the hand-written corpus catches a
        // typo (`Fly.io` vs `Flyio`) that would otherwise silently leave a
        // relational case with no graph to walk.
        for memory in &set.memories {
            let names: Vec<&str> = memory.entities.iter().map(|e| e.name.as_str()).collect();
            for relation in &memory.relations {
                for endpoint in [&relation.subject, &relation.object] {
                    assert!(
                        names.contains(&endpoint.as_str()),
                        "memory {:?} asserts a relation over {endpoint:?}, which is not one of \
                         its declared entities {names:?}",
                        memory.content
                    );
                }
            }
        }

        // The Phase 0 regression kinds exist and are marked informational
        // while they still fail (reranking-plan.md Phase 0.5). Phase 2
        // fixes the defects, clears the flags, and this assertion clears
        // with them — hence it asserts presence, not the absence of the
        // flags, and additionally that every negative case exists with
        // `expect_empty`.
        for kind in ["nl-question", "nl-category", "negative"] {
            let cases: Vec<&Case> = set.cases.iter().filter(|c| c.kind == kind).collect();
            assert!(
                !cases.is_empty(),
                "the {kind} kind from reranking-plan.md Phase 0.3 is missing"
            );
            assert!(
                cases.iter().all(|c| c.informational),
                "{kind} cases fail on purpose until Phases 1–2 land; they must be informational"
            );
        }
        assert!(
            set.cases
                .iter()
                .filter(|c| c.kind == "negative")
                .all(|c| c.expect_empty),
            "a negative case that expects memories is not a negative case"
        );
    }
}
