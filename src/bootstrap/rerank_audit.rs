//! `recuerdos-ai rerank audit` — the reranker's opinion about memories that
//! already exist (reranking-plan.md Phase 6.2).
//!
//! # Why a command, and why over a real store
//!
//! Phases 0–5 measured the second stage on the committed eval corpus —
//! thirty-five memories and forty-one cases, written by whoever wrote the
//! plan. That is enough to catch a regression and nowhere near enough to
//! answer the question an operator actually has: *"my store has eight hundred
//! memories and my own phrasing — would enabling this change what my agent
//! sees, and for better?"* The answer has to come from their data and their
//! queries, so the tool is a replay: run each query through the pipeline as it
//! serves today, run it again with the stage switched on, and print the two
//! orderings side by side.
//!
//! This is also the *only* supported way to run the reranker over an existing
//! corpus. There is deliberately no `rerank all` (plan §4): the reranker is
//! a read-time ranker, its scores mean something only beside the candidate
//! list that produced them, and a command that stamped them onto stored
//! memories would be a cache of stale opinions with no way to invalidate it.
//!
//! # What it promises
//!
//! * **It changes nothing.** The keyword index is opened read-only, recall
//!   runs through [`execute_untracked`], and nothing else on the path writes,
//!   so access counters, `last_accessed_at` and importance are exactly what
//!   they were. A replay that bumped the counters would feed importance
//!   decay and change the ranking it came to measure — and change it again
//!   on the second run.
//! * **It works with the daemon running.** No writer, so no fight for
//!   tantivy's per-index-directory writer lock. (SQLite is opened normally,
//!   which means the usual WAL reader alongside the daemon's writer.)
//! * **Both sides are the real pipeline.** The two recallers are one `Clone`
//!   over one set of indexes, so the only difference between the columns is
//!   the second stage — not a hand-duplicated wiring that drifted.
//!
//! # What it changes about the live pipeline, and why that is disclosed
//!
//! Two deliberate differences, both printed in the report header:
//!
//! * **No relevance floor.** The floor is a decision about what to *show*;
//!   an audit that applied it would hide the near-misses the operator is
//!   trying to judge, and dropping results before the rerank leg would
//!   change which candidates the reranker ever sees.
//! * **No rerank deadline.** The command exists partly to measure what the
//!   model costs; a tight timeout would report "deadline and a fallback"
//!   instead. The configured `timeout_ms` is what a live request would
//!   enforce, so it is reported beside the percentiles it has to contain.
//!
//! [`execute_untracked`]: crate::memories::application::memory_recaller::MemoryRecaller::execute_untracked

use crate::bootstrap::config::AppConfig;
use crate::bootstrap::eval::{percentile, truncate};
use crate::bootstrap::memories_wiring::{
    self, AuditRecall, assemble_audit_recall, load_reranker_for_audit, rerank_strategy,
};
use crate::bootstrap::wiring;
use crate::identity::application::background_user_resolver::BackgroundUserResolver;
use crate::identity::domain::user_context::UserContext;
use crate::identity::domain::user_repository::UserRepository;
use crate::memories::application::memory_recaller::{RerankSkip, RerankTrace};
use crate::memories::domain::embedder::Embedder;
use crate::memories::domain::recall_query::RecallQuery;
use crate::memories::domain::recall_ranker::{RerankStrategy, ScoredMemory};
use crate::memories::domain::reranker::{RerankCandidate, Reranker};
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use crate::shared::sqlite::SqliteDatabase;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

/// Content shown per result in the terminal. Wide enough to identify a
/// memory at a glance, short enough that a line does not wrap and turn a
/// table into a paragraph.
const SNIPPET_CHARS: usize = 64;

/// One line of a query file, plus the optional expectation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditQuery {
    pub text: String,
    /// A substring of the memory the operator believes should win.
    ///
    /// A substring rather than the exact content (which is what `eval`
    /// needs) because the operator is typing this about memories they wrote
    /// months ago and will not paste them verbatim. It is matched
    /// case-insensitively, and the first memory containing it is taken as
    /// the expected one.
    pub expect: Option<String>,
}

/// What the operator asked for, already parsed from the CLI.
#[derive(Debug, Clone)]
pub struct AuditOptions<'a> {
    pub queries: &'a Path,
    /// `None` means "the only user in this store" — see
    /// [`resolve_context`], which refuses to guess between several.
    pub user: Option<&'a str>,
    pub limit: usize,
    /// `Some` overrides `[rerank].depth`; `None` audits the configured depth.
    pub depth: Option<usize>,
}

/// The whole run: what was measured, per query, and the summary.
///
/// `Serialize` is the `--json` shape the Phase 7 grid scripts read. It
/// carries ids, ranks and scores but no memory content
/// ([`RankingLine::content`] is `#[serde(skip)]`), so a report can be
/// attached to an issue or committed without carrying anyone's memories
/// along with it — the same line the shadow log draws (Phase 4), drawn here
/// because this output *is* meant to be shared.
#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    pub user: String,
    pub embedder: String,
    pub reranker: String,
    /// How the rerank leg is fused, weight included when it has one.
    pub strategy: String,
    /// The head the reranker judged per query (`--depth` or `[rerank].depth`).
    pub depth: usize,
    /// Results per query printed and scored (`--limit`).
    pub limit: usize,
    /// `[rerank].mode` in the config this ran with — what clients are served
    /// *today*, which is not necessarily either column.
    pub configured_mode: String,
    /// `[rerank].timeout_ms`: what a live request would enforce. The audit
    /// runs with no deadline (module docs).
    pub live_timeout_ms: u64,
    /// `[retrieval].min_relevance`: what a live request drops below. The
    /// audit runs floorless (module docs).
    pub live_min_relevance: f32,
    pub queries: Vec<QueryAudit>,
    pub summary: AuditSummary,
}

/// One query, both orderings, and what the difference was.
#[derive(Debug, Clone, Serialize)]
pub struct QueryAudit {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
    /// The pipeline without the second stage.
    pub before: Vec<RankingLine>,
    /// The same pipeline with it switched on.
    pub after: Vec<RankingLine>,
    /// The one bit the ship gate is inspected against: the head of the list
    /// is a different memory. Everything else moving is noise by comparison;
    /// this is what a client's answer is built from.
    pub top1_changed: bool,
    /// Whether the second stage actually contributed to `after`.
    ///
    /// The agreement figures are hard to read without it: `before` and `after`
    /// are identical both when reranking agrees and when the stage produced
    /// nothing at all — a model that failed every call, or a `--depth 1` run
    /// where no head was ever two wide. Metered calls cannot tell those apart,
    /// because a call that failed really did happen.
    ///
    /// (The audit runs with no deadline, on purpose — measuring what the model
    /// costs is half the point — so `deadline` cannot appear here today. It is
    /// reported rather than assumed away, because a future audit that replays
    /// with the deployment's own deadline would otherwise inherit this blind
    /// spot.)
    pub contributed: bool,
    /// Why it did not contribute, when it did not: `deadline` (past
    /// `[rerank].timeout_ms`), `model`, or `not-needed` (fewer than two
    /// candidates to reorder).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// 1-based rank of the first memory containing `expect` before / after,
    /// `None` when there was no expectation or it was not in the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_before: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_after: Option<usize>,
}

impl QueryAudit {
    /// Whether this query's second stage gave up for exactly this reason.
    fn died_at(&self, reason: &str) -> bool {
        self.skip_reason.as_deref() == Some(reason)
    }
}

/// One result within an ordering.
#[derive(Debug, Clone, Serialize)]
pub struct RankingLine {
    pub rank: usize,
    /// The full id. The terminal prints its tail (UUIDv7 heads are
    /// timestamps, so they repeat); the JSON keeps the whole thing because
    /// it is the only handle to go and look at the memory itself.
    pub id: String,
    pub category: String,
    /// The absolute relevance the client-facing surface would show, measured
    /// from the index legs — untouched by the rerank stage (invariant I-4).
    pub relevance: f32,
    /// The model's own score, present only on the side that ran the stage.
    /// A ranker's input and an operator's diagnostic, never a threshold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_score: Option<f32>,
    /// The rank this memory held in the *other* ordering, when it appeared
    /// there: what `moved` means, spelled as two numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<usize>,
    /// Truncated content, terminal only — see [`AuditReport`].
    #[serde(skip)]
    pub content: String,
}

/// The numbers the plan asks for, and the denominators beside them.
#[derive(Debug, Clone, Serialize)]
pub struct AuditSummary {
    pub queries: usize,
    pub top1_changed: usize,
    /// 0–100, of `queries`.
    pub top1_changed_percent: f64,
    /// Mean |rank before − rank after| over every memory that appeared in
    /// either ordering. A memory that left the window counts as one past its
    /// end (see [`mean_rank_displacement`]), so a result pushed out of sight
    /// is charged for rather than ignored.
    pub mean_rank_displacement: f64,
    /// Reranker calls that ran — one per query whose fused head had two or
    /// more candidates to reorder. Zero means the stage never fired, and the
    /// latency lines below have nothing to say.
    ///
    /// Read it beside `contributed`: a call that missed its deadline is metered
    /// here because it really did run, and still changed nothing.
    pub rerank_calls: usize,
    /// Queries where the stage's order is actually in the "after" column. Any
    /// figure below `queries` means part of this report compares a reranked
    /// list with itself, which is why [`print_human`] says so out loud.
    pub contributed: usize,
    /// Of the queries that did not contribute: how many ran out of time. The
    /// audit runs deadline-free today (see [`QueryAudit::contributed`]), so
    /// this is zero unless that policy changes — it is counted rather than
    /// assumed away, because the blind spot is the interesting part.
    pub skipped_deadline: usize,
    /// …how many failed inside the model (including scores that did not line
    /// up with the candidates given). The one a real audit can hit today.
    pub skipped_model: usize,
    /// …and how many had fewer than two candidates to reorder: `--depth 1`, or
    /// a store where each query matches one memory. A fact about the run and
    /// the corpus, not about the stage's quality.
    pub not_needed: usize,
    /// The range of candidates the reranker actually judged per call.
    /// `[rerank].depth` is the ceiling; on a small store the corpus is, and
    /// latency scales with this number rather than with the ceiling — which
    /// is why reporting p95 without it would be reporting a number that
    /// cannot be compared to another store's.
    pub rerank_candidates_low: Option<usize>,
    pub rerank_candidates_high: Option<usize>,
    pub rerank_ms_p50: Option<f64>,
    pub rerank_ms_p95: Option<f64>,
    /// Queries carrying `expect` — the denominator of the four figures below.
    pub expectations: usize,
    /// 0–100 over `expectations`. `None` when no query had one, because an
    /// invented `0.0%` would read as a catastrophic score rather than as
    /// "not measured" — the lesson `eval::compare` already encodes.
    pub precision_at_1_before: Option<f64>,
    pub precision_at_1_after: Option<f64>,
    pub mrr_before: Option<f64>,
    pub mrr_after: Option<f64>,
}

/// Runs the audit against the store in `config` and returns the report.
///
/// Rendering is the caller's job ([`print_human`], or serde for `--json`) so
/// nothing here depends on stdout.
pub fn run(config: &AppConfig, options: &AuditOptions) -> Result<AuditReport> {
    let queries = parse_queries(options.queries)?;
    if queries.is_empty() {
        return Err(RaError::Validation(format!(
            "{} contains no queries: one per line, or `[[query]]` blocks with `text` \
             (and optionally `expect`)",
            options.queries.display()
        )));
    }

    let database = wiring::open_database(config)?;
    let identity = wiring::Identity::from_database(Arc::clone(&database))?;
    let context = resolve_context(&identity.users, options.user)?;

    // The deployment's own embedder: an audit measured on a different embedder
    // from the one that built the vector index would not describe this store
    // at all — and `verify_pin` inside the assembly refuses that mismatch
    // rather than reporting nonsense built on a dimension error.
    let embedder = memories_wiring::build_embedder(config)?;
    let model = load_reranker_for_audit(config)?;
    let reranker_name = model.model_id().to_string();
    let depth = options.depth.unwrap_or(config.rerank.depth);

    // stderr, deliberately: the report is stdout, and with `--json` stdout is
    // a file someone parses.
    eprintln!(
        "auditing {} queries for user {} ({} results each, rerank depth {depth})",
        queries.len(),
        context.handle(),
        options.limit
    );

    let replay = Replay {
        config,
        database: &database,
        context: &context,
        embedder,
        reranker: Arc::new(model),
        reranker_name,
        queries: &queries,
        limit: options.limit,
        depth,
    };
    replay_orderings(replay)
}

/// Everything a replay needs, as one value: the store, the models, and the
/// queries — nine separate arguments would have been a call site nobody could
/// read.
struct Replay<'a> {
    config: &'a AppConfig,
    database: &'a Arc<SqliteDatabase>,
    context: &'a UserContext,
    embedder: Arc<dyn Embedder>,
    reranker: Arc<dyn Reranker>,
    reranker_name: String,
    queries: &'a [AuditQuery],
    limit: usize,
    depth: usize,
}

/// The measurement, over a store and models handed in.
///
/// Split from [`run`] so a test can replay over a seeded store with the fake
/// embedder and a scripted reranker — the same reason `eval::seed` and
/// `Memories::for_test` take them — and still exercise the real assembly, the
/// real read-only indexes and the real arithmetic.
fn replay_orderings(input: Replay<'_>) -> Result<AuditReport> {
    let Replay {
        config,
        database,
        context,
        embedder,
        reranker,
        reranker_name,
        queries,
        limit,
        depth,
    } = input;

    let metered = Arc::new(MeteredReranker::new(reranker));
    let recall = assemble_audit_recall(
        config,
        database,
        Arc::clone(&embedder),
        Arc::clone(&metered) as Arc<dyn Reranker>,
        context,
        depth,
    )?;

    let rows = queries
        .iter()
        .map(|query| audit_one(&recall, context, query, limit))
        .collect::<Result<Vec<_>>>()?;

    let strategy = match rerank_strategy(&config.rerank) {
        RerankStrategy::Rrf { weight } => format!("rrf(weight {weight})"),
        RerankStrategy::Replace => "replace".to_string(),
    };
    let summary = summarise(&rows, &metered.timings());

    Ok(AuditReport {
        user: context.handle().to_string(),
        embedder: embedder.model_id().to_string(),
        reranker: reranker_name,
        strategy,
        depth,
        limit,
        configured_mode: config.rerank.mode.clone(),
        live_timeout_ms: config.rerank.timeout_ms,
        live_min_relevance: config.retrieval.min_relevance,
        queries: rows,
        summary,
    })
}

/// Both orderings for one query.
fn audit_one(
    recall: &AuditRecall,
    context: &UserContext,
    query: &AuditQuery,
    limit: usize,
) -> Result<QueryAudit> {
    let request = RecallQuery::new(&query.text, limit as i64)?;
    // Untracked, both sides: this is the whole reason 6.1 exists.
    let before = recall.plain.execute_untracked(context, &request)?;
    let report = recall
        .reranked
        .execute_untracked_traced(context, &request)?;
    let after = report.results;

    // Whether the "after" column is actually reranked. `None` means no stage
    // ran at all, which for the audit can only be a wiring bug — treated as
    // "did not contribute", because that is what the reader needs to know.
    let (contributed, skip_reason) = match &report.rerank {
        Some(RerankTrace::Applied { .. }) => (true, None),
        Some(RerankTrace::Skipped { reason, .. }) => (
            false,
            Some(match reason {
                RerankSkip::Deadline => "deadline",
                RerankSkip::Model => "model",
            }),
        ),
        Some(RerankTrace::NotNeeded { .. }) => (false, Some("not-needed")),
        Some(RerankTrace::Shadow { .. }) | None => (false, Some("not-applied")),
    };

    let (before_positions, after_positions) = (ranks(&before), ranks(&after));
    Ok(QueryAudit {
        text: query.text.clone(),
        expect: query.expect.clone(),
        before: lines(&before, &after_positions),
        after: lines(&after, &before_positions),
        top1_changed: changed_top1(&before, &after),
        contributed,
        skip_reason: skip_reason.map(str::to_string),
        expected_before: expected_rank(&before, query.expect.as_deref()),
        expected_after: expected_rank(&after, query.expect.as_deref()),
    })
}

/// 1-based position of every returned memory, keyed by id.
fn ranks(scored: &[ScoredMemory]) -> HashMap<MemoryId, usize> {
    scored
        .iter()
        .enumerate()
        .map(|(index, hit)| (hit.memory.id(), index + 1))
        .collect()
}

fn lines(scored: &[ScoredMemory], other: &HashMap<MemoryId, usize>) -> Vec<RankingLine> {
    scored
        .iter()
        .enumerate()
        .map(|(index, hit)| RankingLine {
            rank: index + 1,
            id: hit.memory.id().to_string(),
            category: hit.memory.category().as_str().to_string(),
            relevance: hit.relevance,
            rerank_score: hit.rerank_score,
            moved_from: other.get(&hit.memory.id()).copied(),
            content: truncate(hit.memory.content(), SNIPPET_CHARS),
        })
        .collect()
}

/// Whether the head of the list is a different memory.
///
/// Two empty results did not "change" — nothing was served either way, which
/// is a fact an operator reads differently from a swapped answer.
fn changed_top1(before: &[ScoredMemory], after: &[ScoredMemory]) -> bool {
    let head = |hits: &[ScoredMemory]| hits.first().map(|hit| hit.memory.id());
    head(before) != head(after)
}

/// The first memory whose content contains `expect`, case-insensitively.
fn expected_rank(scored: &[ScoredMemory], expect: Option<&str>) -> Option<usize> {
    let needle = expect?.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    scored
        .iter()
        .position(|hit| hit.memory.content().to_lowercase().contains(&needle))
        .map(|index| index + 1)
}

/// Mean |before − after| over the union of both orderings.
///
/// A memory that appears in only one of them still moved — out of, or into,
/// what a client sees — so it counts as ranked just past the end of the list
/// that omitted it. `max(len) + 1` for both sides keeps the charge symmetric
/// and reproducible by hand from the printed lists.
fn mean_rank_displacement(rows: &[QueryAudit]) -> f64 {
    let mut total = 0.0_f64;
    let mut counted = 0_usize;
    for row in rows {
        // A memory that appears in only one list still moved — out of what a
        // client sees, or into it — so it counts as ranked just past the end
        // of the list that omitted it. Using the same `outside` for both
        // sides keeps the charge symmetric and reproducible from the printed
        // lists, which is the whole point of printing them.
        let outside = row.before.len().max(row.after.len()) + 1;
        let position = |lines: &[RankingLine], id: &str| {
            lines
                .iter()
                .find(|line| line.id == id)
                .map_or(outside, |line| line.rank)
        };

        let mut ids: Vec<&str> = row
            .before
            .iter()
            .chain(row.after.iter())
            .map(|line| line.id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();

        for id in ids {
            let (before, after) = (position(&row.before, id), position(&row.after, id));
            total += (before as f64 - after as f64).abs();
            counted += 1;
        }
    }
    if counted == 0 {
        0.0
    } else {
        total / counted as f64
    }
}

/// The share-of-credit function both the p@1 and the MRR lines are built
/// from: `1 / rank` for the expectation's position, `0` when it is not in
/// the window at all. Passing p@1 as "rank one or nothing" is what makes it
/// the mean of `1` and `0` — the same two definitions `eval::summarise`
/// uses, applied to the two columns so the *delta* is the finding.
///
/// `None` rather than `0.0` when nothing was expected: an invented zero
/// reads as a terrible score, and "not measured" is a different fact.
fn mean_credential(rows: &[&QueryAudit], rank: fn(&QueryAudit) -> Option<usize>) -> Option<f64> {
    if rows.is_empty() {
        return None;
    }
    let mean = rows
        .iter()
        .map(|row| rank(row).map_or(0.0, |position| 1.0 / position as f64))
        .sum::<f64>()
        / rows.len() as f64;
    Some(mean * 100.0)
}

fn summarise(rows: &[QueryAudit], timings: &[Call]) -> AuditSummary {
    let queries = rows.len();
    let top1_changed = rows.iter().filter(|row| row.top1_changed).count();
    let expected: Vec<&QueryAudit> = rows.iter().filter(|row| row.expect.is_some()).collect();
    let judged: Vec<f64> = timings.iter().map(|call| call.candidates as f64).collect();

    AuditSummary {
        queries,
        top1_changed,
        top1_changed_percent: if queries == 0 {
            0.0
        } else {
            (top1_changed as f64 / queries as f64) * 100.0
        },
        mean_rank_displacement: mean_rank_displacement(rows),
        rerank_calls: timings.len(),
        contributed: rows.iter().filter(|row| row.contributed).count(),
        skipped_deadline: rows.iter().filter(|row| row.died_at("deadline")).count(),
        skipped_model: rows.iter().filter(|row| row.died_at("model")).count(),
        not_needed: rows.iter().filter(|row| row.died_at("not-needed")).count(),
        rerank_candidates_low: percentile(&judged, 0.0).map(|value| value as usize),
        rerank_candidates_high: percentile(&judged, 100.0).map(|value| value as usize),
        rerank_ms_p50: percentile(
            &timings.iter().map(|call| call.millis).collect::<Vec<_>>(),
            50.0,
        ),
        rerank_ms_p95: percentile(
            &timings.iter().map(|call| call.millis).collect::<Vec<_>>(),
            95.0,
        ),
        expectations: expected.len(),
        precision_at_1_before: mean_credential(&expected, |row| {
            (row.expected_before == Some(1)).then_some(1)
        }),
        precision_at_1_after: mean_credential(&expected, |row| {
            (row.expected_after == Some(1)).then_some(1)
        }),
        mrr_before: mean_credential(&expected, |row| row.expected_before),
        mrr_after: mean_credential(&expected, |row| row.expected_after),
    }
}

/// A reranker that records how long each call took.
///
/// Why the timing lives here rather than around the recall call: a recall
/// also embeds the query, and on the local model that is the larger half of
/// the wall clock. Reporting that as "rerank latency" would blame the second
/// stage for the first. On this boundary a sample is exactly the inference
/// over `min(depth, candidates)` pairs — which is the number `timeout_ms` has
/// to contain. Also used by `eval --grid` (reranking-plan.md Phase 7.2),
/// which needs the same stage-only percentiles per cell.
pub(crate) struct MeteredReranker {
    inner: Arc<dyn Reranker>,
    /// One entry per call: how long it took and how many pairs it judged.
    /// Latency without the second number is not comparable between stores —
    /// 30 candidates and 5 candidates are different jobs. A `Vec` rather than
    /// a histogram: an audit is tens of queries, and the raw samples make the
    /// printed percentiles checkable by hand.
    calls: Mutex<Vec<Call>>,
}

/// One timed reranker call.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Call {
    pub(crate) millis: f64,
    pub(crate) candidates: usize,
}

impl MeteredReranker {
    pub(crate) fn new(inner: Arc<dyn Reranker>) -> Self {
        Self {
            inner,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn timings(&self) -> Vec<Call> {
        // A panic in the model while scoring unwinds through this lock, and
        // the samples taken before it are still valid and still wanted: the
        // audit's own report matters more than the lock's bookkeeping.
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Reranker for MeteredReranker {
    fn score(&self, query: &str, candidates: &[RerankCandidate<'_>]) -> Result<Vec<f32>> {
        let started = Instant::now();
        let scores = self.inner.score(query, candidates)?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Call {
                millis: elapsed,
                candidates: candidates.len(),
            });
        Ok(scores)
    }
}

/// The user whose store is audited.
///
/// Same rule the background jobs follow (`graph backfill`, `consolidate`):
/// the store says who exists, and the command never invents or guesses one.
/// Here that is sharper than in a job — auditing the wrong user's memories
/// produces a confident report about data the operator did not mean to look
/// at, so several users means `--user`, full stop.
fn resolve_context(
    users: &Arc<dyn UserRepository>,
    requested: Option<&str>,
) -> Result<UserContext> {
    let handle = match requested {
        Some(handle) => handle.to_string(),
        None => {
            let mut all = users.list()?;
            all.sort_by_key(|user| user.handle().to_string());
            match all.as_slice() {
                [] => {
                    return Err(RaError::NotFound(
                        "this store has no users yet — create one with `recuerdos-ai user add <handle>`"
                            .to_string(),
                    ));
                }
                [only] => only.handle().to_string(),
                many => {
                    let names = many
                        .iter()
                        .map(|user| user.handle())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(RaError::Validation(format!(
                        "this store has {} users ({names}); pass --user <handle> to say whose \
                         memories to audit",
                        many.len()
                    )));
                }
            }
        }
    };

    let Some(found) = users.find_by_handle(&handle)? else {
        let listed = users.list()?;
        let mut known = listed.iter().map(|user| user.handle()).collect::<Vec<_>>();
        known.sort();
        return Err(RaError::NotFound(format!(
            "no user {handle:?}. This store has: {}",
            known.join(", ")
        )));
    };
    BackgroundUserResolver::new(Arc::clone(users)).execute(found.id())
}

/// Reads the query file: one query per line, or TOML `[[query]]` blocks.
///
/// Sniffed rather than keyed on the file extension, because the plain form
/// is what someone writes at a terminal and the TOML form is what they add
/// `expect` to later, in the same file — and being told to rename it to keep
/// the same content would be a pointless tax. `#` introduces a comment in
/// the plain form, which is also what TOML uses.
pub fn parse_queries(path: &Path) -> Result<Vec<AuditQuery>> {
    let raw = std::fs::read_to_string(path).map_err(|error| {
        RaError::Validation(format!(
            "could not read the queries file {}: {error}",
            path.display()
        ))
    })?;

    if raw
        .lines()
        .any(|line| line.trim_start().starts_with("[[query]]"))
    {
        #[derive(Deserialize)]
        struct QueriesFile {
            #[serde(default, rename = "query")]
            queries: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            text: String,
            #[serde(default)]
            expect: Option<String>,
        }

        let parsed: QueriesFile = toml::from_str(&raw).map_err(|error| {
            RaError::Validation(format!(
                "{} is not a valid queries file: {error}. Use `[[query]]` blocks with `text` \
                 (and optionally `expect`), or one plain query per line",
                path.display()
            ))
        })?;
        return Ok(parsed
            .queries
            .into_iter()
            .filter(|entry| !entry.text.trim().is_empty())
            .map(|entry| AuditQuery {
                text: entry.text.trim().to_string(),
                expect: entry.expect.map(|expect| expect.trim().to_string()),
            })
            .collect());
    }

    Ok(raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| AuditQuery {
            text: line.to_string(),
            expect: None,
        })
        .collect())
}

/// The operator's report. Content appears here and nowhere else — see
/// [`AuditReport`] for why `--json` leaves it out.
pub fn print_human(report: &AuditReport) {
    println!(
        "user {} · embedder {} · reranker {}",
        report.user, report.embedder, report.reranker
    );
    println!(
        "strategy {} · depth {} · limit {} · live rerank timeout {} ms (none here)",
        report.strategy, report.depth, report.limit, report.live_timeout_ms
    );
    println!(
        "serving mode {:?} — {}",
        report.configured_mode,
        what_clients_see(&report.configured_mode)
    );
    println!(
        "floor: clients drop below {:.2}, this audit shows everything",
        report.live_min_relevance
    );

    for (index, row) in report.queries.iter().enumerate() {
        println!(
            "\n{}. {}{}",
            index + 1,
            row.text,
            if row.top1_changed {
                "   ⚑ top-1 changed"
            } else {
                ""
            }
        );
        print_ordering("before", &row.before, false);
        print_ordering("after", &row.after, true);
        if let Some(expect) = &row.expect {
            println!(
                "   expect {expect:?}: rank {} → {}",
                rank_text(row.expected_before),
                rank_text(row.expected_after)
            );
        }
    }

    let summary = &report.summary;
    println!("\n{}", "─".repeat(78));
    if summary.contributed < summary.queries {
        // Before any number, because this is the case where the numbers
        // describe nothing: a stage that missed its deadline served the
        // `before` order, so the two columns agree for a reason that has
        // nothing to do with reranking quality — and a metered call cannot
        // tell that apart from a stage that agreed.
        let quiet = summary.queries - summary.contributed;
        println!(
            "⚠ the stage contributed to {}/{} queries: {} served the pre-rerank order, \
             so the agreement above is not evidence.",
            summary.contributed, summary.queries, quiet
        );
        let mut reasons: Vec<String> = Vec::new();
        if summary.skipped_deadline > 0 {
            reasons.push(format!(
                "{} ran out of time (raise [rerank].timeout_ms, or lower [rerank].depth)",
                summary.skipped_deadline
            ));
        }
        if summary.skipped_model > 0 {
            reasons.push(format!("{} failed inside the model", summary.skipped_model));
        }
        if summary.not_needed > 0 {
            reasons.push(format!(
                "{} had fewer than two candidates to reorder — the floor and the store, \
                 not the stage",
                summary.not_needed
            ));
        }
        if !reasons.is_empty() {
            println!("   {}", reasons.join("; "));
        }
    }
    println!(
        "top-1 changed: {}/{} ({:.1}%)   mean rank displacement: {:.2}",
        summary.top1_changed,
        summary.queries,
        summary.top1_changed_percent,
        summary.mean_rank_displacement
    );
    match (summary.rerank_ms_p50, summary.rerank_ms_p95) {
        (Some(p50), Some(p95)) => {
            let judged = match (
                summary.rerank_candidates_low,
                summary.rerank_candidates_high,
            ) {
                (Some(low), Some(high)) if low != high => format!("{low}–{high} candidates"),
                (Some(_), Some(high)) => format!("{high} candidates"),
                _ => "candidates".to_string(),
            };
            println!(
                "rerank latency: p50 {:.0} ms   p95 {:.0} ms   ({} calls, {judged} judged; depth ceiling {})",
                p50, p95, summary.rerank_calls, report.depth
            );
        }
        _ => println!(
            "rerank latency: not measured — the stage ran {} times (nothing to reorder below 2 candidates)",
            summary.rerank_calls
        ),
    }
    if summary.expectations == 0 {
        println!(
            "no `expect` in the query file: p@1 and MRR before/after need to be told \
             which memory answers each query"
        );
    } else {
        println!(
            "expectations ({} of {} queries): p@1 {} → {}   MRR {} → {}",
            summary.expectations,
            summary.queries,
            percent(summary.precision_at_1_before),
            percent(summary.precision_at_1_after),
            percent(summary.mrr_before),
            percent(summary.mrr_after)
        );
    }
    if summary.top1_changed > 0 {
        // The ship gate is a human reading every one of these, so point at
        // them rather than making someone scroll for the ⚑ marks.
        let flagged = report
            .queries
            .iter()
            .enumerate()
            .filter(|(_, row)| row.top1_changed)
            .map(|(index, _)| (index + 1).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("read these before trusting the number: queries {flagged}");
    }
}

/// One sentence that keeps a report from being misread: in `off` the operator
/// is looking at a future, in `on` at their present, and in `shadow` at a
/// future nobody is served yet.
fn what_clients_see(mode: &str) -> &'static str {
    match mode.trim().to_ascii_lowercase().as_str() {
        "on" => "clients see the `after` column today",
        "shadow" => "clients see the `before` column; the stage scores and logs only",
        _ => "clients see the `before` column today",
    }
}

/// One ordering. `reranked` decides only what is *annotated*: the rerank
/// score exists on that side alone, and so does "from N" — the plain column is
/// the baseline, and marking where its rows would end up makes the reader
/// compare a thing they are not supposed to be reading yet.
fn print_ordering(label: &str, lines: &[RankingLine], reranked: bool) {
    println!("   {label}");
    if lines.is_empty() {
        println!("     (nothing recalled)");
        return;
    }
    for line in lines {
        let moved = match line.moved_from {
            Some(from) if reranked && from != line.rank => format!(" (from {from})"),
            _ => String::new(),
        };
        let score = if reranked {
            line.rerank_score
                .map(|value| format!(" rerank {value:.3}"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        println!(
            "   {:>2}. {}  {:<22} rel {:.2}{score}{moved}  {}",
            line.rank,
            short_id(&line.id),
            truncate(&line.category, 22),
            line.relevance,
            line.content
        );
    }
}

/// The tail of a UUIDv7, as the shadow log prints it: the head is the
/// creation timestamp and repeats across memories saved in the same second.
fn short_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 8 {
        return id.to_string();
    }
    chars[chars.len() - 8..].iter().collect()
}

fn rank_text(rank: Option<usize>) -> String {
    rank.map_or_else(|| "absent".to_string(), |rank| rank.to_string())
}

fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_string(), |value| format!("{value:.1}%"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::memories_wiring::Memories;
    use crate::bootstrap::wiring::{DATABASE_FILE, Identity};
    use crate::identity::domain::scope::Scope;
    use crate::memories::application::test_doubles::{FakeReranker, FallibleEmbedder, new_memory};
    use crate::shared::sqlite::map_sqlite_error;

    /// A store on disk with a live read-write instance still attached to it —
    /// the daemon stand-in. It holds the keyword index's writer lock for the
    /// whole test, which is the condition the audit promises to work under.
    struct Store {
        config: AppConfig,
        database: Arc<SqliteDatabase>,
        users: Arc<dyn UserRepository>,
        embedder: Arc<dyn Embedder>,
        alex: UserContext,
        sam: UserContext,
        _writable: Memories,
        _scratch: tempfile::TempDir,
    }

    /// Builds the store and saves `(handle, content)` pairs into it.
    fn store(saved: &[(&str, &str)]) -> Store {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let mut config = AppConfig::default();
        config.storage.path = scratch.path().to_string_lossy().to_string();
        // `replace` plus the reversing fake makes "the reranked order was
        // served" an *exact* assertion. The damped rrf leg deliberately cannot
        // fully invert a head — its arithmetic is tested in `recall_ranker`,
        // and asserting an inversion through it would test the wrong thing.
        config.rerank.strategy = "replace".to_string();

        let database = Arc::new(SqliteDatabase::open(&scratch.path().join(DATABASE_FILE)).unwrap());
        let identity = Identity::from_database(Arc::clone(&database)).unwrap();
        let authenticate = |handle: &str| -> UserContext {
            if identity.users.find_by_handle(handle).unwrap().is_none() {
                identity.user_creator.execute(handle, None).unwrap();
            }
            let issued = identity
                .api_key_issuer
                .execute(handle, vec![Scope::Admin], "test")
                .unwrap();
            identity
                .key_authenticator
                .execute(&issued.token.render())
                .unwrap()
        };
        let alex = authenticate("alex");
        let sam = authenticate("sam");

        let embedder: Arc<dyn Embedder> = Arc::new(FallibleEmbedder::default());
        let writable = Memories::for_test(
            Arc::clone(&database),
            Arc::clone(&embedder),
            scratch.path().to_path_buf(),
        )
        .unwrap();
        for (handle, content) in saved {
            let context = if *handle == "sam" { &sam } else { &alex };
            writable
                .saver
                .execute(context, new_memory(content), "test")
                .unwrap();
        }

        Store {
            config,
            database,
            users: Arc::clone(&identity.users),
            embedder,
            alex,
            sam,
            _writable: writable,
            _scratch: scratch,
        }
    }

    fn replay_in(store: &Store, who: &UserContext, queries: &[AuditQuery]) -> AuditReport {
        replay_orderings(Replay {
            config: &store.config,
            database: &store.database,
            context: who,
            embedder: Arc::clone(&store.embedder),
            reranker: Arc::new(FakeReranker::reversing()),
            reranker_name: "fake-reranker".to_string(),
            queries,
            limit: 8,
            depth: 30,
        })
        .expect("the audit should run over a seeded store")
    }

    /// Three memories that all answer the same query, so the fused head is
    /// three deep and the fake has something to reorder.
    fn three_memories() -> Store {
        let contents: Vec<String> = ["first", "second", "third"]
            .iter()
            .map(|which| format!("the user prefers pnpm {which}"))
            .collect();
        let pairs: Vec<(&str, &str)> = contents.iter().map(|c| ("alex", c.as_str())).collect();
        store(&pairs)
    }

    fn audit(text: &str, expect: Option<&str>) -> AuditQuery {
        AuditQuery {
            text: text.to_string(),
            expect: expect.map(str::to_string),
        }
    }

    fn ids(lines: &[RankingLine]) -> Vec<&str> {
        lines.iter().map(|line| line.id.as_str()).collect()
    }

    /// Every row of the system of record, as the audit must leave it.
    fn snapshot(database: &SqliteDatabase) -> Vec<(String, String, Option<String>, i64, f64)> {
        database
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT id, content, last_accessed_at, access_count, importance
                         FROM memories ORDER BY id",
                    )
                    .map_err(|e| map_sqlite_error(e, "snapshot"))?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    })
                    .map_err(|e| map_sqlite_error(e, "snapshot"))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| map_sqlite_error(e, "snapshot"))?;
                Ok(rows)
            })
            .unwrap()
    }

    #[test]
    fn the_reranked_column_is_the_reversed_head_and_the_plain_one_is_untouched() {
        let store = three_memories();
        let report = replay_in(&store, &store.alex, &[audit("which package manager", None)]);

        assert_eq!(report.queries.len(), 1);
        let row = &report.queries[0];
        assert!(
            row.before.len() >= 2,
            "the fixture should rank several results"
        );

        let mut reversed = ids(&row.before);
        reversed.reverse();
        assert_eq!(
            ids(&row.after),
            reversed,
            "`On` did not serve the rerank order"
        );
        assert!(row.top1_changed, "a reversal of three must flag the head");
        assert!(
            row.after.iter().all(|line| line.rerank_score.is_some()),
            "the reranked column carries the model's score"
        );
        assert!(
            row.before.iter().all(|line| line.rerank_score.is_none()),
            "the plain column must look exactly like what a client sees today"
        );
        assert_eq!(report.summary.rerank_calls, 1);
        assert_eq!(report.summary.queries, 1);
        assert_eq!(report.summary.top1_changed_percent, 100.0);
        assert_eq!(report.strategy, "replace");
    }

    #[test]
    fn the_summary_arithmetic_is_the_number_a_human_can_check_by_hand() {
        let store = three_memories();
        // Learn the fused order first, then ask for the memory that came in
        // third — so "it should have been first" is a fact about this run
        // rather than a guess about how BM25 and the fake embeddings tie-break.
        let plain = replay_in(&store, &store.alex, &[audit("which package manager", None)]);
        let buried = plain.queries[0].before[2].content.clone();

        let report = replay_in(
            &store,
            &store.alex,
            &[audit("which package manager", Some(&buried))],
        );
        let row = &report.queries[0];
        assert_eq!(row.expected_before, Some(3));
        assert_eq!(row.expected_after, Some(1));

        let summary = &report.summary;
        assert_eq!(summary.expectations, 1);
        assert_eq!(summary.precision_at_1_before, Some(0.0));
        assert_eq!(summary.precision_at_1_after, Some(100.0));
        let mrr_before = summary.mrr_before.expect("one expectation was given");
        let mrr_after = summary.mrr_after.expect("one expectation was given");
        assert!(
            (mrr_before - 100.0 / 3.0).abs() < 0.01,
            "MRR before: {mrr_before}"
        );
        assert!((mrr_after - 100.0).abs() < 0.01, "MRR after: {mrr_after}");
        // Ranks 1↔3 and 2 staying: (2 + 0 + 2) / 3.
        assert!(
            (summary.mean_rank_displacement - 4.0 / 3.0).abs() < 0.001,
            "displacement: {}",
            summary.mean_rank_displacement
        );
    }

    #[test]
    fn an_unexpected_query_reports_that_it_was_not_measured_rather_than_zero() {
        let store = three_memories();
        let report = replay_in(&store, &store.alex, &[audit("which package manager", None)]);
        let summary = &report.summary;
        assert_eq!(summary.expectations, 0);
        assert_eq!(summary.precision_at_1_before, None);
        assert_eq!(summary.mrr_after, None);
    }

    #[test]
    fn the_json_report_carries_ids_and_scores_and_no_memories() {
        let store = three_memories();
        let report = replay_in(&store, &store.alex, &[audit("which package manager", None)]);
        let encoded = serde_json::to_string(&report).expect("the report serialises");

        // The operator's own memories stay in the terminal: the report is the
        // artefact that gets pasted into an issue or read by a Phase 7 script.
        assert!(
            !encoded.contains("pnpm"),
            "memory content leaked into --json: {encoded}"
        );
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["user"], "alex");
        assert_eq!(value["reranker"], "fake-reranker");
        assert_eq!(value["queries"].as_array().unwrap().len(), 1);
        let first = &value["queries"][0];
        assert_eq!(first["top1_changed"], true);
        assert!(first["before"].as_array().unwrap().len() >= 2);
        assert!(first["after"][0]["id"].is_string());
        assert!(first["after"][0]["rerank_score"].is_number());
        assert!(first["before"][0].get("rerank_score").is_none());
        assert!(first["before"][0].get("content").is_none());
        assert!(value["summary"]["rerank_ms_p50"].is_number());
        // The head it actually judged: depth is the ceiling, this store is the
        // ceiling that binds — and latency is meaningless without it.
        assert_eq!(value["summary"]["rerank_candidates_low"], 3);
        assert_eq!(value["summary"]["rerank_candidates_high"], 3);
    }

    #[test]
    fn auditing_leaves_memories_access_counters_and_importance_byte_identical() {
        let store = three_memories();
        let before = snapshot(&store.database);
        assert_eq!(
            before.len(),
            3,
            "the fixture should have saved three memories"
        );
        // Saved, never read: the columns the audit must not touch start empty.
        assert!(
            before
                .iter()
                .all(|(_, _, last, count, _)| last.is_none() && *count == 0),
            "the fixture starts unread: {before:?}"
        );

        let queries = [audit("which package manager", None)];
        // Twice, because "it did not write" and "it is not idempotent" are
        // different failures and only the second run catches the drift.
        let first = replay_in(&store, &store.alex, &queries);
        let second = replay_in(&store, &store.alex, &queries);

        assert_eq!(
            snapshot(&store.database),
            before,
            "the audit wrote to the store"
        );
        assert_eq!(
            first.summary.top1_changed, second.summary.top1_changed,
            "two audits of an unchanged store disagreed"
        );
        for (earlier, later) in first.queries.iter().zip(&second.queries) {
            assert_eq!(ids(&earlier.before), ids(&later.before));
            assert_eq!(ids(&earlier.after), ids(&later.after));
        }
    }

    #[test]
    fn an_audit_sees_only_the_user_it_was_told_to_read() {
        let store = store(&[
            ("alex", "the user prefers pnpm in alex's projects"),
            ("sam", "the user prefers pnpm in sam's projects"),
        ]);
        let report = replay_in(&store, &store.sam, &[audit("which package manager", None)]);

        assert!(!report.queries[0].before.is_empty());
        assert!(
            report.queries[0]
                .before
                .iter()
                .all(|line| line.content.contains("sam")),
            "alex's memories appeared in sam's audit"
        );
        assert_eq!(report.user, "sam");
    }

    #[test]
    fn several_users_requires_the_flag_and_an_unknown_one_says_who_exists() {
        let store = store(&[("alex", "the user prefers pnpm")]);

        let ambiguous = resolve_context(&store.users, None)
            .expect_err("two users exist, so the command must not pick one");
        let message = ambiguous.to_string();
        assert!(message.contains("--user"), "{message}");
        assert!(
            message.contains("alex") && message.contains("sam"),
            "{message}"
        );

        let chosen = resolve_context(&store.users, Some("sam")).unwrap();
        assert_eq!(chosen.handle(), "sam");

        let unknown = resolve_context(&store.users, Some("nobody")).expect_err("no such user");
        let message = unknown.to_string();
        assert!(message.contains("nobody"), "{message}");
        assert!(
            message.contains("alex, sam"),
            "should list the real handles: {message}"
        );
    }

    #[test]
    fn a_store_with_no_users_says_so_instead_of_auditing_nothing() {
        let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
        let identity = Identity::from_database(database).unwrap();
        let error =
            resolve_context(&identity.users, None).expect_err("there is nobody to audit yet");
        assert!(error.to_string().contains("user add"), "{}", error);
    }

    #[test]
    fn a_plain_query_file_is_one_per_line_with_comments() {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("queries.txt");
        std::fs::write(
            &path,
            "# what my agent gets wrong\nwhich package manager\n\n  \n# expect-less too\nwho leads the team?\n",
        )
        .unwrap();

        let parsed = parse_queries(&path).unwrap();
        assert_eq!(
            parsed.iter().map(|q| q.text.as_str()).collect::<Vec<_>>(),
            ["which package manager", "who leads the team?"]
        );
        assert!(parsed.iter().all(|q| q.expect.is_none()));
    }

    #[test]
    fn a_toml_query_file_carries_the_expectations() {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("queries.toml");
        std::fs::write(
            &path,
            "[[query]]\ntext = \"which package manager\"\nexpect = \"pnpm\"\n\n[[query]]\ntext = \"unanswered\"\n",
        )
        .unwrap();

        let parsed = parse_queries(&path).unwrap();
        assert_eq!(parsed[0].text, "which package manager");
        assert_eq!(parsed[0].expect.as_deref(), Some("pnpm"));
        assert_eq!(parsed[1].expect, None);
    }

    #[test]
    fn a_broken_toml_file_explains_both_formats() {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("queries.toml");
        std::fs::write(&path, "[[query]]\ntext = \"unclosed\n").unwrap();

        let error = parse_queries(&path).expect_err("malformed TOML");
        let message = error.to_string();
        assert!(message.contains("[[query]]"), "{message}");
        assert!(message.contains("one plain query per line"), "{message}");
    }

    #[test]
    fn an_empty_query_file_is_refused_before_anything_is_opened() {
        let store = three_memories();
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("queries.txt");
        std::fs::write(&path, "# nothing here\n").unwrap();

        let error = run(
            &store.config,
            &AuditOptions {
                queries: &path,
                user: Some("alex"),
                limit: 8,
                depth: None,
            },
        )
        .expect_err("no queries to run");
        assert!(error.to_string().contains("no queries"), "{error}");
    }
}
