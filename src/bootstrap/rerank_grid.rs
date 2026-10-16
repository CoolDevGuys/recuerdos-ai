//! `recuerdos-ai eval --grid` — the Phase 7 tuning grid (reranking-plan.md
//! task 7.2): model × strategy × depth × weight, scored against the eval
//! set, printing the table §12 records.
//!
//! # Why the grid lives inside `eval` rather than beside it
//!
//! The plan asks for a checked-in, reproducible script. The honest shape
//! of that is one seeded corpus measured many times, not one process per
//! cell: the store, the embedder session and the keyword indexes are
//! identical across cells by construction (the recaller clones the audit
//! already relies on), so a cell's numbers differ from the reference *only*
//! in the second stage being measured. Per-process runs would also pay the
//! ~100-memory seed and the model load per cell — 45 cells of pure noise
//! budget for the price of one measurement.
//!
//! Two latency figures are printed per cell and they answer different
//! questions:
//!
//! * `p50/p95` wrap the whole recall (embedder included), exactly like
//!   `eval`'s summary line — so a cell is comparable to the committed
//!   baseline and to `just eval-native`. The ship gate's *added* latency
//!   is this column's delta against the `off` row, same machine, same run.
//! * `rerank p50/p95` wrap the `Reranker` boundary alone (Phase 6's
//!   [`MeteredReranker`]) — the number `timeout_ms` has to contain, and
//!   the one that scales with `depth`.
//!
//! The grid runs with **no rerank deadline** for the same reason the audit
//! does (measuring what the model costs is half the job), and says so in
//! its header. The configured `timeout_ms` is printed beside the
//! percentiles it must contain.
//!
//! # What the grid does not decide
//!
//! It prints measurements; the ship gate in §7 decides. The `off` row is
//! emitted first and every Δ column reads against it, so the table pastes
//! into §12 carrying its own reference — a grid row without its Δp@1 is a
//! number nobody can act on.

use crate::bootstrap::config::AppConfig;
use crate::bootstrap::eval::{self, Case, percentile};
use crate::bootstrap::memories_wiring::Memories;
use crate::bootstrap::rerank_audit::{Call, MeteredReranker};
use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::recall_query::RecallQuery;
use crate::memories::domain::recall_ranker::RerankStrategy;
use crate::memories::domain::reranker::{RerankCandidate, RerankMode, Reranker};
use crate::providers::infrastructure::rerank::fastembed_reranker::FastembedReranker;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The grid's axes, as parsed from the CLI flags. The defaults are the
/// plan's table (task 7.2); `--grid-models` is deliberately *not* the full
/// built-in list — three of the four models need a gigabyte of weights on
/// disk and ~1.5 GiB of RAM per session (Phase 7's measurements), so the
/// caller names the set they can afford.
#[derive(Debug, Clone)]
pub struct GridOptions {
    pub models: Vec<String>,
    /// `"rrf"` and/or `"replace"`, in report order.
    pub strategies: Vec<String>,
    pub depths: Vec<usize>,
    /// RRF weights; ignored by `replace` rows, which have no weight.
    pub weights: Vec<f32>,
}

const STRATEGIES: &[&str] = &["rrf", "replace"];

impl GridOptions {
    /// Parses the comma lists the CLI passes. Errors name the offending
    /// token rather than the whole flag: this is typed at a terminal
    /// under measurement pressure, not edited in a config file.
    pub fn parse(models: &str, strategies: &str, depths: &str, weights: &str) -> Result<Self> {
        let split = |list: &str| -> Vec<String> {
            list.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        };

        let models = split(models);
        if models.is_empty() {
            return Err(RaError::Validation(
                "--grid-models is empty — name at least one rerank model".to_string(),
            ));
        }
        let strategies = split(strategies);
        if strategies.is_empty() {
            return Err(RaError::Validation(
                "--grid-strategies is empty — name at least one of {rrf, replace}".to_string(),
            ));
        }
        for strategy in &strategies {
            if !STRATEGIES.contains(&strategy.as_str()) {
                return Err(RaError::Validation(format!(
                    "--grid-strategies contains {strategy:?} — the grid knows {STRATEGIES:?}"
                )));
            }
        }
        let depths = split(depths)
            .into_iter()
            .map(|depth| {
                depth
                    .parse::<usize>()
                    .map_err(|_| RaError::Validation(format!("--grid-depths contains {depth:?}")))
            })
            .collect::<Result<Vec<_>>>()?;
        if depths.is_empty() {
            return Err(RaError::Validation(
                "--grid-depths is empty — the grid needs at least one depth".to_string(),
            ));
        }
        if depths.iter().any(|depth| *depth < 2) {
            return Err(RaError::Validation(
                "--grid-depths below 2 cannot reorder anything (the rerank leg skips pools under 2)"
                    .to_string(),
            ));
        }
        let weights = split(weights)
            .into_iter()
            .map(|weight| {
                weight
                    .parse::<f32>()
                    .map_err(|_| RaError::Validation(format!("--grid-weights contains {weight:?}")))
            })
            .collect::<Result<Vec<_>>>()?;
        if weights.is_empty() {
            return Err(RaError::Validation(
                "--grid-weights is empty — rrf rows need at least one weight".to_string(),
            ));
        }
        if weights
            .iter()
            .any(|weight| *weight < 0.0 || !weight.is_finite())
        {
            return Err(RaError::Validation(
                "--grid-weights holds a negative or non-finite value".to_string(),
            ));
        }

        Ok(Self {
            models,
            strategies,
            depths,
            weights,
        })
    }

    /// The (strategy, depth, weight) rows for one model: weight varies the
    /// `rrf` rows only, exactly as the plan's table defines the axes.
    fn cells(&self) -> Vec<(RerankStrategy, usize)> {
        let mut cells = Vec::new();
        for strategy in &self.strategies {
            for depth in self.depths.iter().copied() {
                if strategy == "replace" {
                    cells.push((RerankStrategy::Replace, depth));
                } else {
                    for weight in self.weights.iter().copied() {
                        cells.push((RerankStrategy::Rrf { weight }, depth));
                    }
                }
            }
        }
        cells
    }
}

/// One table row: the cell's identity, the report metrics
/// [`eval::summarise`] defines for it, and the stage-only latencies.
///
/// `Serialize` is the `--json` shape: every number the ship gate reads
/// (overall and per-kind p@1, both latency pairs, abstention) is here,
/// so a recorded grid artifact re-decides the gate without a rerun.
#[derive(Debug, Clone, Serialize)]
pub struct GridCellReport {
    /// `"off"` for the reference row.
    pub model: String,
    /// `"off"`, `"rrf"` or `"replace"`.
    pub strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f32>,
    pub depth: usize,
    pub recall_at_k: f64,
    pub precision_at_1: f64,
    pub mrr: f64,
    pub abstention_rate: f64,
    /// Whole-recall latency, like `eval`'s summary line — Δ against the
    /// `off` row is the ship gate's "added latency".
    pub latency_ms_p50: f64,
    pub latency_ms_p95: f64,
    /// [`Reranker`] boundary only; absent on the `off` row, which ran none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_ms_p50: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_ms_p95: Option<f64>,
    /// The head actually judged at the peak (`depth` is the ceiling; the
    /// fused pool often is the real bound, and latency scales with this).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_candidates_high: Option<usize>,
    pub precision_at_1_by_kind: BTreeMap<String, f64>,
}

/// The whole grid: what was measured, then every row in plan order
/// (the `off` reference first, then per model: strategy → depth → weight).
#[derive(Debug, Serialize)]
pub struct GridReport {
    pub embedder: String,
    pub corpus: usize,
    pub scored_cases: usize,
    pub negative_cases: usize,
    /// `[rerank].timeout_ms`: what a live request would enforce. The grid
    /// itself runs deadlineless (module docs) — this is the number the
    /// `rerank_ms_p95` column has to fit under.
    pub live_timeout_ms: u64,
    pub cells: Vec<GridCellReport>,
}

/// Runs one cell: clone the seeded pipeline, switch on the second stage
/// being measured, replay every case untracked, summarise.
///
/// The base pipeline is `memories.recaller` as assembled — `reranker:
/// None` still routes through an explicit `with_second_stage(None, Off)`
/// rather than a bare clone, so a grid run under a config that already
/// enables the stage measures the *pre-rerank* pipeline as its reference.
pub(crate) fn measure_cell(
    cases: &[Case],
    context: &UserContext,
    memories: &Memories,
    model: &str,
    strategy: RerankStrategy,
    depth: usize,
    reranker: Option<Arc<MeteredReranker>>,
) -> Result<GridCellReport> {
    let has_stage = reranker.is_some();
    let stage = reranker
        .as_ref()
        .map(|metered| Arc::clone(metered) as Arc<dyn Reranker>);
    let recaller = (*memories.recaller)
        .clone()
        .with_second_stage(
            stage,
            if has_stage {
                RerankMode::On
            } else {
                RerankMode::Off
            },
            depth,
            Duration::ZERO,
        )
        .with_rerank_strategy(strategy);

    let mut results = Vec::with_capacity(cases.len());
    for case in cases {
        results.push(eval::score(
            &recaller,
            memories.min_relevance,
            context,
            case,
        )?);
    }
    let report = eval::summarise(&results);

    let timings: Vec<Call> = reranker
        .map(|metered| metered.timings())
        .unwrap_or_default();
    let millis: Vec<f64> = timings.iter().map(|call| call.millis).collect();
    let judged: Vec<f64> = timings.iter().map(|call| call.candidates as f64).collect();

    let (strategy_name, weight) = match strategy {
        RerankStrategy::Rrf { weight } if has_stage => ("rrf", Some(weight)),
        RerankStrategy::Replace if has_stage => ("replace", None),
        // The reference row ran no stage: no strategy, no weight.
        _ => ("off", None),
    };

    Ok(GridCellReport {
        model: model.to_string(),
        strategy: strategy_name.to_string(),
        weight,
        depth,
        recall_at_k: report.recall_at_k,
        precision_at_1: report.precision_at_1,
        mrr: report.mrr,
        abstention_rate: report.abstention_rate,
        latency_ms_p50: report.latency_ms_p50,
        latency_ms_p95: report.latency_ms_p95,
        rerank_ms_p50: percentile(&millis, 50.0),
        rerank_ms_p95: percentile(&millis, 95.0),
        rerank_candidates_high: percentile(&judged, 100.0).map(|value| value as usize),
        precision_at_1_by_kind: report.precision_at_1_by_kind,
    })
}

/// Runs the grid over a freshly seeded eval corpus and returns every row.
///
/// Streams the table to stdout as cells finish (a full grid is tens of
/// minutes; a silent terminal is indistinguishable from a hung one) unless
/// `json` asks for the artifact-only form, where stdout belongs to the
/// report alone.
pub fn run(
    cases_path: &Path,
    config: &AppConfig,
    options: &GridOptions,
    json: bool,
) -> Result<GridReport> {
    let set = eval::load_set(cases_path)?;
    eval::validate(&set, cases_path)?;
    let (memories, context, _scratch) = eval::seed(&set, config)?;
    let embedder = memories.embedder.model_id().to_string();

    // One untimed recall before timing anything, for the eval's own
    // reason: index opens and cold caches must not land in a percentile.
    memories
        .recaller
        .execute_untracked(&context, &RecallQuery::new("warm up", 1)?)?;

    let scored_cases = set.cases.iter().filter(|case| !case.expect_empty).count();
    let negative_cases = set.cases.len() - scored_cases;

    if !json {
        println!(
            "grid: embedder {embedder} · corpus {} memories · {scored_cases} scored + {negative_cases} negative cases",
            set.memories.len()
        );
        println!(
            "no rerank deadline in force (a live request would enforce {} ms — read the rerank p95 against it)",
            config.rerank.timeout_ms
        );
        println!();
    }

    let mut cells = Vec::new();

    // The reference row first: every Δ below reads against it.
    let reference = measure_cell(
        &set.cases,
        &context,
        &memories,
        "off",
        RerankStrategy::default(),
        0,
        None,
    )?;
    cells.push(reference.clone());
    if !json {
        print_cell(cells.last().expect("just pushed"), None);
    }

    for requested in &options.models {
        if !json {
            println!("loading {requested} …");
        }
        let shared = Arc::new(FastembedReranker::load(
            requested,
            config.model_cache_dir(),
            1,
            config.rerank.batch_size,
        )?);
        let model = shared.model_id().to_string();
        wake(shared.as_ref())?;

        for (strategy, depth) in options.cells() {
            let metered = Arc::new(MeteredReranker::new(
                Arc::clone(&shared) as Arc<dyn Reranker>
            ));
            let cell = measure_cell(
                &set.cases,
                &context,
                &memories,
                &model,
                strategy,
                depth,
                Some(metered),
            )?;
            cells.push(cell);
            if !json {
                print_cell(cells.last().expect("just pushed"), Some(&reference));
            }
        }
    }

    if !json {
        println!();
        println!("{}", "─".repeat(110));
        println!("per-kind p@1 (Δp@1 above is the overall figure; the gate reads per kind too)");
        for cell in &cells {
            let id = cell_id(cell);
            let kinds = cell
                .precision_at_1_by_kind
                .iter()
                .map(|(kind, value)| format!("{kind} {value:.1}"))
                .collect::<Vec<_>>()
                .join(" · ");
            println!("{id:>52}  {kinds}");
        }
    }

    Ok(GridReport {
        embedder,
        corpus: set.memories.len(),
        scored_cases,
        negative_cases,
        live_timeout_ms: config.rerank.timeout_ms,
        cells,
    })
}

/// One untimed scoring call, so the first cell of a model does not pay
/// tokenizer setup and cold inference in its latency percentiles — the
/// warm-up recall covers the indexes, nothing covers the freshly loaded
/// rerank session.
fn wake(reranker: &dyn Reranker) -> Result<()> {
    let candidates = [
        RerankCandidate {
            id: MemoryId::new(),
            text: "warm up one",
        },
        RerankCandidate {
            id: MemoryId::new(),
            text: "warm up two",
        },
    ];
    reranker.score("warm up", &candidates).map(|_| ())
}

/// The row's short identity, as the per-kind block prints it.
fn cell_id(cell: &GridCellReport) -> String {
    match cell.weight {
        Some(weight) => format!(
            "{}/{} {}/{weight}",
            model_tail(&cell.model),
            cell.strategy,
            cell.depth
        ),
        None if cell.strategy == "off" => "off".to_string(),
        None => format!(
            "{}/{} {}",
            model_tail(&cell.model),
            cell.strategy,
            cell.depth
        ),
    }
}

/// `jina-reranker-v1-turbo-en` prints as `jina-v1-turbo` in the narrow
/// identity column — full names are in the table's own model column and
/// in `--json`.
fn model_tail(model: &str) -> String {
    model
        .replace("-reranker", "")
        .replace("base-multilingual", "ml")
}

/// One table row, with the Δ columns computed against the reference when
/// there is one. Latency Δ is signed the same direction as the quality
/// Δ's "higher is better" is *not*: added latency is a cost, so a positive
/// Δp95 is printed without a sign-flip and reads as milliseconds paid.
fn print_cell(cell: &GridCellReport, reference: Option<&GridCellReport>) {
    let (delta_p1, delta_mrr, delta_p95) = match reference {
        Some(off) => (
            cell.precision_at_1 - off.precision_at_1,
            cell.mrr - off.mrr,
            cell.latency_ms_p95 - off.latency_ms_p95,
        ),
        None => (0.0, 0.0, 0.0),
    };
    let weight = cell
        .weight
        .map(|weight| weight.to_string())
        .unwrap_or_else(|| "—".to_string());
    let rerank = match (
        cell.rerank_ms_p50,
        cell.rerank_ms_p95,
        cell.rerank_candidates_high,
    ) {
        (Some(p50), Some(p95), Some(high)) => format!("{p50:.0}/{p95:.0} ms ≤{high}c"),
        _ => "—".to_string(),
    };
    let deltas = if reference.is_none() {
        "  reference".to_string()
    } else {
        format!("{delta_p1:+6.1} {delta_mrr:+6.1} {delta_p95:+7.0}")
    };
    println!(
        "{:<36} {:>8} {:>4} {:>5} │ {:>5.1} {:>5.1} {:>5.1} {:>5.1} │ {:>6.1} {:>6.1} │ {:>16} │ {deltas}",
        model_tail(&cell.model),
        cell.strategy,
        weight,
        cell.depth,
        cell.precision_at_1,
        cell.mrr,
        cell.recall_at_k,
        cell.abstention_rate,
        cell.latency_ms_p50,
        cell.latency_ms_p95,
        rerank,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::eval::test_case;
    use crate::bootstrap::memories_wiring::Memories;
    use crate::bootstrap::wiring::{DATABASE_FILE, Identity};
    use crate::identity::domain::scope::Scope;
    use crate::memories::application::test_doubles::{FakeReranker, FallibleEmbedder, new_memory};

    fn defaults() -> GridOptions {
        GridOptions::parse(
            "jina-reranker-v1-turbo-en",
            "rrf,replace",
            "10,20,30,50",
            "0.5,1,2,4",
        )
        .expect("the plan's default grid parses")
    }

    #[test]
    fn the_plan_s_grid_is_twenty_rows_per_model_and_weight_varies_rrf_only() {
        let options = defaults();
        let cells = options.cells();
        // rrf: 4 depths × 4 weights = 16; replace: 4 depths, no weight.
        assert_eq!(cells.len(), 20);
        let rrf = cells
            .iter()
            .filter(|(strategy, _)| matches!(strategy, RerankStrategy::Rrf { .. }))
            .count();
        assert_eq!(rrf, 16);
        assert!(cells.iter().all(|(strategy, depth)| match strategy {
            RerankStrategy::Rrf { weight } => *weight >= 0.0 && *depth >= 10,
            RerankStrategy::Replace => true,
        }));
    }

    #[test]
    fn parsing_names_the_offending_token() {
        let error = GridOptions::parse("m", "rrf,squeeze", "10", "1").unwrap_err();
        assert!(error.to_string().contains("\"squeeze\""), "{error}");
        let error = GridOptions::parse("m", "rrf", "10,0", "1").unwrap_err();
        assert!(error.to_string().contains("below 2"), "{error}");
        let error = GridOptions::parse("m", "rrf", "10", "-1").unwrap_err();
        assert!(error.to_string().contains("negative"), "{error}");
        let error = GridOptions::parse("", "rrf", "10", "1").unwrap_err();
        assert!(error.to_string().contains("grid-models"), "{error}");
    }

    /// Three memories that all answer one query, over a fake embedder and
    /// the reversing fake reranker: `replace` must serve the exact reversed
    /// head, which makes p@1 flip 100→0 for the case expecting the
    /// reference head and 0→100 for the case expecting its tail.
    ///
    /// The `TempDir` comes back (unused by the test body) for the eval's
    /// own reason: dropping it would delete the indexes under the run.
    fn seeded() -> (Memories, UserContext, Vec<String>, tempfile::TempDir) {
        let scratch = tempfile::tempdir().expect("scratch");
        let database = Arc::new(
            crate::shared::sqlite::SqliteDatabase::open(&scratch.path().join(DATABASE_FILE))
                .unwrap(),
        );
        let identity = Identity::from_database(Arc::clone(&database)).unwrap();
        identity.user_creator.execute("grid", None).unwrap();
        let issued = identity
            .api_key_issuer
            .execute("grid", vec![Scope::Read, Scope::Write], "grid")
            .unwrap();
        let context = identity
            .key_authenticator
            .execute(&issued.token.render())
            .unwrap();

        let embedder: Arc<dyn crate::memories::domain::embedder::Embedder> =
            Arc::new(FallibleEmbedder::default());
        let memories = Memories::for_test(
            Arc::clone(&database),
            embedder,
            scratch.path().to_path_buf(),
        )
        .unwrap();
        let contents: Vec<String> = ["first", "second", "third"]
            .iter()
            .map(|which| format!("the user prefers pnpm {which}"))
            .collect();
        for content in &contents {
            memories
                .saver
                .execute(&context, new_memory(content), "grid")
                .unwrap();
        }
        (memories, context, contents, scratch)
    }

    #[test]
    fn the_reference_row_is_the_plain_pipeline_and_the_reversed_cell_flips_p_at_1() {
        let (memories, context, contents, _scratch) = seeded();
        let base = (*memories.recaller).clone();
        let query = RecallQuery::new("which package manager", 5).unwrap();
        let fused = base.execute_untracked(&context, &query).unwrap();
        assert!(fused.len() >= 3, "fixture must rank three results");
        let (head, tail) = (
            fused[0].memory.content().to_string(),
            fused[2].memory.content().to_string(),
        );
        assert_ne!(head, tail);
        assert!(contents.contains(&head) && contents.contains(&tail));

        let cases_head = vec![test_case(
            "head case",
            "paraphrase",
            "which package manager",
            &[head.as_str()],
            false,
        )];
        let cases_tail = vec![test_case(
            "tail case",
            "paraphrase",
            "which package manager",
            &[tail.as_str()],
            false,
        )];

        let reference = measure_cell(
            &cases_head,
            &context,
            &memories,
            "off",
            RerankStrategy::default(),
            0,
            None,
        )
        .unwrap();
        assert_eq!(reference.precision_at_1, 100.0);
        assert_eq!(reference.strategy, "off");
        assert_eq!(reference.rerank_ms_p50, None, "the reference ran no stage");

        let reversed = measure_cell(
            &cases_head,
            &context,
            &memories,
            "fake",
            RerankStrategy::Replace,
            30,
            Some(Arc::new(MeteredReranker::new(Arc::new(
                FakeReranker::reversing(),
            )))),
        )
        .unwrap();
        assert_eq!(
            reversed.precision_at_1, 0.0,
            "the reversed head cannot be the reference head"
        );
        assert_eq!(reversed.strategy, "replace");
        assert_eq!(reversed.weight, None, "replace has no weight to report");
        assert_eq!(reversed.rerank_candidates_high, Some(3));
        assert!(reversed.rerank_ms_p95.is_some());

        let flipped = measure_cell(
            &cases_tail,
            &context,
            &memories,
            "fake",
            RerankStrategy::Replace,
            30,
            Some(Arc::new(MeteredReranker::new(Arc::new(
                FakeReranker::reversing(),
            )))),
        )
        .unwrap();
        assert_eq!(
            flipped.precision_at_1, 100.0,
            "the reversed head is the reference tail"
        );
    }

    #[test]
    fn rows_and_ids_render_the_shape_the_results_log_pastes() {
        let cell = GridCellReport {
            model: "jina-reranker-v1-turbo-en".to_string(),
            strategy: "rrf".to_string(),
            weight: Some(2.0),
            depth: 30,
            recall_at_k: 88.5,
            precision_at_1: 67.5,
            mrr: 76.0,
            abstention_rate: 100.0,
            latency_ms_p50: 60.0,
            latency_ms_p95: 90.0,
            rerank_ms_p50: Some(40.0),
            rerank_ms_p95: Some(55.0),
            rerank_candidates_high: Some(30),
            precision_at_1_by_kind: BTreeMap::from([("needle".to_string(), 66.7)]),
        };
        // `print_cell` prints to stdout (this harness does not capture it);
        // what tests can pin is the identity string the per-kind block and
        // `--json` consumers key rows by.
        assert_eq!(cell_id(&cell), "jina-v1-turbo-en/rrf 30/2");
        let encoded = serde_json::to_string(&cell).unwrap();
        assert!(encoded.contains("\"weight\":2.0"));
        assert!(!encoded.contains("content"));
    }
}
