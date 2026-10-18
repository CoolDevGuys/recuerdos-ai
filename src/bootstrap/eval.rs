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
use crate::memories::application::memory_recaller::{MemoryRecaller, RerankSkip, RerankTrace};
use crate::memories::domain::category::Category;
use crate::memories::domain::entity_graph::Relation;
use crate::memories::domain::entity_key::EntityKey;
use crate::memories::domain::memory::{Entity, MemorySource, NewMemory};
use crate::memories::domain::recall_query::RecallQuery;
use crate::shared::clock::FixedClock;
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::SqliteDatabase;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Set the first time an eval run discovers its rerank stage was skipped, so
/// the operator hears about it once rather than once per case. Process-wide by
/// design: the eval is a CLI run, and "warn me the first time" is the ask.
static RERANK_SKIP_WARNED: AtomicBool = AtomicBool::new(false);

/// The instant the eval calls "now", fixed: `2026-06-01T00:00:00Z`.
///
/// Two problems die here (recall-quality-plan.md Task 0.1).
///
/// **Cross-process drift.** With the wall clock, two runs of the same corpus
/// on either side of a second boundary age every memory by a different
/// fraction of a day, and the reranking plan measured that as MRR 74.3 vs
/// 74.6 for identical code. Recency is a multiplier on the score, so a
/// moving `now` is a moving measurement.
///
/// **Ties that are really coin flips.** `created_at` ages in whole seconds,
/// so a corpus seeded in one pass is the same age for every memory: a
/// symmetric disagreement between the vector and keyword legs is an *exact*
/// score tie, and `sort_ranked` settles it by memory id. Ids are UUIDv7, so
/// the "winner" changes when the corpus is re-seeded, and the number a phase
/// is gated on was decided by a random value. Ages (Task 0.2) put real
/// distance between memories; fixing `now` makes the distance mean the same
/// thing in every run.
///
/// Latency is the one figure this cannot pin, which is why the gate reads it
/// paired on/off and never absolute.
pub(crate) fn eval_now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
        .expect("the eval's fixed instant is a valid RFC 3339 timestamp")
        .with_timezone(&Utc)
}

/// The cut-off the gate is defined at.
///
/// Five because that is roughly what fits in an agent's context budget
/// alongside an actual conversation — a memory ranked eighth is not
/// wrong, it is just never seen. Also the `limit` the Phase 7 grid
/// measures at, so grid rows stay comparable to the gated numbers.
pub(crate) const K: i64 = 5;

/// Reads and parses an eval set. Shared with `rerank audit`-style tools
/// and the Phase 7 grid / distribution measurements, which all score the
/// same text the quality gates score against.
pub(crate) fn load_set(cases_path: &Path) -> Result<EvalSet> {
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
    Ok(set)
}

#[derive(Debug, Deserialize)]
pub(crate) struct EvalSet {
    #[serde(default, rename = "memory")]
    pub(crate) memories: Vec<SeedMemory>,
    #[serde(default, rename = "case")]
    pub(crate) cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SeedMemory {
    pub(crate) content: String,
    pub(crate) category: String,
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
    /// How old this memory is at [`eval_now`], in days.
    ///
    /// A real store is a pile of memories written months apart, and recency
    /// is a multiplier on its rank score, so age decides results. Left unset
    /// the memory gets a stable pseudo-random age derived from its content
    /// (see [`seeded_age_days`]), which is what makes the eval behave like a
    /// real store without anyone hand-ageing a hundred entries.
    ///
    /// **Set it explicitly wherever the corpus encodes an order** — a
    /// superseded fact, a "current" fact, a distractor that is meant to be
    /// the older of two lookalikes. A hash cannot know which of a pair is
    /// supposed to be newer, and an age assigned the wrong way round turns a
    /// case about relevance into a case about an accident (Task 0.2).
    #[serde(default)]
    age_days: Option<f32>,
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
pub(crate) struct Case {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) expect: Vec<String>,
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
    pub(crate) expect_empty: bool,
    /// Reported everywhere but excluded from the numbers a committed
    /// baseline gates against, so a deliberately-failing regression case
    /// (reranking-plan.md Phase 0.5: `nl-*` and `negative` before the fix)
    /// documents the defect without reddening CI. Phase 2 removes the
    /// flag once the defects are fixed and the baseline re-recorded.
    #[serde(default)]
    informational: bool,
}

/// What one case scored. Opaque outside `bootstrap::eval`: the grid and
/// the distribution measurement collect these and hand them back to
/// [`summarise`], the single definition of what a run's numbers mean.
#[derive(Debug, Clone)]
pub(crate) struct CaseResult {
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
    /// Whether *every* expectation landed in the top K, not merely the first
    /// one (recall-quality-plan.md Task 0.4, V3).
    ///
    /// The distinction is the difference between "an agent got an answer" and
    /// "an agent got the whole answer", and it is what a multi-hop question
    /// turns on: for `who leads the team that maintains the billing service`,
    /// the bridge fact and the name together are the answer, and finding only
    /// one of them is not the success the case means. `recall` already counts
    /// that as 50%; `complete` says it plainly instead of as a fraction a
    /// per-kind mean rounds away.
    ///
    /// For an `expect_empty` case it carries the pass bit, like `recall` and
    /// `top_hit`: nothing came back, so every expectation there was (namely
    /// none) was met.
    complete: bool,
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
    /// Fraction of the *scored* cases whose every expected memory landed in
    /// the top K, 0–100 (recall-quality-plan.md Task 0.4).
    ///
    /// This is the completeness half of the story the other three hide. A
    /// multi-hop case wants both the bridge fact and the name: recall@5 calls
    /// finding one of them 50%, p@1 calls it a miss, MRR never sees the second
    /// one at all. A fix that stitches a two-hop answer together therefore
    /// moves *only* this number — which is why Phase 0 measures it before any
    /// phase claims a gain.
    #[serde(default)]
    pub complete_at_k: f64,
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
    /// complete@k per `kind`, for the same reason: the `relational` and `chain`
    /// kinds are where completeness is the whole question, and an overall mean
    /// lets one of them collapse while the other improves.
    #[serde(default)]
    pub complete_at_k_by_kind: BTreeMap<String, f64>,
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
    let set = load_set(cases_path)?;
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
    // And which legs are in the pipeline being measured: with the graph off
    // there is no hop and no seeded edge, so the `relational` numbers describe
    // a two-leg recall. A row that moved them had better say so out loud.
    if memories.graph.is_none() {
        println!(
            "graph: off (`[graph].enabled = false` — relational cases read the two-leg pipeline)\n"
        );
    }

    // One untimed recall before timing any: the first call pays
    // connection setup (remote providers), index open and cache misses,
    // and with ~40 samples a single cold call lands straight in p95.
    let warmup = RecallQuery::new("warm up", 1)?;
    // Untracked, like every replay in this process (reranking-plan.md
    // Phase 6.1): a warmup is not a user reading their memories, and
    // stamping them as accessed would feed importance decay from a
    // measurement run — throwaway store or not.
    memories.recaller.execute_untracked(&context, &warmup)?;

    let mut results = Vec::with_capacity(set.cases.len());
    for case in &set.cases {
        results.push(score(
            &memories.recaller,
            memories.min_relevance,
            &context,
            case,
        )?);
    }

    print_report(&results);

    let mut gated = summarise(&gate(&results));
    gated.embedder = embedder;
    Ok(gated)
}

/// Reject a corpus or case set that cannot measure what it claims to.
///
/// Runs at load time rather than only in the committed-file parse test, so a
/// hand-edited set fails where the mistake was made. Four rules:
///
/// * an `expect`-less non-negative case divides by zero and poisons every
///   mean as NaN, and an `expect_empty` case with expectations has
///   contradicted itself;
/// * two definitions of the same `content` (V1) are not two memories — the
///   second takes a slot in every top-5 list the first reaches, so the eval
///   scores the corpus's own repetition instead of retrieval;
/// * a `current-fact` case whose expected memory is *older* than another
///   memory about the same entity is not testing whether the current fact
///   wins: the recency multiplier hands it the ranking on a plate, or
///   takes it away, and the case measures an accident (Task 0.2);
/// * a `vocab-gap` case whose expectation shares a content word with the
///   query is not testing a vocabulary gap either — it is an ordinary case
///   wearing a label that makes the phase-5 numbers mean something they do
///   not (Task 0.7).
pub(crate) fn validate(set: &EvalSet, path: &Path) -> Result<()> {
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

    if let Some((content, first, second)) = find_duplicate_content(&set.memories) {
        issues.push(format!(
            "memory {content:?} is defined twice (entries {} and {}) — a duplicate takes a slot \
             in every top-5 list its twin reaches, so the run scores the corpus's repetition \
             rather than retrieval. Delete one definition",
            first + 1,
            second + 1
        ));
    }

    for memory in &set.memories {
        // `seeded_at` turns days into seconds and chrono will not carry a NaN,
        // so a nonsense age here would be a panic or a bogus `created_at` in
        // the middle of a seeding run that looks like a harness bug.
        if let Some(age) = memory.age_days {
            if !age.is_finite() || age < 0.0 || age > MAX_AGE_DAYS {
                issues.push(format!(
                    "memory {:?} has age_days {age} — the corpus ages memories in years, not \
                     infinities: use a finite number in `0..={MAX_AGE_DAYS:.0}`",
                    truncate(&memory.content, 48)
                ));
            }
        }
    }

    for case in set
        .cases
        .iter()
        .filter(|case| case.kind == CURRENT_FACT_KIND)
    {
        if let Some(issue) = superseded_current_fact(set, case) {
            issues.push(issue);
        }
    }

    for case in set.cases.iter().filter(|case| case.kind == VOCAB_GAP_KIND) {
        if let Some(issue) = vocabulary_gap_named_in_query(set, case) {
            issues.push(issue);
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

/// The first content defined by two `[[memory]]` blocks, with their indices.
fn find_duplicate_content(memories: &[SeedMemory]) -> Option<(&str, usize, usize)> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, memory) in memories.iter().enumerate() {
        if let Some(first) = seen.insert(memory.content.as_str(), index) {
            return Some((&memory.content, first, index));
        }
    }
    None
}

/// Why a `current-fact` case does not actually measure "the current fact
/// wins", if it does not.
///
/// The subject of a memory is whatever it declares as an entity — the graph
/// joins on exactly that key, so "memories sharing its subject entity" is
/// `EntityKey` identity here rather than a judgement about wording. An
/// expected memory with no declared entities has no subject to compare
/// against, and the rule has nothing to say about it.
fn superseded_current_fact(set: &EvalSet, case: &Case) -> Option<String> {
    let keys_of = |memory: &SeedMemory| -> Vec<String> {
        memory
            .entities
            .iter()
            .map(|entity| EntityKey::new(&entity.name).as_str().to_string())
            .collect()
    };

    for wanted in &case.expect {
        let expected = set
            .memories
            .iter()
            .find(|memory| &memory.content == wanted)?;
        let subjects = keys_of(expected);
        if subjects.is_empty() {
            continue;
        }
        let expected_age = seeded_age_days(expected);
        for other in &set.memories {
            if &other.content == wanted {
                continue;
            }
            let newer = seeded_age_days(other) < expected_age;
            let same_subject = keys_of(other).iter().any(|key| subjects.contains(key));
            if newer && same_subject {
                return Some(format!(
                    "case {:?} (kind {CURRENT_FACT_KIND}) expects {:?} to be the current fact, \
                     but {:?} speaks of the same entity and is seeded newer ({:.0} days old vs \
                     {:.0}) — say which one is meant to be current by setting `age_days`",
                    case.name,
                    truncate(&expected.content, 48),
                    truncate(&other.content, 48),
                    seeded_age_days(other),
                    expected_age
                ));
            }
        }
    }
    None
}

/// The kind whose whole premise is that the query shares no content word
/// with the memory that answers it (recall-quality-plan.md Task 0.7).
const VOCAB_GAP_KIND: &str = "vocab-gap";

/// Grammar, not topic: a `vocab-gap` case that overlaps with its answer on
/// one of these still shares no vocabulary. Deliberately short — it holds the
/// function words this corpus writes, because a longer list's only effect is
/// to hide a shared word that was worth seeing.
const FUNCTION_WORDS: &[&str] = &[
    "a", "after", "and", "any", "are", "as", "at", "be", "before", "but", "by", "can", "did", "do",
    "does", "for", "from", "had", "has", "have", "how", "if", "in", "into", "is", "it", "its",
    "me", "my", "no", "not", "of", "on", "or", "our", "out", "so", "than", "that", "the", "their",
    "them", "then", "there", "these", "they", "this", "to", "up", "us", "was", "we", "were",
    "what", "when", "where", "which", "who", "why", "will", "with", "would", "you", "your",
];

/// The topical words of a piece of text: lowercased runs of alphanumerics,
/// minus [`FUNCTION_WORDS`] and single letters.
///
/// Coarser on purpose than the index's analyzer (which tokenises per field and
/// drops its own stopwords): the question here is "do these two sentences talk
/// about the same thing using the same words", and being stricter than the
/// analyzer means a case can only pass this check by sharing nothing at all.
fn content_words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() > 1 && !FUNCTION_WORDS.contains(word))
        .map(str::to_string)
        .collect()
}

/// Whether a `vocab-gap` case names its own answer.
///
/// The kind exists to measure the answer whose wording has nothing in common
/// with the question — Phase 5's ingest-time probes are scored on exactly that,
/// and §0.9 freezes these cases so it cannot be done from them. A case that
/// shares a content word measures ordinary lexical matching while reporting as
/// a vocabulary gap, so the claim gets checked the way the `current-fact`
/// ages are: at load, where the mistake was made.
fn vocabulary_gap_named_in_query(set: &EvalSet, case: &Case) -> Option<String> {
    let asked = content_words(&case.query);
    for wanted in &case.expect {
        let Some(answer) = set.memories.iter().find(|memory| &memory.content == wanted) else {
            continue; // an expectation with no memory is the parse test's to report
        };
        // Content, tags and category: the three fields the keyword leg's query
        // parser is built over, so a shared word in any of them is one a real
        // request can match on.
        let visible = format!(
            "{} {} {}",
            answer.content,
            answer.tags.join(" "),
            answer.category
        );
        let seen = content_words(&visible);
        let shared: Vec<&String> = seen.intersection(&asked).collect();
        if !shared.is_empty() {
            return Some(format!(
                "case {:?} is kind {VOCAB_GAP_KIND}, which claims nothing in the query appears in \
                 the memory that answers it, but they share {} — reword one side, or move the case \
                 to the kind it actually measures",
                case.name,
                shared
                    .iter()
                    .map(|word| format!("{word:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    None
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

/// The kind whose whole premise is that the memory it expects is the newest
/// thing the corpus says about its subject.
const CURRENT_FACT_KIND: &str = "current-fact";

/// The oldest age the corpus accepts, in days: ten years.
///
/// Past the recency band's floor a year already says everything the multiplier
/// will say, so an age measured in decades is a typo rather than a fact, and
/// `seeded_at` would be moving `now` by an implausible span.
const MAX_AGE_DAYS: f32 = 3_650.0;

/// The age, in days at [`eval_now`], that this corpus entry is seeded with.
///
/// The explicit `age_days` wins; otherwise the age is derived from the
/// content by [`hashed_age_days`], so an entry without an age is still not
/// left to chance — it is just left to a rule instead of to a run.
pub(crate) fn seeded_age_days(seed: &SeedMemory) -> f32 {
    seed.age_days
        .unwrap_or_else(|| hashed_age_days(&seed.content))
}

/// A stable pseudo-random age in `[0, 365)` days, derived from the bytes of
/// the memory's content by FNV-1a.
///
/// The eval needs a corpus whose memories are not all the same age (Task
/// 0.2), and it needs that *without* a seeded RNG: an `StdRng::seed_from_u64`
/// is stable only while the iteration order and the number of draws are, so
/// inserting one memory anywhere in the file re-ages every memory after it
/// and the baseline moves for a reason nobody can read. A hash of the content
/// has no global state at all: add, remove or reorder entries and every
/// untouched entry keeps the age it had.
///
/// 365 days because that is a year of a personal store, and four times the
/// configured 90-day recency half-life: the whole floored band gets exercised
/// rather than just its fresh end. Folding into *seconds* rather than days
/// keeps two different contents from landing on the same instant (a hundred
/// entries over 365 buckets would collide constantly, and a collision is an exact
/// score tie — the thing this task exists to remove).
fn hashed_age_days(content: &str) -> f32 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    const SECONDS_IN_A_YEAR: u64 = 365 * 86_400;

    let mut hash = FNV_OFFSET;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    (hash % SECONDS_IN_A_YEAR) as f32 / 86_400.0
}

/// The instant a memory of `age_days` is stamped with: [`eval_now`] minus its
/// age. Whole seconds, which is the resolution the ranker ages at anyway.
fn seeded_at(age_days: f32) -> DateTime<Utc> {
    let seconds = (f64::from(age_days) * 86_400.0).round() as i64;
    eval_now() - chrono::Duration::try_seconds(seconds).expect("an eval age fits in a chrono span")
}

/// Builds a throwaway instance and fills it with the corpus.
///
/// The `TempDir` comes back so the caller holds it: dropping it deletes
/// the database and the tantivy indexes out from under the run.
///
/// The instance is assembled from the caller's `config` — embedder,
/// relevance floor, ranker tuning, graph on or off, and all — with exactly
/// one override: the storage path, so the run can never touch a real store.
/// Shared with the Phase 7 grid, which measures every cell against one
/// seeded corpus rather than reseeding per cell.
///
/// **Time.** The instance is wired to a [`FixedClock`] rather than the wall
/// clock, and that clock is left at [`eval_now`] when this returns: every
/// memory's `created_at` is [`eval_now`] minus its `age_days`, and every
/// score is computed against [`eval_now`] (Tasks 0.1 and 0.2). The same
/// corpus therefore produces the same recency multiplier in every run, on
/// every machine.
pub(crate) fn seed(
    set: &EvalSet,
    config: &AppConfig,
) -> Result<(Memories, UserContext, tempfile::TempDir)> {
    let scratch = tempfile::tempdir()
        .map_err(|e| RaError::Internal(format!("could not create a scratch directory: {e}")))?;

    let mut config = config.clone();
    config.storage.path = scratch.path().to_string_lossy().to_string();
    // The graph is whatever the caller's config says — the harness does not
    // override it. Since Task 7.3.6 the shipped default is on, so the
    // `relational` cases and the edges the corpus carries measure the graph
    // leg by default; and because the switch is not pinned here,
    // `scripts/recall-grid.sh` can measure `[graph].enabled` as the knob it
    // is (recall-quality-plan.md Task 0.6). A harness that decided a knob it
    // is being asked to measure would report its own override as a result.

    let database = Arc::new(SqliteDatabase::open(
        &scratch.path().join(crate::bootstrap::wiring::DATABASE_FILE),
    )?);
    let identity = Identity::from_database(Arc::clone(&database))?;
    // One clock for the whole instance. The saver stamps `created_at` from
    // it, the ranker ages against it, and the graph's `valid_from` is the
    // memory's `created_at`, so this handle is what makes "aged corpus,
    // fixed now" true for every part of the pipeline at once.
    let clock = FixedClock::at(eval_now());
    let memories = Memories::build_with_clock(&config, database, Arc::new(clock.clone()))?;

    // A real key, authenticated for real: the eval must exercise the same
    // scoping every request does, or it could pass against a store no
    // client could actually read.
    identity.user_creator.execute("eval", None)?;
    let issued =
        identity
            .api_key_issuer
            .execute("eval", vec![Scope::Read, Scope::Write], "eval")?;
    let context = identity.key_authenticator.execute(&issued.token.render())?;

    // With the graph on, a missing handle would be a wiring bug rather than a
    // config choice, and stopping loudly is the cheap fix: a corpus seeded
    // without edges scores every `relational` case as a silent zero, which is
    // the worst kind of green. With it off, ingest records no edges — exactly
    // what a deployment with `[graph].enabled = false` does — and the
    // relational cases measure the two-leg pipeline, which is the point of
    // measuring that knob at all.
    let graph = match (config.graph.enabled, memories.graph.as_ref()) {
        (true, graph) => {
            Some(graph.expect("the config has the graph on, so Memories must have built one"))
        }
        (false, _) => None,
    };

    for seed in &set.memories {
        // Time-travel, then save: `created_at` comes off this clock, and the
        // graph edge below inherits it as `valid_from`, so one move ages the
        // memory and its relationships together (Task 0.2).
        clock.set(seeded_at(seeded_age_days(seed)));

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

        // No graph configured, no edges recorded: exactly what a deployment
        // with `[graph].enabled = false` stores, and the hole the
        // `relational` cases then measure.
        if let Some(graph) = graph {
            graph.record(
                &context,
                stored.id(),
                &entities,
                &relations,
                stored.created_at(),
            )?;
        }
    }

    // Back to the instant every score is measured at. Leaving it on the last
    // seed's age would make the whole run's recency depend on whichever entry
    // happened to be last in the file.
    clock.set(eval_now());

    Ok((memories, context, scratch))
}

/// Scores one case against one already-seeded pipeline.
///
/// The recaller is a parameter (not read off [`Memories`]) because the
/// Phase 7 grid clones the seeded pipeline per cell — every cell differs
/// from the reference only in its second stage, which is the property a
/// paired measurement exists to isolate. `min_relevance` is the instance
/// floor, applied to negative cases only (see below).
pub(crate) fn score(
    recaller: &MemoryRecaller,
    min_relevance: f32,
    context: &UserContext,
    case: &Case,
) -> Result<CaseResult> {
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
        query = query.with_min_relevance(min_relevance);
    }

    let started = std::time::Instant::now();
    // Untracked (reranking-plan.md Phase 6.1): the eval replays queries over
    // memories it seeded, and a replay that recorded each hit as an access
    // would move the counters importance decay reads — so scoring the same
    // corpus twice would not measure the same thing twice.
    let report = recaller.execute_untracked_traced(context, &query)?;
    let hits = report.results;
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    // Once per run, because this is the worst kind of green: an eval whose
    // second stage never finished reports reranking's measured gain as 0.0 and
    // its cost as nothing. The gain was +5.8 p@1 precisely because it *ran*.
    if matches!(report.rerank, Some(RerankTrace::Skipped { .. }))
        && !RERANK_SKIP_WARNED.swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        eprintln!(
            "⚠ rerank stage skipped ({}) — the numbers below describe the pre-rerank order",
            match report.rerank {
                Some(RerankTrace::Skipped {
                    reason: RerankSkip::Deadline,
                    busy: true,
                    ..
                }) => "no session was free — [rerank].max_concurrency",
                Some(RerankTrace::Skipped {
                    reason: RerankSkip::Deadline,
                    ..
                }) => "past [rerank].timeout_ms",
                _ => "model failure",
            }
        );
    }
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
            complete: passed,
            reciprocal_rank: None,
            latency_ms,
            expect_empty: true,
            missed: if passed {
                Vec::new()
            } else {
                vec![format!(
                    "(expected no results at the {min_relevance:?} floor; got {})",
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
        // `missed` is computed against the returned top K, so "nothing missed"
        // is exactly "every expectation was inside it". One expected memory
        // makes this the same bit as recall == 1.0; two is where it earns its
        // place (see the field doc).
        complete: missed.is_empty(),
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

/// The report one set of case results produces — the single definition
/// of recall/precision/MRR/latency means, shared by the bare `eval` run
/// and every Phase 7 grid cell.
pub(crate) fn summarise(results: &[CaseResult]) -> Report {
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
    let mut complete_by_kind: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for result in results {
        recall_by_kind
            .entry(result.kind.clone())
            .or_default()
            .push(result.recall);
        precision_by_kind
            .entry(result.kind.clone())
            .or_default()
            .push(if result.top_hit { 1.0 } else { 0.0 });
        complete_by_kind
            .entry(result.kind.clone())
            .or_default()
            .push(if result.complete { 1.0 } else { 0.0 });
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
        // Scored cases only, like the other three ranking means: a negative's
        // `complete` bit is its abstain pass, and abstentions already have
        // `abstention_rate` rather than a place in this average.
        complete_at_k: mean(
            &scored
                .iter()
                .map(|r| if r.complete { 1.0 } else { 0.0 })
                .collect::<Vec<_>>(),
        ),
        latency_ms_p50: percentile(&latencies, 50.0).unwrap_or(0.0),
        latency_ms_p95: percentile(&latencies, 95.0).unwrap_or(0.0),
        precision_at_1_by_kind: precision_by_kind
            .into_iter()
            .map(|(kind, values)| (kind, mean(&values)))
            .collect(),
        complete_at_k_by_kind: complete_by_kind
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
/// Shared with `rerank audit`, which reports the same p50/p95 pair for the
/// rerank stage (reranking-plan.md Phase 6.2).
pub(crate) fn percentile(values: &[f64], p: f64) -> Option<f64> {
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
        "recall@{K}: {:.1}%   precision@1: {:.1}%   MRR: {:.1}%   complete@{K}: {:.1}%   latency p50: {:.1} ms p95: {:.1} ms   ({} scored cases, {} negative)",
        report.recall_at_k,
        report.precision_at_1,
        report.mrr,
        report.complete_at_k,
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
        let complete = report.complete_at_k_by_kind.get(kind).unwrap_or(&0.0);
        println!(
            "  {kind:>16}: recall {score:.1}%   p@1 {precision:.1}%   complete@{K} {complete:.1}%"
        );
    }
}

/// Shortens to `limit` characters with an ellipsis, counting *characters*
/// so a memory containing an emoji cannot be cut mid-codepoint. Shared with
/// `rerank audit`, which prints the same kind of one-line snippet.
pub(crate) fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars()
        .take(limit.saturating_sub(1))
        .collect::<String>()
        + "…"
}

/// A filter-less [`Case`] for sibling-module tests (`rerank_grid`,
/// `rerank_distribution`) that build a small eval set in code instead of
/// reading the committed file. Lives here so those modules never need
/// `Case`'s private filter fields, and disappears from the binary with
/// the test cfg.
#[cfg(test)]
pub(crate) fn test_case(
    name: &str,
    kind: &str,
    query: &str,
    expect: &[&str],
    expect_empty: bool,
) -> Case {
    Case {
        name: name.to_string(),
        kind: kind.to_string(),
        query: query.to_string(),
        expect: expect.iter().map(|content| content.to_string()).collect(),
        categories: Vec::new(),
        tags: Vec::new(),
        subcategories: Vec::new(),
        expect_empty,
        informational: false,
    }
}

/// A plain [`SeedMemory`] (no entities, tags or subcategory) — see
/// [`test_case`] for why these builders live in this module.
#[cfg(test)]
pub(crate) fn test_memory(content: &str, category: &str) -> SeedMemory {
    SeedMemory {
        content: content.to_string(),
        category: category.to_string(),
        subcategory: None,
        tags: Vec::new(),
        entities: Vec::new(),
        relations: Vec::new(),
        age_days: None,
    }
}

/// [`test_memory`] with the two things the age rules read: declared entities
/// (the subject a `current-fact` case is compared on) and an explicit age.
#[cfg(test)]
pub(crate) fn test_memory_about(
    content: &str,
    category: &str,
    entities: &[&str],
    age_days: f32,
) -> SeedMemory {
    SeedMemory {
        entities: entities
            .iter()
            .map(|name| SeedEntity {
                name: name.to_string(),
                kind: String::new(),
            })
            .collect(),
        age_days: Some(age_days),
        ..test_memory(content, category)
    }
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

    let gated: Vec<(String, f64, f64)> = vec![
        (
            "precision@1".to_string(),
            baseline.precision_at_1,
            report.precision_at_1,
        ),
        ("MRR".to_string(), baseline.mrr, report.mrr),
        // Task 0.4's completeness metric, gated like the others so a change
        // that stops answering multi-hop questions in full cannot pass on a
        // flat p@1. `K` is interpolated rather than spelled out: the metric
        // means "every expectation inside the top K", and a future K change
        // must move the label with it.
        (
            format!("complete@{K}"),
            baseline.complete_at_k,
            report.complete_at_k,
        ),
    ];

    for (metric, before, after) in &gated {
        if *before == 0.0 {
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
    // Completeness too, and printed only on a drop like the recall line: a
    // `chain` kind that stops completing two-hop answers while its p@1 holds
    // steady is the regression this metric exists to catch.
    for (kind, before) in &baseline.complete_at_k_by_kind {
        if let Some(after) = report.complete_at_k_by_kind.get(kind) {
            let kind_drop = before - after;
            if kind_drop > max_drop {
                println!(
                    "  {kind}: complete@{K} {before:.1}% → {after:.1}% ({kind_drop:.1} points worse)"
                );
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
    // Test-only: the corpus-shape test below reads the same candidate scanner the
    // pipeline seeds a hop from. Importing it here rather than at the top of the
    // module keeps the shipping build from carrying an import nothing there reads.
    use crate::memories::application::memory_recaller::seed_candidates;

    fn result(kind: &str, recall: f64, top_hit: bool) -> CaseResult {
        CaseResult {
            name: format!("{kind} case"),
            kind: kind.to_string(),
            informational: false,
            recall,
            top_hit,
            // The helper has no expectation list to check against, so it can
            // only say what the recall number implies: every expectation found
            // is recall 1.0. A test that needs the two to disagree — a
            // two-expectation case that found one — overrides this bit.
            complete: recall >= 1.0,
            // A case with a top hit has its expected memory at rank 1.
            reciprocal_rank: if top_hit { Some(1.0) } else { None },
            latency_ms: 10.0,
            expect_empty: false,
            missed: vec![],
        }
    }

    /// A negative case's scored stand-in: pass/fail lives entirely in the
    /// recall bit, as `score` writes it — including `complete`, which for
    /// these carries the same abstain-pass bit (see `score`'s `expect_empty` arm).
    fn negative(passed: bool) -> CaseResult {
        CaseResult {
            expect_empty: true,
            recall: if passed { 1.0 } else { 0.0 },
            top_hit: passed,
            complete: passed,
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
    fn complete_at_k_counts_a_two_part_answer_whole_or_not_at_all() {
        // Task 0.4's whole point in three cases: a two-hop question that found
        // one of its two memories is a *failed* answer here, where recall@5
        // books it as a half and MRR never looks at the second half at all.
        let half_a_chain = CaseResult {
            complete: false,
            ..result("chain", 0.5, true)
        };
        let whole_chain = CaseResult {
            complete: true,
            ..result("chain", 1.0, true)
        };
        let no_answer = CaseResult {
            complete: false,
            ..result("chain", 0.0, false)
        };

        let report = summarise(&[half_a_chain, whole_chain, no_answer]);
        assert_eq!(report.recall_at_k, 50.0, "recall still credits the half");
        // Tolerance rather than `assert_eq!`: the mean is `1.0/3.0*100.0`, and
        // writing the same number as `100.0/3.0` rounds differently in the last
        // bit. The point is one-of-three, not the ULPs.
        let one_of_three = 100.0 / 3.0;
        assert!(
            (report.complete_at_k - one_of_three).abs() < 1e-9,
            "{}",
            report.complete_at_k
        );
        assert!(
            (report.complete_at_k_by_kind["chain"] - one_of_three).abs() < 1e-9,
            "{:?}",
            report.complete_at_k_by_kind
        );
    }

    #[test]
    fn a_single_expectation_completes_wherever_recall_reads_one() {
        // The degenerate case, pinned because it is most of the eval: with one
        // expected memory, complete and recall agree even when the answer is
        // buried at rank 5 — p@1 is the metric that disagrees there.
        let buried = CaseResult {
            complete: true,
            reciprocal_rank: Some(0.2),
            ..result("needle", 1.0, false)
        };
        let report = summarise(&[buried]);
        assert_eq!(report.complete_at_k, 100.0);
        assert_eq!(report.precision_at_1, 0.0);
    }

    #[test]
    fn negatives_keep_complete_out_of_the_ranking_mean() {
        // Same rule as recall/p@1/MRR (see `negatives_move_abstention_never_the_ranking_means`):
        // a flipping negative must not move the scored mean, or every phase's
        // headroom number becomes partly a function of the abstention set. It
        // does keep its own bit in the per-kind line, where `by_kind` and
        // `precision_at_1_by_kind` already read negatives the same way.
        let report = summarise(&[
            result("paraphrase", 1.0, true),
            negative(true),
            negative(false),
        ]);
        assert_eq!(report.complete_at_k, 100.0, "one scored case, complete");
        assert_eq!(report.abstention_rate, 50.0);
        assert_eq!(report.complete_at_k_by_kind["negative"], 50.0);
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
    fn an_old_format_baseline_still_deserialises() {
        // Invariant I-5 of reranking-plan.md, pinned *inline* now that
        // Phase 2.5 re-recorded the committed baseline with the newer
        // fields: a baseline predating mrr/latency/precision_at_1_by_kind
        // must keep loading (the old committed file was exactly this
        // shape — recall, precision, cases, by_kind — and operators keep
        // older ones in the wild).
        const OLD: &str = r##"{"recall_at_k":90.0,"precision_at_1":60.0,"cases":24,"by_kind":{"paraphrase":87.5}}"##;
        let baseline: Report =
            serde_json::from_str(OLD).expect("an old-format baseline must still deserialise");
        assert!(baseline.recall_at_k > 0.0);
        assert_eq!(baseline.mrr, 0.0, "absent in an old baseline");
        assert_eq!(baseline.latency_ms_p50, 0.0);
        assert!(baseline.precision_at_1_by_kind.is_empty());
        assert_eq!(baseline.complete_at_k, 0.0, "absent in an old baseline");
        assert!(baseline.complete_at_k_by_kind.is_empty());
        assert!(
            baseline.embedder.is_empty(),
            "the field postdates old baselines"
        );
    }

    #[test]
    fn the_committed_baseline_records_the_metrics_phase_2_added() {
        // Phase 2.5 re-recorded `eval/baseline.json`; the DoD is that the
        // p@1, MRR and latency fields are *populated*, so a future
        // regression in any of them — not just recall — fails the gate.
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("eval/baseline.json"),
        )
        .expect("eval/baseline.json should exist");
        let baseline: Report =
            serde_json::from_str(&raw).expect("the committed baseline must deserialise");
        assert!(baseline.recall_at_k > 0.0);
        assert!(baseline.precision_at_1 > 0.0);
        assert!(baseline.mrr > 0.0, "re-recorded baselines carry MRR");
        assert!(baseline.latency_ms_p50 > 0.0);
        assert!(!baseline.precision_at_1_by_kind.is_empty());
        assert!(!baseline.embedder.is_empty());
        // Phase 2.5 also removed the informational flags, so the kinds
        // that were reporting-only are in the gate now.
        assert!(baseline.by_kind.contains_key("negative"));
        assert!(baseline.by_kind.contains_key("nl-question"));
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
    fn complete_at_k_gates_once_recorded_and_is_skipped_before_that() {
        // Task 0.4's DoD both ways. `eval/baseline.json` predates the field, so
        // the first run after it must not be failed by a metric it never
        // recorded; the run after re-recording must be.
        let unrecorded = tempfile::NamedTempFile::new().unwrap();
        write_baseline(
            &Report {
                recall_at_k: 80.0,
                precision_at_1: 70.0,
                mrr: 60.0,
                ..Report::default()
            },
            unrecorded.path(),
        )
        .unwrap();
        let collapsed = Report {
            complete_at_k: 10.0,
            ..measured_with_ranking_means()
        };
        assert!(
            compare(&collapsed, unrecorded.path(), 5.0).is_ok(),
            "a baseline that never measured completeness cannot gate on it"
        );

        let recorded = tempfile::NamedTempFile::new().unwrap();
        write_baseline(
            &Report {
                complete_at_k: 80.0,
                ..measured_with_ranking_means()
            },
            recorded.path(),
        )
        .unwrap();
        let err = compare(&collapsed, recorded.path(), 5.0)
            .expect_err("a 70-point completeness collapse must fail the gate");
        assert!(
            err.to_string().contains("complete@5"),
            "the failure should name the metric: {err}"
        );
    }

    /// A report whose three pre-existing ranking means are all set, for tests
    /// that gate on a *newer* metric and must not trip the older ones on the
    /// way (a default `Report` has 0.0 p@1 and MRR, which read as "unrecorded"
    /// and make such a test pass for the wrong reason).
    fn measured_with_ranking_means() -> Report {
        Report {
            recall_at_k: 80.0,
            precision_at_1: 70.0,
            mrr: 60.0,
            ..Report::default()
        }
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

    // --- ages: Task 0.1's clock and Task 0.2's corpus ---------------------

    #[test]
    fn an_unaged_memory_gets_the_same_age_in_every_run() {
        // The default has to look like a real store (ages spread over the
        // year) and behave like a constant (identical across runs, machines,
        // and edits to *other* entries). An RNG gives the first and loses the
        // last two, which is why this is a hash of the content.
        let pnpm = test_memory("User prefers pnpm over npm and yarn", "preference.coding");
        let standups = test_memory("Standups are at 09:30 UTC on Tuesdays", "fact.project");

        assert_eq!(
            seeded_age_days(&pnpm),
            seeded_age_days(&pnpm),
            "the age must not depend on how many times it is asked"
        );
        assert!(
            (0.0..365.0).contains(&seeded_age_days(&pnpm)),
            "an unaged memory must land inside one year, got {}",
            seeded_age_days(&pnpm)
        );
        assert_ne!(
            seeded_age_days(&pnpm),
            seeded_age_days(&standups),
            "two different memories must not share an instant — that is a score tie again"
        );

        // Order-independence, spelled as the property that matters: an entry's
        // age is a function of its own content, so a corpus that gains or
        // reorders entries leaves every unchanged entry where it was. Under an
        // RNG seeded once per run, inserting one memory re-ages the whole file
        // and the baseline moves for a reason nobody can read.
        let corpus = |extra: bool| -> Vec<f32> {
            let mut memories = vec![
                test_memory("User prefers pnpm over npm and yarn", "preference.coding"),
                test_memory("Standups are at 09:30 UTC on Tuesdays", "fact.project"),
            ];
            if extra {
                memories.insert(0, test_memory("Nadia is the tech lead", "fact.person"));
            }
            memories
                .iter()
                .skip(memories.len() - 2)
                .map(seeded_age_days)
                .collect()
        };
        assert_eq!(
            corpus(false),
            corpus(true),
            "the age of an entry moved when an *unrelated* \
                                                 entry joined the corpus"
        );
    }

    #[test]
    fn an_explicit_age_beats_the_hash_and_seeds_at_eval_now_minus_itself() {
        let aged = SeedMemory {
            age_days: Some(30.0),
            ..test_memory("The analytics pipeline was built on Kafka", "fact.project")
        };
        assert_eq!(
            seeded_age_days(&aged),
            30.0,
            "the corpus must win over the hash"
        );
        assert_eq!(
            seeded_at(30.0),
            eval_now() - chrono::Duration::days(30),
            "30 days old means created 30 days before the eval's now"
        );
        // Half days survive: the ranker ages in seconds, so 0.5 d is not
        // rounded away into a tie with a memory aged 0.
        assert_eq!(
            seeded_at(0.5),
            eval_now() - chrono::Duration::try_seconds(43_200).unwrap()
        );
        assert_eq!(seeded_at(0.0), eval_now());
    }

    #[test]
    fn the_committed_corpus_is_aged_like_a_real_store() {
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("eval/cases.toml"),
        )
        .expect("eval/cases.toml should exist");
        let set: EvalSet = toml::from_str(&raw).expect("eval/cases.toml should parse");

        let ages: Vec<f32> = set.memories.iter().map(seeded_age_days).collect();
        let youngest = ages.iter().copied().fold(f32::MAX, f32::min);
        let oldest = ages.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            youngest < 30.0 && oldest > 300.0,
            "the corpus spans {youngest:.0}–{oldest:.0} days: the recency band is not exercised"
        );
        let distinct: std::collections::BTreeSet<i64> =
            ages.iter().map(|age| (age * 86_400.0) as i64).collect();
        assert!(
            distinct.len() >= set.memories.len() - 4,
            "{} of {} memories share a seeded instant — exact score ties are back",
            set.memories.len() - distinct.len(),
            set.memories.len()
        );
    }

    #[test]
    fn validate_rejects_a_memory_defined_twice() {
        // V1: the duplicate does not add a memory, it adds a second slot in
        // every top-5 list its twin reaches.
        let set = EvalSet {
            memories: vec![
                test_memory(
                    "User prefers Vitest over Jest for anything new",
                    "preference.coding",
                ),
                test_memory("Standups are at 09:30 UTC", "fact.project"),
                test_memory(
                    "User prefers Vitest over Jest for anything new",
                    "preference.coding",
                ),
            ],
            cases: vec![case("scored", vec!["Standups are at 09:30 UTC"], false)],
        };

        let error = validate(&set, Path::new("x.toml")).unwrap_err().to_string();
        assert!(error.contains("defined twice"), "{error}");
        assert!(error.contains("Vitest"), "{error}");
        assert!(error.contains("entries 1 and 3"), "{error}");
    }

    #[test]
    fn validate_rejects_a_vocab_gap_case_that_names_its_own_answer() {
        // The kind's whole claim is that the question cannot lean on a shared
        // word. Let one through and every Phase 5 number measured on it means
        // something other than what it says.
        let set = EvalSet {
            memories: vec![test_memory(
                "Catering for the Lisbon office excludes shellfish",
                "fact.project",
            )],
            cases: vec![Case {
                kind: "vocab-gap".to_string(),
                query: "what about shellfish at the team lunch".to_string(),
                informational: true,
                ..case(
                    "dietary",
                    vec!["Catering for the Lisbon office excludes shellfish"],
                    false,
                )
            }],
        };

        let error = validate(&set, Path::new("x.toml")).unwrap_err().to_string();
        assert!(error.contains("vocab-gap"), "{error}");
        assert!(error.contains("\"shellfish\""), "{error}");
    }

    #[test]
    fn a_vocab_gap_case_sharing_only_grammar_still_validates() {
        // "for" and "a" appear on both sides and mean nothing: the topical
        // vocabulary is still disjoint, which is what the kind is about.
        let set = EvalSet {
            memories: vec![test_memory(
                "Catering for the Lisbon office excludes shellfish",
                "fact.project",
            )],
            cases: vec![Case {
                kind: "vocab-gap".to_string(),
                query: "are there dietary requirements for a team lunch".to_string(),
                informational: true,
                ..case(
                    "dietary",
                    vec!["Catering for the Lisbon office excludes shellfish"],
                    false,
                )
            }],
        };

        validate(&set, Path::new("x.toml")).expect("sharing only function words keeps the gap");
    }

    #[test]
    fn validate_rejects_a_current_fact_the_corpus_has_already_superseded() {
        // The `current-fact` kind asks "does the current fact win?". If the
        // corpus seeds the expected memory as the older of two about the same
        // subject, the answer is decided by the recency multiplier before the
        // ranker is asked anything, and a pass measures nothing.
        let set = |expected_age: f32| EvalSet {
            memories: vec![
                test_memory_about(
                    "The backend deploys on Hetzner",
                    "fact.project",
                    &["backend"],
                    expected_age,
                ),
                test_memory_about(
                    "The backend runs on Fly.io",
                    "fact.project",
                    &["backend"],
                    // Unconditionally newer than any age this test accepts.
                    1.0,
                ),
            ],
            cases: vec![test_case(
                "current fact: deployment target",
                "current-fact",
                "where does the backend deploy",
                &["The backend deploys on Hetzner"],
                false,
            )],
        };

        let stale = validate(&set(200.0), Path::new("x.toml"))
            .unwrap_err()
            .to_string();
        assert!(stale.contains("current-fact"), "{stale}");
        assert!(stale.contains("seeded newer"), "{stale}");

        // The same corpus with the intended order is fine, and so is a pair
        // that never speaks of the same entity.
        validate(&set(0.5), Path::new("x.toml")).expect("the current fact is the newest");
    }

    #[test]
    fn validate_rejects_an_age_that_cannot_be_seeded() {
        for age in [-1.0, f32::NAN, f32::INFINITY, 40_000.0] {
            let set = EvalSet {
                memories: vec![SeedMemory {
                    age_days: Some(age),
                    ..test_memory("Standups are at 09:30 UTC", "fact.project")
                }],
                cases: vec![case("scored", vec!["Standups are at 09:30 UTC"], false)],
            };
            let error = validate(&set, Path::new("x.toml"))
                .expect_err("{age} must not reach the seeding run");
            assert!(error.to_string().contains("age_days"), "{error}");
        }
    }

    #[test]
    #[ignore = "requires the ONNX model; run with --ignored"]
    fn seeding_stamps_each_memory_with_the_age_the_corpus_gives_it() {
        // The end-to-end half of Task 0.1+0.2: `seed` moves a clock the saver
        // and the recaller both read, so the age in the file is the age in the
        // row. Model-dependent, so ignored like the other real-model tests.
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
        let set = EvalSet {
            memories: vec![
                test_memory("The analytics pipeline was built on Kafka", "fact.project"),
                test_memory("The analytics pipeline runs on Kinesis", "fact.project"),
            ],
            cases: vec![],
        };

        let (memories, context, _scratch) = seed(&set, &config).expect("seed with ages");
        let stored = memories.repository.list(&context, true).unwrap();
        assert_eq!(stored.len(), 2);
        for memory in &stored {
            let seed = set
                .memories
                .iter()
                .find(|seed| seed.content == memory.content())
                .expect("the stored memory came from the corpus");
            assert_eq!(
                memory.created_at(),
                seeded_at(seeded_age_days(seed)),
                "{:?} was not stamped with its corpus age",
                memory.content()
            );
        }

        // And the clock is left at `eval_now`, so a memory saved after seeding
        // is brand new rather than as old as the file's last entry.
        let after = memories
            .saver
            .execute(&context, test_new_memory("written after seeding"), "test")
            .unwrap();
        assert_eq!(after.created_at(), eval_now());
    }

    /// Task 0.6, and the reason the harness no longer forces the graph on:
    /// `[graph].enabled` is a knob `scripts/recall-grid.sh` measures, so
    /// pinning it here would have the harness report its own override as a
    /// result. On, the corpus's relations become edges a hop can walk; off,
    /// seeding records none — exactly what a deployment with the graph off
    /// stores — and the memories are still there for the two legs that remain.
    #[test]
    #[ignore = "requires the ONNX model; run with --ignored"]
    fn a_graph_the_config_disables_seeds_no_edges() {
        let cache = std::env::var("RECUERDOS_AI_MODEL_CACHE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("/models"));
        let config = |enabled| AppConfig {
            embeddings: crate::bootstrap::config::EmbeddingsConfig {
                cache_dir: cache.to_string_lossy().to_string(),
                ..Default::default()
            },
            graph: crate::bootstrap::config::GraphConfig {
                enabled,
                ..Default::default()
            },
            ..AppConfig::default()
        };
        let set = EvalSet {
            memories: vec![
                SeedMemory {
                    entities: ["Kinesis", "Kafka"]
                        .iter()
                        .map(|name| SeedEntity {
                            name: name.to_string(),
                            kind: String::new(),
                        })
                        .collect(),
                    relations: vec![SeedRelation {
                        subject: "Kinesis".to_string(),
                        predicate: "replaced".to_string(),
                        object: "Kafka".to_string(),
                    }],
                    ..test_memory("The analytics pipeline runs on Kinesis", "fact.project")
                },
                test_memory("Kafka is the broker we retired", "fact.project"),
            ],
            cases: vec![],
        };

        let (on, on_context, _scratch) = seed(&set, &config(true)).expect("seed with the graph on");
        let graph = on
            .graph
            .as_ref()
            .expect("the config has the graph on, so seed must have built one");
        let seeds = graph
            .seeds(&on_context, &[EntityKey::new("Kinesis")])
            .expect("seed lookup");
        assert!(
            !seeds.is_empty(),
            "a declared entity must be there to hop from"
        );
        assert!(
            !graph
                .neighbours(&on_context, &seeds, 1, None, 10)
                .expect("hop")
                .is_empty(),
            "the corpus's relation must have been recorded as an edge"
        );

        let (off, off_context, _scratch) =
            seed(&set, &config(false)).expect("seed with the graph off");
        assert!(
            off.graph.is_none(),
            "the harness must not wire a graph the config turned off"
        );
        assert_eq!(
            off.repository.list(&off_context, true).unwrap().len(),
            2,
            "turning the graph off must not cost the corpus its memories"
        );
    }

    /// A [`NewMemory`] for the ignored seeding test, matching
    /// `test_doubles::new_memory` without importing it into a `#[ignore]`d
    /// path that also needs the real embedder.
    fn test_new_memory(content: &str) -> NewMemory {
        NewMemory {
            content: content.to_string(),
            category: Category::parse("fact.project").unwrap(),
            subcategory: None,
            tags: Vec::new(),
            entities: Vec::new(),
            confidence: 1.0,
            source: crate::memories::domain::memory::MemorySource {
                client: Some("test".to_string()),
                session_id: None,
            },
            expires_at: None,
        }
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

        // The Phase 0 regression kinds exist (reranking-plan.md
        // Phase 0.3) and — since Phases 1–2 fixed the defects they were
        // written to document — *none* of them carries the `informational`
        // flag any more (Phase 2.5): every case in the file gates the
        // baseline now, and this assertion is what stops a new
        // "temporarily informational" flag from silently living forever.
        for kind in ["nl-question", "nl-category", "negative"] {
            let cases: Vec<&Case> = set.cases.iter().filter(|c| c.kind == kind).collect();
            assert!(
                !cases.is_empty(),
                "the {kind} kind from reranking-plan.md Phase 0.3 is missing"
            );
            assert!(
                cases.iter().all(|c| !c.informational),
                "{kind} cases are gated since Phases 1–2 fixed their defects; a reappearing \
                 informational flag needs the defect it hides to be recorded in §12 first"
            );
        }

        // Task 0.7 (recall-quality-plan.md) added four kinds that document
        // defects its own phases have not fixed — `chain` (Phase 3), `alias`
        // (Phase 6), `temporal` (Phase 7) and `vocab-gap` (Phase 5) — and they
        // are the *only* muted cases in the file. That keeps the promise
        // Phase 2.5 made in the shape it still means: nothing is informational
        // because someone found the red inconvenient. Each promotion therefore
        // means deleting the flag from its kind and re-recording the baseline,
        // not editing this list.
        const PHASE_ZERO_SEVEN: [(&str, usize); 4] = [
            ("chain", 4),
            ("temporal", 4),
            ("alias", 4),
            ("vocab-gap", 6),
        ];
        for (kind, minimum) in PHASE_ZERO_SEVEN {
            let cases: Vec<&Case> = set.cases.iter().filter(|c| c.kind == kind).collect();
            assert!(
                cases.len() >= minimum,
                "the {kind} kind from recall-quality-plan.md Task 0.7 needs {minimum} cases, \
                 found {}",
                cases.len()
            );
            assert!(
                cases.iter().all(|case| case.informational),
                "{kind} measures a defect its phase has not fixed yet; gating on it would redden \
                 CI for a known gap, and the phase would arrive with nothing to move"
            );
        }
        for case in set.cases.iter().filter(|case| case.informational) {
            assert!(
                PHASE_ZERO_SEVEN.iter().any(|(kind, _)| *kind == case.kind),
                "case {:?} (kind {:?}) is informational without a phase that owns its defect — \
                 record the defect in the plan before muting it",
                case.name,
                case.kind
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

    /// Which of the corpus's cases a graph is even allowed to start from.
    ///
    /// A hop begins at a name the *store* confirmed: [`seed_candidates`] offers
    /// every 1- to 3-word window of the query and `EntityGraph::seeds` keeps only
    /// the ones some memory declared — the G2 guard (recall-quality-plan.md §10),
    /// deliberately exact, because §17 has the abstention number for fuzzy seeding.
    /// The consequence the plan's Phase 3 gate did not anticipate: a case whose
    /// query paraphrases its entities away is unreachable by *any* graph mechanism,
    /// however the hop or the reserved slot is tuned.
    ///
    /// Pinned because Phase 3 was designed to move this kind and did not: all four
    /// `chain` cases Task 0.7 wrote are of that shape, which is why
    /// `[graph].expansion_slots` moves `relational` by 28.6 points of complete@5 and
    /// `chain` by exactly nothing (§17). The fifth chain case names `checkout flow`
    /// outright, which is what gives the kind one instance the mechanism can be
    /// reviewed on. If this test flips, it is the corpus's seedability that moved —
    /// re-measure the phase's gate either way, because the recorded number stopped
    /// meaning what it says.
    #[test]
    fn chain_cases_paraphrase_their_entities_so_no_hop_can_start() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("eval/cases.toml");
        let set = load_set(&path).expect("the committed set loads");

        let declared: BTreeSet<String> = set
            .memories
            .iter()
            .flat_map(|memory| memory.entities.iter())
            .map(|entity| EntityKey::new(&entity.name).as_str().to_string())
            .collect();
        assert!(
            !declared.is_empty(),
            "a corpus with no declared entities makes every graph case unreachable by \
             construction, which is not the same as passing"
        );

        let confirmed = |query: &str| {
            seed_candidates(query)
                .into_iter()
                .filter(|key| declared.contains(key.as_str()))
                .collect::<Vec<_>>()
        };

        let chain: Vec<&Case> = set.cases.iter().filter(|c| c.kind == "chain").collect();
        let seedable: Vec<&str> = chain
            .iter()
            .filter(|c| !confirmed(&c.query).is_empty())
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            seedable,
            vec!["chain: the checkout flow named outright"],
            "the chain kind's seedability changed. Unseedable cases cannot be reached by the \
             hop, so a slot change measured against them is measuring nothing: {seedable:?}",
        );

        // `relational` is the kind the slot's +28.6 was measured on, and the reason
        // it could be measured there at all: most of its cases name something.
        let relational: Vec<&Case> = set
            .cases
            .iter()
            .filter(|c| c.kind == "relational")
            .collect();
        assert_eq!(
            relational.len(),
            7,
            "the kind the gate reads has changed size"
        );
        let seeded = relational
            .iter()
            .filter(|c| !confirmed(&c.query).is_empty())
            .count();
        assert!(
            seeded >= 5,
            "only {seeded} of {} relational cases confirm a seed, and the slot's measured gain \
             is bounded by exactly that: what the hop is allowed to start from",
            relational.len(),
        );
    }
}
