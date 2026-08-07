//! One consolidation pass over every user's memories.
//!
//! # What it does
//!
//! For each user, within each category: re-embed the active memories,
//! link every pair that scores above `[consolidation].similarity_threshold`,
//! resolve those links into clusters, and hand each cluster to
//! [`MemoryMerger`].
//!
//! # Why within a category
//!
//! It is both the cheap answer and the right one. Cheap, because pairwise
//! comparison is quadratic and comparing categories separately turns one
//! big square into several small ones. Right, because two memories in
//! different categories are not duplicates by definition — a
//! `preference.coding` and a `fact.project` that read alike are a
//! preference and a fact, and merging them would produce something that
//! is neither.
//!
//! # Why re-embed rather than read the stored vectors
//!
//! `VectorIndex` answers "what is near this vector", which is the wrong
//! question: it returns ranks, and consolidation needs a comparable
//! number to check against a threshold. Re-embedding a category's
//! memories is one batch call to a local model in a nightly job, and it
//! keeps the whole comparison on one scale.
//!
//! # Two halves, only one of which needs a model
//!
//! Expiry and decay are arithmetic — a timestamp comparison and a score
//! over access bookkeeping — so they run in every installation,
//! including the zero-egress default. Merging needs a model to judge
//! whether two memories mean the same thing, and simply does not happen
//! without one. Tying the first to the second would mean the common
//! configuration silently keeps expired memories forever.
//!
//! # Why it never fails the whole run
//!
//! A run touches every user. One user's broken cluster, missing model
//! reply, or unreadable memory must not stop the other users' from being
//! consolidated — so failures are logged per user and the pass continues.
//! The alternative is that one bad memory freezes consolidation for
//! everybody, silently, for as long as it takes someone to notice.

use crate::consolidation::application::memory_maintainer::MemoryMaintainer;
use crate::consolidation::application::memory_merger::{MemoryMerger, MergeOutcome};
use crate::consolidation::domain::cluster_builder::ClusterBuilder;
use crate::consolidation::domain::consolidation_run::{ClusterPreview, ConsolidationReport};
use crate::consolidation::domain::consolidation_state::ConsolidationStateStore;
use crate::consolidation::domain::similarity::cosine;
use crate::identity::application::background_user_resolver::BackgroundUserResolver;
use crate::identity::domain::user_context::UserContext;
use crate::identity::domain::user_repository::UserRepository;
use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::shared::blocking::blocking;
use crate::shared::clock::Clock;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::MemoryId;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Most memories one category is compared within, per run.
///
/// Pairwise comparison is quadratic: 2000 memories is two million cosine
/// distances, a second or two of a nightly job's time. Ten thousand would
/// be fifty million and a noticeable stall on a machine that is also
/// serving requests.
///
/// Overflow is logged rather than silently dropped. A user with more than
/// this in one category has a corpus large enough that its duplicates are
/// a smaller problem than a consolidation pass that runs for minutes.
pub const MAX_MEMORIES_PER_CATEGORY: usize = 2_000;

#[derive(Clone)]
pub struct ConsolidationRunner {
    users: Arc<dyn UserRepository>,
    resolver: Arc<BackgroundUserResolver>,
    memories: Arc<dyn MemoryRepository>,
    embedder: Arc<dyn Embedder>,
    maintainer: Arc<MemoryMaintainer>,
    /// `None` without a configured provider. Expiry and decay still run.
    merger: Option<Arc<MemoryMerger>>,
    clock: Arc<dyn Clock>,
    threshold: f32,
    /// Maximum LLM merge calls per run. `None` means unlimited.
    max_llm_calls: Option<usize>,
    /// Maximum wall-clock seconds per run. `None` means unlimited.
    max_duration_secs: Option<u64>,
    /// Maximum memories retired per run. `None` means unlimited.
    max_memories: Option<usize>,
    /// Watermark store for the skip-unchanged heuristic. `None` disables
    /// skipping — every group is examined every run, the pre-7.1
    /// behaviour.
    state_store: Option<Arc<dyn ConsolidationStateStore>>,
}

/// A `(category, subcategory)` group that was examined this run, and the
/// maximum `updated_at` seen in it. Recorded as the skip watermark once
/// the group's clusters have been handled without error.
struct ProcessedGroup {
    category: String,
    subcategory: Option<String>,
    max_updated_at: DateTime<Utc>,
}

/// What [`ConsolidationRunner::plan`] produces: how many memories were
/// examined, the clusters worth a model call, how many groups were
/// skipped as unchanged, and the watermark to record for each group that
/// was examined.
type PlanOutcome = (usize, Vec<Vec<Memory>>, usize, Vec<ProcessedGroup>);

/// Tracks remaining budget during a consolidation run.
struct ConsolidationBudget {
    max_llm_calls: Option<usize>,
    max_duration_secs: Option<u64>,
    max_memories: Option<usize>,
    llm_calls_made: usize,
    memories_retired: usize,
    start: Instant,
    exhausted: bool,
    reason: Option<String>,
}

impl ConsolidationBudget {
    fn new(
        max_llm_calls: Option<usize>,
        max_duration_secs: Option<u64>,
        max_memories: Option<usize>,
    ) -> Self {
        Self {
            max_llm_calls,
            max_duration_secs,
            max_memories,
            llm_calls_made: 0,
            memories_retired: 0,
            start: Instant::now(),
            exhausted: false,
            reason: None,
        }
    }

    /// Returns true if the budget is exhausted.
    fn is_exhausted(&self) -> bool {
        self.exhausted
    }

    /// Check all budget limits. Mark exhausted if any limit is reached.
    fn check(&mut self) {
        if self.exhausted {
            return;
        }

        if let Some(max) = self.max_llm_calls {
            if self.llm_calls_made >= max {
                self.exhausted = true;
                self.reason = Some("max_llm_calls reached".to_string());
                return;
            }
        }

        if let Some(max_secs) = self.max_duration_secs {
            let elapsed = self.start.elapsed().as_secs();
            if elapsed >= max_secs {
                self.exhausted = true;
                self.reason = Some(format!("max_duration_secs reached ({elapsed}s)"));
                return;
            }
        }

        if let Some(max) = self.max_memories {
            if self.memories_retired >= max {
                self.exhausted = true;
                self.reason = Some("max_memories reached".to_string());
            }
        }
    }

    /// Record that an LLM merge call was made. Returns true if the budget
    /// is now exhausted and the caller should stop.
    fn record_llm_call(&mut self) -> bool {
        self.llm_calls_made += 1;
        self.check();
        self.exhausted
    }

    /// Record that memories were retired. Returns true if the budget is
    /// now exhausted and the caller should stop.
    fn record_retired(&mut self, count: usize) -> bool {
        self.memories_retired += count;
        self.check();
        self.exhausted
    }
}

impl ConsolidationRunner {
    /// Takes the user table once and derives the context resolver from
    /// it. Passing both would let a caller hand over two different user
    /// tables — one to walk, another to authenticate against — which is
    /// not a configuration that means anything.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        users: Arc<dyn UserRepository>,
        memories: Arc<dyn MemoryRepository>,
        embedder: Arc<dyn Embedder>,
        maintainer: Arc<MemoryMaintainer>,
        merger: Option<Arc<MemoryMerger>>,
        clock: Arc<dyn Clock>,
        threshold: f32,
        max_llm_calls: Option<usize>,
        max_duration_secs: Option<u64>,
        max_memories: Option<usize>,
        state_store: Option<Arc<dyn ConsolidationStateStore>>,
    ) -> Self {
        Self {
            resolver: Arc::new(BackgroundUserResolver::new(Arc::clone(&users))),
            users,
            memories,
            embedder,
            maintainer,
            merger,
            clock,
            threshold,
            max_llm_calls,
            max_duration_secs,
            max_memories,
            state_store,
        }
    }

    /// Runs one pass. `dry_run` stops after clustering: nothing is
    /// written, and no model is called either — a preview an operator can
    /// run without spending anything.
    pub async fn execute(&self, dry_run: bool) -> Result<ConsolidationReport> {
        let users = self.users.clone();
        let all = blocking(move || users.list()).await?;

        let mut report = ConsolidationReport {
            dry_run,
            users: all.len(),
            ..ConsolidationReport::default()
        };

        let mut budget = ConsolidationBudget::new(
            self.max_llm_calls,
            self.max_duration_secs,
            self.max_memories,
        );

        for user in all {
            if budget.is_exhausted() {
                tracing::info!(
                    "stopping consolidation: budget exhausted ({})",
                    budget.reason.as_deref().unwrap_or("limit reached")
                );
                report.budget_exhausted = true;
                report.budget_reason = budget.reason;
                break;
            }

            let context = match self.resolver.execute(user.id()) {
                Ok(context) => context,
                Err(error) => {
                    tracing::warn!(user = %user.handle(), %error, "skipping a user");
                    continue;
                }
            };

            match self.consolidate_user(&context, dry_run, &mut budget).await {
                Ok(theirs) => report.absorb(theirs),
                Err(error) => {
                    // Logged and skipped: one user's failure must not
                    // cost everyone else their consolidation.
                    tracing::warn!(
                        user = %user.handle(),
                        %error,
                        "consolidation failed for this user; continuing with the rest"
                    );
                }
            }
        }

        tracing::info!("{}", report.summary());
        Ok(report)
    }

    async fn consolidate_user(
        &self,
        context: &UserContext,
        dry_run: bool,
        budget: &mut ConsolidationBudget,
    ) -> Result<ConsolidationReport> {
        // Maintenance first, and before the snapshot: a memory that
        // expired last night should not be clustered, merged, and only
        // then retired.
        let maintenance = if dry_run {
            Default::default()
        } else {
            let this = self.clone();
            let (owned, now) = (context.clone(), self.clock.now());
            blocking(move || this.maintainer.execute(&owned, now)).await?
        };

        let this = self.clone();
        let owned = context.clone();
        let (examined, clusters, categories_skipped, processed) =
            blocking(move || this.plan(&owned)).await?;

        let mut report = ConsolidationReport {
            dry_run,
            memories_examined: examined,
            clusters_found: clusters.len(),
            categories_skipped,
            expired: maintenance.expired,
            rescored: maintenance.rescored,
            ..ConsolidationReport::default()
        };

        if dry_run {
            report.previews = clusters.iter().map(|cluster| preview(cluster)).collect();
            return Ok(report);
        }

        let Some(merger) = self.merger.as_ref() else {
            // No provider. Expiry and decay have already run, which is
            // everything this installation can do. No watermark is
            // recorded: nothing was consolidated, so a later run once a
            // provider is configured must still examine these groups.
            return Ok(report);
        };

        // Groups whose merge errored this run: their watermark must not be
        // recorded, or the unmerged duplicates would be skipped next time.
        let mut failed: HashSet<(String, Option<String>)> = HashSet::new();
        // Set when the run stops mid-clusters on a budget limit: the
        // groups after the break went unexamined, so nothing is recorded.
        let mut budget_broke = false;

        for cluster in clusters {
            if budget.is_exhausted() {
                tracing::info!(
                    "stopping user consolidation: budget exhausted ({})",
                    budget.reason.as_deref().unwrap_or("limit reached")
                );
                report.budget_exhausted = true;
                report.budget_reason = budget.reason.clone();
                budget_broke = true;
                break;
            }

            let key = group_key(&cluster);

            match merger.execute(context, &cluster).await {
                Ok(MergeOutcome::Merged { retired, .. }) => {
                    report.merged += 1;
                    report.retired += retired;
                    // A merge costs both an LLM call and retires memories.
                    if budget.record_llm_call() {
                        report.budget_exhausted = true;
                        report.budget_reason = budget.reason.clone();
                        budget_broke = true;
                        break;
                    }
                    if budget.record_retired(retired) {
                        report.budget_exhausted = true;
                        report.budget_reason = budget.reason.clone();
                        budget_broke = true;
                        break;
                    }
                }
                Ok(MergeOutcome::KeptSeparate { .. }) => {
                    report.kept_separate += 1;
                    if budget.record_llm_call() {
                        report.budget_exhausted = true;
                        report.budget_reason = budget.reason.clone();
                        budget_broke = true;
                        break;
                    }
                }
                Err(error) => {
                    // Same reasoning as the per-user skip, one level down:
                    // a provider hiccup on one cluster should not cost the
                    // user their other clusters.
                    tracing::warn!(%error, "a cluster could not be merged; leaving it alone");
                    failed.insert(key);
                }
            }
        }

        // Record the skip watermark for every group examined this run
        // whose clusters were all handled without error. Skipped entirely
        // when the budget cut the run short, so no half-consolidated group
        // is ever marked done. The recorded watermark is the group's
        // pre-merge maximum; a group that merged will read one changed
        // (post-merge) memory on the next run and be re-examined once more
        // — a cheap pass that finds nothing to merge — before settling
        // into being skipped.
        if let Some(store) = self.state_store.clone()
            && !budget_broke
        {
            let to_record: Vec<ProcessedGroup> = processed
                .into_iter()
                .filter(|group| {
                    !failed.contains(&(group.category.clone(), group.subcategory.clone()))
                })
                .collect();

            if !to_record.is_empty() {
                let owned = context.clone();
                blocking(move || {
                    for group in &to_record {
                        store.record(
                            &owned,
                            &group.category,
                            group.subcategory.as_deref(),
                            group.max_updated_at,
                        )?;
                    }
                    Ok::<(), RaError>(())
                })
                .await?;
            }
        }

        Ok(report)
    }

    /// The synchronous half: snapshot, embed, cluster.
    ///
    /// Returns how many memories were examined, the clusters worth a
    /// model call, how many groups were skipped because nothing changed
    /// since the last run, and — for the groups it did examine — the
    /// watermark to record once their merges succeed. Groups are
    /// processed largest-first so the budget is spent where duplicates
    /// are densest.
    fn plan(&self, context: &UserContext) -> Result<PlanOutcome> {
        let now = self.clock.now();
        let active: Vec<Memory> = self
            .memories
            .list(context, false)?
            .into_iter()
            .filter(|memory| memory.is_active_at(now))
            .collect();

        let examined = active.len();
        let mut clusters = Vec::new();
        let mut categories_skipped = 0usize;
        let mut processed: Vec<ProcessedGroup> = Vec::new();

        let mut groups = by_category_and_subcategory(active);
        // Largest-group-first: spend the budget where duplicates are
        // densest.
        groups.sort_by_key(|b| std::cmp::Reverse(b.1.len()));

        for ((category, subcategory), group) in groups {
            if group.len() < 2 {
                continue;
            }

            // The group's watermark: the newest change to any of its
            // active memories. `max()` is only `None` for an empty group,
            // already excluded above.
            let Some(current_max) = group.iter().map(|memory| memory.updated_at()).max() else {
                continue;
            };

            // Skip-unchanged heuristic: if nothing in this group has
            // changed since it was last consolidated, skip it entirely.
            if self.should_skip(context, &category, subcategory.as_deref(), current_max)? {
                categories_skipped += 1;
                continue;
            }

            processed.push(ProcessedGroup {
                category: category.clone(),
                subcategory: subcategory.clone(),
                max_updated_at: current_max,
            });

            let group = match group.len() > MAX_MEMORIES_PER_CATEGORY {
                false => group,
                true => {
                    tracing::warn!(
                        category,
                        total = group.len(),
                        limit = MAX_MEMORIES_PER_CATEGORY,
                        "category is too large to compare in full; consolidating the \
                         most recent slice of it this run"
                    );
                    let mut group = group;
                    group.sort_by_key(|b| std::cmp::Reverse(b.created_at()));
                    group.truncate(MAX_MEMORIES_PER_CATEGORY);
                    group
                }
            };

            clusters.extend(self.cluster(group)?);
        }

        Ok((examined, clusters, categories_skipped, processed))
    }

    /// Returns true if this `(category, subcategory)` group should be
    /// skipped because nothing in it changed since it was last
    /// consolidated. Without a state store the answer is always false —
    /// the pre-7.1 behaviour of examining every group every run.
    fn should_skip(
        &self,
        context: &UserContext,
        category: &str,
        subcategory: Option<&str>,
        current_max: DateTime<Utc>,
    ) -> Result<bool> {
        let Some(store) = &self.state_store else {
            return Ok(false);
        };
        let stored = store.last_max_updated_at(context, category, subcategory)?;
        Ok(stored == Some(current_max))
    }

    /// Embeds one category's memories and groups the near-duplicates.
    fn cluster(&self, group: Vec<Memory>) -> Result<Vec<Vec<Memory>>> {
        let texts: Vec<String> = group
            .iter()
            .map(|memory| memory.content().to_string())
            .collect();
        let vectors = self.embedder.embed(&texts, EmbeddingTask::Document)?;

        let mut builder = ClusterBuilder::new();
        for (left, left_vector) in vectors.iter().enumerate() {
            for (offset, right_vector) in vectors[left + 1..].iter().enumerate() {
                if cosine(left_vector, right_vector) >= self.threshold {
                    builder.link(group[left].id(), group[left + 1 + offset].id());
                }
            }
        }

        let by_id: HashMap<MemoryId, &Memory> =
            group.iter().map(|memory| (memory.id(), memory)).collect();

        Ok(builder
            .clusters()
            .into_iter()
            .map(|ids| {
                ids.into_iter()
                    .filter_map(|id| by_id.get(&id).map(|memory| (*memory).clone()))
                    .collect()
            })
            .collect())
    }
}

type CategoryGroup = Vec<((String, Option<String>), Vec<Memory>)>;

/// The `(category, subcategory)` a cluster belongs to, taken from its
/// first memory — every memory in a cluster shares both, since clustering
/// happens within a group. Matches the watermark key in `plan`.
fn group_key(cluster: &[Memory]) -> (String, Option<String>) {
    match cluster.first() {
        Some(memory) => (
            memory.category().as_str().to_string(),
            memory.subcategory().map(|sub| sub.to_string()),
        ),
        None => (String::new(), None),
    }
}

fn by_category_and_subcategory(memories: Vec<Memory>) -> CategoryGroup {
    let mut groups: CategoryGroup = Vec::new();

    for memory in memories {
        let key = (
            memory.category().as_str().to_string(),
            memory.subcategory().map(|s| s.to_string()),
        );
        match groups
            .iter_mut()
            .find(|((cat, sub), _)| *cat == key.0 && *sub == key.1)
        {
            Some((_, group)) => group.push(memory),
            None => groups.push((key, vec![memory])),
        }
    }

    groups
}

fn preview(cluster: &[Memory]) -> ClusterPreview {
    ClusterPreview {
        category: cluster
            .first()
            .map(|memory| memory.category().as_str().to_string())
            .unwrap_or_default(),
        contents: cluster
            .iter()
            .map(|memory| memory.content().replace('\n', " "))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consolidation::application::memory_merger::ACTOR;
    use crate::memories::application::test_doubles::{Fixture, fixed_clock, new_memory, now};
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory_repository::AuditOperation;
    use crate::memories::domain::recall_query::RecallQuery;
    use crate::understanding::application::scripted_chat_model::ScriptedChatModel;
    use crate::understanding::domain::chat_model::ChatModel;
    use crate::understanding::domain::taxonomy::Taxonomy;
    use serde_json::json;

    /// The fake embedder is deterministic per content, so identical text
    /// embeds identically — which is how a test creates a cluster without
    /// a real model. A threshold of 0.999 then means "only near-identical
    /// text clusters", keeping these tests about the runner rather than
    /// about embedding quality.
    const STRICT: f32 = 0.999;

    /// In-memory watermark store, deterministic and offline like the rest
    /// of the fixture. Keys on the user so cross-tenant behaviour is
    /// exercised the same way the SQLite store is.
    #[derive(Default)]
    struct InMemoryStateStore {
        watermarks: std::sync::Mutex<HashMap<(String, String, String), DateTime<Utc>>>,
    }

    impl InMemoryStateStore {
        fn key(
            context: &UserContext,
            category: &str,
            subcategory: Option<&str>,
        ) -> (String, String, String) {
            (
                context.user_id().to_string(),
                category.to_string(),
                subcategory.unwrap_or("").to_string(),
            )
        }
    }

    impl ConsolidationStateStore for InMemoryStateStore {
        fn last_max_updated_at(
            &self,
            context: &UserContext,
            category: &str,
            subcategory: Option<&str>,
        ) -> Result<Option<DateTime<Utc>>> {
            Ok(self
                .watermarks
                .lock()
                .unwrap()
                .get(&Self::key(context, category, subcategory))
                .copied())
        }

        fn record(
            &self,
            context: &UserContext,
            category: &str,
            subcategory: Option<&str>,
            max_updated_at: DateTime<Utc>,
        ) -> Result<()> {
            self.watermarks
                .lock()
                .unwrap()
                .insert(Self::key(context, category, subcategory), max_updated_at);
            Ok(())
        }
    }

    fn build_runner(
        fixture: &Fixture,
        model: ScriptedChatModel,
        threshold: f32,
    ) -> (ConsolidationRunner, Arc<ScriptedChatModel>) {
        let model = Arc::new(model);
        let merger = Arc::new(MemoryMerger::new(
            Arc::new(fixture.saver()),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&model) as Arc<dyn ChatModel>,
            Arc::new(Taxonomy::new(vec![])),
        ));

        (
            ConsolidationRunner::new(
                Arc::clone(&fixture.users) as Arc<dyn UserRepository>,
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                Arc::clone(&fixture.embedder) as Arc<dyn Embedder>,
                Arc::new(MemoryMaintainer::new(
                    Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                    Arc::new(fixture.forgetter()),
                )),
                Some(merger),
                fixed_clock(),
                threshold,
                None,
                None,
                None,
                None,
            ),
            model,
        )
    }

    /// A runner wired with a state store, for the skip-unchanged tests.
    fn build_runner_with_state(
        fixture: &Fixture,
        model: ScriptedChatModel,
        threshold: f32,
        state: Arc<dyn ConsolidationStateStore>,
    ) -> (ConsolidationRunner, Arc<ScriptedChatModel>) {
        let model = Arc::new(model);
        let merger = Arc::new(MemoryMerger::new(
            Arc::new(fixture.saver()),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&model) as Arc<dyn ChatModel>,
            Arc::new(Taxonomy::new(vec![])),
        ));

        (
            ConsolidationRunner::new(
                Arc::clone(&fixture.users) as Arc<dyn UserRepository>,
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                Arc::clone(&fixture.embedder) as Arc<dyn Embedder>,
                Arc::new(MemoryMaintainer::new(
                    Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                    Arc::new(fixture.forgetter()),
                )),
                Some(merger),
                fixed_clock(),
                threshold,
                None,
                None,
                None,
                Some(state),
            ),
            model,
        )
    }

    fn merged_reply() -> serde_json::Value {
        json!({
            "merge": true,
            "content": "User uses pnpm; never npm or yarn",
            "category": "preference.coding",
            "tags": ["tooling"],
            "reason": "several phrasings of one preference",
        })
    }

    fn recall(fixture: &Fixture, context: &UserContext, query: &str) -> Vec<String> {
        fixture
            .recaller()
            .execute(context, &RecallQuery::new(query, 50).unwrap())
            .unwrap()
            .into_iter()
            .map(|scored| scored.memory.content().to_string())
            .collect()
    }

    #[tokio::test]
    async fn duplicates_become_one_memory_and_the_originals_are_retired() {
        // The DoD scenario, through the whole job rather than one merger
        // call: seed duplicates, run, and check what recall returns.
        let fixture = Fixture::new();
        for _ in 0..5 {
            fixture.save(&fixture.alex, "User prefers pnpm");
        }

        let (runner, _) = build_runner(
            &fixture,
            ScriptedChatModel::new().queue(merged_reply()),
            STRICT,
        );
        let report = runner.execute(false).await.unwrap();

        assert_eq!(report.clusters_found, 1);
        assert_eq!(report.merged, 1);
        assert_eq!(report.retired, 5);
        assert_eq!(
            recall(&fixture, &fixture.alex, "package manager"),
            ["User uses pnpm; never npm or yarn"],
            "recall should return exactly one memory"
        );
    }

    #[tokio::test]
    async fn a_dry_run_mutates_nothing_and_calls_no_model() {
        let fixture = Fixture::new();
        for _ in 0..5 {
            fixture.save(&fixture.alex, "User prefers pnpm");
        }
        let before = recall(&fixture, &fixture.alex, "pnpm");

        let (runner, model) = build_runner(&fixture, ScriptedChatModel::new(), STRICT);
        let report = runner.execute(true).await.unwrap();

        assert!(report.dry_run);
        assert_eq!(report.clusters_found, 1);
        assert_eq!(report.merged, 0);
        assert_eq!(report.retired, 0);
        assert_eq!(
            model.call_count(),
            0,
            "a dry run must not spend anything on a model"
        );
        assert_eq!(recall(&fixture, &fixture.alex, "pnpm"), before);
        assert!(
            fixture
                .memories
                .audit_trail(&fixture.alex, 100)
                .unwrap()
                .iter()
                .all(|entry| entry.actor != ACTOR),
            "a dry run wrote to the audit trail"
        );
    }

    #[tokio::test]
    async fn a_dry_run_shows_what_it_would_merge() {
        // Counts alone would leave an operator no way to judge whether to
        // let the real run proceed.
        let fixture = Fixture::new();
        for _ in 0..3 {
            fixture.save(&fixture.alex, "User prefers pnpm");
        }

        let (runner, _) = build_runner(&fixture, ScriptedChatModel::new(), STRICT);
        let report = runner.execute(true).await.unwrap();

        assert_eq!(report.previews.len(), 1);
        assert_eq!(report.previews[0].category, "preference.coding");
        assert_eq!(report.previews[0].contents.len(), 3);
        assert!(report.previews[0].contents[0].contains("pnpm"));
    }

    #[tokio::test]
    async fn a_store_with_no_duplicates_is_left_alone() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm");
        fixture.save(&fixture.alex, "The backend runs on Hetzner");

        let (runner, model) = build_runner(&fixture, ScriptedChatModel::new(), STRICT);
        let report = runner.execute(false).await.unwrap();

        assert_eq!(report.clusters_found, 0);
        assert_eq!(model.call_count(), 0, "nothing to ask about");
        assert_eq!(recall(&fixture, &fixture.alex, "prefers").len(), 2);
    }

    #[tokio::test]
    async fn memories_in_different_categories_are_never_clustered_together() {
        // Two memories in different categories are not duplicates by
        // definition, however alike they read.
        let fixture = Fixture::new();
        for category in [Category::PreferenceCoding, Category::FactProject] {
            let mut new = crate::memories::application::test_doubles::new_memory(
                "identical text in two categories",
            );
            new.category = category;
            fixture.saver().execute(&fixture.alex, new, "test").unwrap();
        }

        let (runner, model) = build_runner(&fixture, ScriptedChatModel::new(), STRICT);
        let report = runner.execute(false).await.unwrap();

        assert_eq!(report.clusters_found, 0);
        assert_eq!(model.call_count(), 0);
    }

    #[tokio::test]
    async fn one_users_duplicates_are_never_clustered_with_anothers() {
        // The isolation guarantee, at the one place in the system that
        // deliberately walks every user.
        let fixture = Fixture::new();
        for _ in 0..2 {
            fixture.save(&fixture.alex, "User prefers pnpm");
            fixture.save(&fixture.sam, "User prefers pnpm");
        }

        let (runner, _) = build_runner(
            &fixture,
            ScriptedChatModel::new()
                .queue(merged_reply())
                .queue(merged_reply()),
            STRICT,
        );
        let report = runner.execute(false).await.unwrap();

        // Two clusters — one per user — not one cluster of four.
        assert_eq!(report.clusters_found, 2);
        assert_eq!(report.retired, 4);

        for context in [&fixture.alex, &fixture.sam] {
            assert_eq!(
                recall(&fixture, context, "pnpm").len(),
                1,
                "each user should end with their own single merged memory"
            );
        }
    }

    #[tokio::test]
    async fn superseded_and_deleted_memories_are_not_reconsolidated() {
        // Otherwise every run would re-cluster what the last one retired.
        let fixture = Fixture::new();
        for _ in 0..3 {
            fixture.save(&fixture.alex, "User prefers pnpm");
        }

        let (runner, _) = build_runner(
            &fixture,
            ScriptedChatModel::new().queue(merged_reply()),
            STRICT,
        );
        runner.execute(false).await.unwrap();

        // A second pass with no queued replies: if it tried to merge
        // anything, the scripted model would error.
        let (again, model) = build_runner(&fixture, ScriptedChatModel::new(), STRICT);
        let report = again.execute(false).await.unwrap();

        assert_eq!(report.clusters_found, 0);
        assert_eq!(model.call_count(), 0);
    }

    #[tokio::test]
    async fn a_failing_cluster_does_not_cost_the_user_their_other_clusters() {
        let fixture = Fixture::new();
        for _ in 0..2 {
            fixture.save(&fixture.alex, "User prefers pnpm");
        }
        for _ in 0..2 {
            fixture.save(&fixture.alex, "The backend runs on Hetzner");
        }

        let (runner, _) = build_runner(
            &fixture,
            ScriptedChatModel::new()
                .queue_error(
                    crate::understanding::domain::chat_model::ChatError::Transient(
                        "429".to_string(),
                    ),
                )
                .queue(merged_reply()),
            STRICT,
        );
        let report = runner.execute(false).await.unwrap();

        assert_eq!(report.clusters_found, 2);
        assert_eq!(report.merged, 1, "the second cluster should still merge");
    }

    #[tokio::test]
    async fn a_declined_cluster_is_reported_as_kept_rather_than_merged() {
        let fixture = Fixture::new();
        for _ in 0..2 {
            fixture.save(&fixture.alex, "User prefers pnpm");
        }

        let (runner, _) = build_runner(
            &fixture,
            ScriptedChatModel::new().queue(json!({"merge": false, "reason": "actually different"})),
            STRICT,
        );
        let report = runner.execute(false).await.unwrap();

        assert_eq!(report.kept_separate, 1);
        assert_eq!(report.merged, 0);
        assert_eq!(recall(&fixture, &fixture.alex, "pnpm").len(), 2);
    }

    #[tokio::test]
    async fn everything_consolidation_writes_is_attributed_to_it() {
        let fixture = Fixture::new();
        for _ in 0..2 {
            fixture.save(&fixture.alex, "User prefers pnpm");
        }

        let (runner, _) = build_runner(
            &fixture,
            ScriptedChatModel::new().queue(merged_reply()),
            STRICT,
        );
        runner.execute(false).await.unwrap();

        let audit = fixture.memories.audit_trail(&fixture.alex, 100).unwrap();
        let merges: Vec<_> = audit
            .iter()
            .filter(|entry| entry.operation == AuditOperation::Merge)
            .collect();

        assert_eq!(merges.len(), 2);
        assert!(merges.iter().all(|entry| entry.actor == ACTOR));
    }

    #[tokio::test]
    async fn memories_with_different_subcategories_are_never_clustered_together() {
        // Same category, different sub-label: the runner should create
        // separate groups so consolidation never merges across sub-labels.
        let fixture = Fixture::new();

        // Two "tooling" memories that are near-duplicates
        for _ in 0..2 {
            let mut new = new_memory("User prefers pnpm for package management");
            new.subcategory = Some("tooling".to_string());
            fixture.saver().execute(&fixture.alex, new, "test").unwrap();
        }

        // Two "testing" memories that are near-duplicates
        for _ in 0..2 {
            let mut new = new_memory("User prefers Vitest for unit tests");
            new.subcategory = Some("testing".to_string());
            fixture.saver().execute(&fixture.alex, new, "test").unwrap();
        }

        let (runner, _model) = build_runner(
            &fixture,
            ScriptedChatModel::new()
                .queue(json!({
                    "merge": true,
                    "content": "User prefers pnpm",
                    "category": "preference.coding",
                    "subcategory": "tooling",
                    "tags": [],
                    "reason": "duplicates"
                }))
                .queue(json!({
                    "merge": true,
                    "content": "User prefers Vitest",
                    "category": "preference.coding",
                    "subcategory": "testing",
                    "tags": [],
                    "reason": "duplicates"
                })),
            STRICT,
        );
        let report = runner.execute(false).await.unwrap();

        // Two clusters — one per subcategory — not one cluster of four.
        assert_eq!(
            report.clusters_found, 2,
            "each subcategory forms its own cluster independently"
        );
        assert_eq!(report.merged, 2);
        assert_eq!(report.retired, 4);

        // Each subcategory has exactly one surviving memory.
        let all = recall(&fixture, &fixture.alex, "user prefers");
        assert_eq!(all.len(), 2, "two merged memories, one per subcategory");
        assert!(
            all.iter().any(|c| c.contains("pnpm")),
            "tooling subcategory has its own merged memory"
        );
        assert!(
            all.iter().any(|c| c.contains("Vitest")),
            "testing subcategory has its own merged memory"
        );
    }

    #[tokio::test]
    async fn budget_exhaustion_stops_the_run_and_reports_why() {
        // When the budget is exhausted mid-run, the report says so and no
        // half-applied merge is left behind. The budget is per-run, not
        // per-user, so the second user never starts.
        let fixture = Fixture::new();
        for _ in 0..2 {
            fixture.save(&fixture.alex, "User prefers pnpm");
        }
        for _ in 0..2 {
            fixture.save(&fixture.sam, "Sam prefers cargo");
        }

        let (_runner, model) = build_runner(
            &fixture,
            ScriptedChatModel::new().queue(merged_reply()),
            STRICT,
        );

        // Manually create a runner with a tight budget: 1 LLM call max.
        let tight_runner = ConsolidationRunner::new(
            Arc::clone(&fixture.users) as Arc<dyn UserRepository>,
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&fixture.embedder) as Arc<dyn Embedder>,
            Arc::new(MemoryMaintainer::new(
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                Arc::new(fixture.forgetter()),
            )),
            Some(Arc::new(MemoryMerger::new(
                Arc::new(fixture.saver()),
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                Arc::clone(&model) as Arc<dyn ChatModel>,
                Arc::new(Taxonomy::new(vec![])),
            ))),
            fixed_clock(),
            STRICT,
            Some(1),
            None,
            None,
            None,
        );

        let report = tight_runner.execute(false).await.unwrap();

        assert!(report.budget_exhausted, "run should have stopped early");
        assert!(
            report.budget_reason.as_deref() == Some("max_llm_calls reached"),
            "reason should name the limit that was hit"
        );
        // Only one cluster was merged; the second user was never started.
        assert_eq!(report.merged, 1);
    }

    #[tokio::test]
    async fn fresh_duplicates_still_merge_with_a_state_store() {
        // The load-bearing regression guard: the skip path must never cost
        // the dedup guarantee. On the first run there is no watermark, so
        // nothing is skipped and the duplicates merge exactly as before.
        let fixture = Fixture::new();
        for _ in 0..5 {
            fixture.save(&fixture.alex, "User prefers pnpm");
        }

        let state: Arc<dyn ConsolidationStateStore> = Arc::new(InMemoryStateStore::default());
        let (runner, _) = build_runner_with_state(
            &fixture,
            ScriptedChatModel::new().queue(merged_reply()),
            STRICT,
            state,
        );
        let report = runner.execute(false).await.unwrap();

        assert_eq!(
            report.categories_skipped, 0,
            "nothing to skip on a first run"
        );
        assert_eq!(report.merged, 1);
        assert_eq!(report.retired, 5);
        assert_eq!(
            recall(&fixture, &fixture.alex, "package manager"),
            ["User uses pnpm; never npm or yarn"],
        );
    }

    #[tokio::test]
    async fn an_unchanged_group_is_skipped_on_the_next_run() {
        // Two distinct memories in one group: examined on the first run
        // (they don't cluster), skipped on the second because nothing
        // changed. The skip means the second run never embeds or asks the
        // model about them.
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm");
        fixture.save(&fixture.alex, "User forbids barrel files");

        let state: Arc<dyn ConsolidationStateStore> = Arc::new(InMemoryStateStore::default());

        let (first, first_model) = build_runner_with_state(
            &fixture,
            ScriptedChatModel::new(),
            STRICT,
            Arc::clone(&state),
        );
        let first_report = first.execute(false).await.unwrap();
        assert_eq!(
            first_report.categories_skipped, 0,
            "the first run has no watermark to skip against"
        );
        assert_eq!(
            first_model.call_count(),
            0,
            "distinct memories don't cluster"
        );

        let (second, _) = build_runner_with_state(
            &fixture,
            ScriptedChatModel::new(),
            STRICT,
            Arc::clone(&state),
        );
        let second_report = second.execute(false).await.unwrap();

        assert_eq!(
            second_report.categories_skipped, 1,
            "the unchanged group should be skipped the second time"
        );
        assert_eq!(
            second_report.clusters_found, 0,
            "a skipped group produces no clusters to consider"
        );
    }

    #[tokio::test]
    async fn touching_a_group_re_enables_it() {
        // A group skipped last run must be examined again once a memory in
        // it changes. A new memory carries a later `updated_at`, lifting
        // the group's maximum above the recorded watermark.
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm");
        fixture.save(&fixture.alex, "User forbids barrel files");

        let state: Arc<dyn ConsolidationStateStore> = Arc::new(InMemoryStateStore::default());

        // Run once to record the watermark, and again to confirm the skip.
        for _ in 0..2 {
            let (runner, _) = build_runner_with_state(
                &fixture,
                ScriptedChatModel::new(),
                STRICT,
                Arc::clone(&state),
            );
            runner.execute(false).await.unwrap();
        }

        // A newer memory in the same group — inserted with a later
        // timestamp so the group's max `updated_at` moves past the mark.
        let later = now() + chrono::Duration::hours(1);
        let fresh = Memory::create(
            fixture.alex.user_id(),
            new_memory("User writes doc comments on every public fn"),
            later,
        )
        .unwrap();
        fixture
            .memories
            .insert(&fixture.alex, &fresh, "test")
            .unwrap();

        let (runner, _) = build_runner_with_state(
            &fixture,
            ScriptedChatModel::new(),
            STRICT,
            Arc::clone(&state),
        );
        let report = runner.execute(false).await.unwrap();

        assert_eq!(
            report.categories_skipped, 0,
            "a changed group must be re-examined, not skipped"
        );
    }

    #[tokio::test]
    async fn a_failed_merge_is_retried_next_run_not_skipped() {
        // The correctness guard for the watermark: a group whose merge
        // errored must NOT be recorded as consolidated, or its duplicates
        // would be skipped forever. The next run, with a working model,
        // must still merge them.
        let fixture = Fixture::new();
        for _ in 0..2 {
            fixture.save(&fixture.alex, "User prefers pnpm");
        }

        let state: Arc<dyn ConsolidationStateStore> = Arc::new(InMemoryStateStore::default());

        // First run: the model errors on the only cluster.
        let (failing, _) = build_runner_with_state(
            &fixture,
            ScriptedChatModel::new().queue_error(
                crate::understanding::domain::chat_model::ChatError::Transient("429".to_string()),
            ),
            STRICT,
            Arc::clone(&state),
        );
        let failed_report = failing.execute(false).await.unwrap();
        assert_eq!(failed_report.merged, 0, "the merge failed this run");

        // Second run: nothing changed, but because the first run's merge
        // failed the group was never watermarked, so it is examined again
        // and merges cleanly.
        let (retry, _) = build_runner_with_state(
            &fixture,
            ScriptedChatModel::new().queue(merged_reply()),
            STRICT,
            Arc::clone(&state),
        );
        let retry_report = retry.execute(false).await.unwrap();

        assert_eq!(
            retry_report.categories_skipped, 0,
            "a group whose merge failed must not be skipped"
        );
        assert_eq!(
            retry_report.merged, 1,
            "the retry should merge the duplicates"
        );
        assert_eq!(recall(&fixture, &fixture.alex, "package manager").len(), 1);
    }
}
