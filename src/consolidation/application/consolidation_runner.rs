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
use crate::consolidation::domain::similarity::cosine;
use crate::identity::application::background_user_resolver::BackgroundUserResolver;
use crate::identity::domain::user_context::UserContext;
use crate::identity::domain::user_repository::UserRepository;
use crate::memories::domain::embedder::Embedder;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::shared::blocking::blocking;
use crate::shared::clock::Clock;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use std::collections::HashMap;
use std::sync::Arc;

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
}

impl ConsolidationRunner {
    /// Takes the user table once and derives the context resolver from
    /// it. Passing both would let a caller hand over two different user
    /// tables — one to walk, another to authenticate against — which is
    /// not a configuration that means anything.
    pub fn new(
        users: Arc<dyn UserRepository>,
        memories: Arc<dyn MemoryRepository>,
        embedder: Arc<dyn Embedder>,
        maintainer: Arc<MemoryMaintainer>,
        merger: Option<Arc<MemoryMerger>>,
        clock: Arc<dyn Clock>,
        threshold: f32,
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

        for user in all {
            let context = match self.resolver.execute(user.id()) {
                Ok(context) => context,
                Err(error) => {
                    tracing::warn!(user = %user.handle(), %error, "skipping a user");
                    continue;
                }
            };

            match self.consolidate_user(&context, dry_run).await {
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
        let (examined, clusters) = blocking(move || this.plan(&owned)).await?;

        let mut report = ConsolidationReport {
            dry_run,
            memories_examined: examined,
            clusters_found: clusters.len(),
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
            // everything this installation can do.
            return Ok(report);
        };

        for cluster in clusters {
            match merger.execute(context, &cluster).await {
                Ok(MergeOutcome::Merged { retired, .. }) => {
                    report.merged += 1;
                    report.retired += retired;
                }
                Ok(MergeOutcome::KeptSeparate { .. }) => report.kept_separate += 1,
                Err(error) => {
                    // Same reasoning as the per-user skip, one level down:
                    // a provider hiccup on one cluster should not cost the
                    // user their other clusters.
                    tracing::warn!(%error, "a cluster could not be merged; leaving it alone");
                }
            }
        }

        Ok(report)
    }

    /// The synchronous half: snapshot, embed, cluster.
    ///
    /// Returns how many memories were examined and the clusters worth a
    /// model call. Kept in one blocking section because it is all
    /// database and ONNX work, and splitting it would only multiply the
    /// hops on and off the runtime.
    fn plan(&self, context: &UserContext) -> Result<(usize, Vec<Vec<Memory>>)> {
        let now = self.clock.now();
        let active: Vec<Memory> = self
            .memories
            .list(context, false)?
            .into_iter()
            .filter(|memory| memory.is_active_at(now))
            .collect();

        let examined = active.len();
        let mut clusters = Vec::new();

        for (category, group) in by_category(active) {
            if group.len() < 2 {
                continue;
            }

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

        Ok((examined, clusters))
    }

    /// Embeds one category's memories and groups the near-duplicates.
    fn cluster(&self, group: Vec<Memory>) -> Result<Vec<Vec<Memory>>> {
        let texts: Vec<String> = group
            .iter()
            .map(|memory| memory.content().to_string())
            .collect();
        let vectors = self.embedder.embed(&texts)?;

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

fn by_category(memories: Vec<Memory>) -> Vec<(String, Vec<Memory>)> {
    let mut groups: Vec<(String, Vec<Memory>)> = Vec::new();

    for memory in memories {
        let category = memory.category().as_str().to_string();
        match groups.iter_mut().find(|(name, _)| *name == category) {
            Some((_, group)) => group.push(memory),
            None => groups.push((category, vec![memory])),
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
    use crate::memories::application::test_doubles::{Fixture, fixed_clock};
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
}
