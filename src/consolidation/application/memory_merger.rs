//! Replaces a cluster of near-duplicate memories with the one memory
//! that says what all of them said.
//!
//! # Why a model decides and not the threshold
//!
//! The similarity score got these memories into the same cluster; it
//! cannot tell whether they mean the same thing. "Prefers pnpm" and
//! "Prefers Vitest" are two tooling preferences a sentence embedding
//! finds very close together, and merging them would destroy a fact the
//! user cannot get back. So the threshold proposes and the model
//! disposes — and the prompt is written to argue against merging.
//!
//! # Why the merged memory is written, not chosen
//!
//! Keeping the "best" member would silently drop whatever detail the
//! others carried. The replacement is composed to preserve all of it,
//! which is what makes merging lossless compression rather than
//! summarisation.
//!
//! # What survives
//!
//! Nothing is deleted. The cluster's members are superseded — retained,
//! excluded from ordinary recall, reachable with `include_superseded` —
//! and every one of them gets a `merge` audit entry carrying the model's
//! reasoning. "Why did five of my memories become one overnight?" has to
//! stay answerable.

use crate::consolidation::domain::merge_prompt::{
    MergeDecision, check_mergeable, merge_request, parse_merge,
};
use crate::identity::domain::user_context::UserContext;
use crate::memories::application::direct_memory_saver::DirectMemorySaver;
use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use crate::understanding::domain::chat_model::ChatModel;
use crate::understanding::domain::taxonomy::Taxonomy;
use std::sync::Arc;

/// Recorded as the actor on everything consolidation writes, so the audit
/// trail separates the nightly job from anything a user or agent did.
pub const ACTOR: &str = "consolidation";

/// What merging one cluster did.
#[derive(Debug, Clone, PartialEq)]
pub enum MergeOutcome {
    Merged {
        /// The memory that now says what the cluster said.
        replacement: MemoryId,
        /// How many members were actually retired. Can be fewer than the
        /// cluster's size: a member the user deleted between the snapshot
        /// and the apply is skipped.
        retired: usize,
    },
    /// The cluster was left exactly as it was.
    KeptSeparate { reason: String },
}

pub struct MemoryMerger {
    saver: Arc<DirectMemorySaver>,
    memories: Arc<dyn MemoryRepository>,
    model: Arc<dyn ChatModel>,
    taxonomy: Arc<Taxonomy>,
}

impl MemoryMerger {
    pub fn new(
        saver: Arc<DirectMemorySaver>,
        memories: Arc<dyn MemoryRepository>,
        model: Arc<dyn ChatModel>,
        taxonomy: Arc<Taxonomy>,
    ) -> Self {
        Self {
            saver,
            memories,
            model,
            taxonomy,
        }
    }

    pub async fn execute(&self, context: &UserContext, cluster: &[Memory]) -> Result<MergeOutcome> {
        check_mergeable(cluster)?;

        let answer = self
            .model
            .complete_structured(&merge_request(&self.taxonomy, cluster))
            .await?;

        let decision = parse_merge(&answer, &self.taxonomy, cluster);
        let (content, category, tags, reason) = match decision {
            MergeDecision::Merge {
                content,
                category,
                tags,
                reason,
            } => (content, category, tags, reason),
            MergeDecision::KeepSeparate { reason } => {
                tracing::debug!(size = cluster.len(), %reason, "cluster left alone");
                return Ok(MergeOutcome::KeptSeparate { reason });
            }
        };

        // Confidence is the cluster's highest, not an average. Every
        // member asserts the same thing, so agreement between them is
        // evidence for it — averaging would make a memory *less* certain
        // for having been said twice.
        let confidence = cluster
            .iter()
            .map(Memory::confidence)
            .fold(0.0f32, f32::max);

        // Every member of a cluster shares a subcategory — the runner
        // groups by (category, subcategory) before clustering — so the
        // merged memory keeps it rather than dropping to the bare
        // category, which would erase the sub-label and drop the memory
        // out of subcategory-scoped recall.
        let subcategory = cluster
            .iter()
            .find_map(|memory| memory.subcategory().map(str::to_string));

        // The replacement is written before anything is retired. If the
        // save fails, the cluster is untouched and the next run tries
        // again; the reverse order would leave the memories superseded by
        // an id that does not exist.
        let replacement = self.saver.execute(
            context,
            NewMemory {
                content,
                category,
                subcategory,
                tags,
                // Entities are re-derivable from the merged content by a
                // later extraction pass, and picking a union of the
                // members' entities would attach names the replacement no
                // longer mentions.
                entities: Vec::new(),
                confidence,
                source: MemorySource {
                    client: Some(ACTOR.to_string()),
                    session_id: None,
                },
                // A merged memory does not inherit an expiry. If one
                // member was set to expire and another was not, the
                // durable one wins — the merge asserts it is still true.
                expires_at: None,
            },
            ACTOR,
        )?;

        let members: Vec<MemoryId> = cluster.iter().map(Memory::id).collect();
        let retired = self.memories.merge(
            context,
            &members,
            replacement.id(),
            ACTOR,
            &format!("merged into {}: {reason}", replacement.id()),
        )?;

        tracing::info!(
            replacement = %replacement.id(),
            retired,
            cluster = cluster.len(),
            "merged a cluster of duplicates"
        );

        Ok(MergeOutcome::Merged {
            replacement: replacement.id(),
            retired,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, new_memory};
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory_repository::AuditOperation;
    use crate::memories::domain::recall_query::RecallQuery;
    use crate::understanding::application::scripted_chat_model::ScriptedChatModel;
    use serde_json::json;

    fn merger(
        fixture: &Fixture,
        model: ScriptedChatModel,
    ) -> (MemoryMerger, Arc<ScriptedChatModel>) {
        let model = Arc::new(model);
        (
            MemoryMerger::new(
                Arc::new(fixture.saver()),
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                Arc::clone(&model) as Arc<dyn ChatModel>,
                Arc::new(Taxonomy::new(vec![])),
            ),
            model,
        )
    }

    /// Five phrasings of one preference — the DoD scenario.
    fn five_phrasings(fixture: &Fixture) -> Vec<Memory> {
        [
            "Prefers pnpm",
            "User uses pnpm",
            "pnpm is the package manager here",
            "Never use npm on this project",
            "yarn is not used; pnpm instead",
        ]
        .iter()
        .map(|content| fixture.save(&fixture.alex, content))
        .collect()
    }

    fn merged_reply() -> serde_json::Value {
        json!({
            "merge": true,
            "content": "User uses pnpm as the package manager; never npm or yarn",
            "category": "preference.coding",
            "tags": ["tooling"],
            "reason": "five phrasings of one package-manager preference",
        })
    }

    fn recall(fixture: &Fixture, query: &str) -> Vec<String> {
        fixture
            .recaller()
            .execute(&fixture.alex, &RecallQuery::new(query, 20).unwrap())
            .unwrap()
            .into_iter()
            .map(|scored| scored.memory.content().to_string())
            .collect()
    }

    #[tokio::test]
    async fn five_phrasings_become_one_active_memory_and_five_superseded() {
        let fixture = Fixture::new();
        let cluster = five_phrasings(&fixture);
        let (merger, _) = merger(&fixture, ScriptedChatModel::new().queue(merged_reply()));

        let outcome = merger.execute(&fixture.alex, &cluster).await.unwrap();

        let replacement = match outcome {
            MergeOutcome::Merged {
                replacement,
                retired,
            } => {
                assert_eq!(retired, 5, "every member should have been retired");
                replacement
            }
            other => panic!("expected a merge, got {other:?}"),
        };

        assert_eq!(
            recall(&fixture, "package manager"),
            ["User uses pnpm as the package manager; never npm or yarn"],
            "recall should return exactly one memory after a merge"
        );

        // Superseded, not deleted: still there, still reachable.
        let including = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &RecallQuery::new("package manager", 20)
                    .unwrap()
                    .including_superseded(),
            )
            .unwrap();
        assert_eq!(including.len(), 6, "the originals should still exist");

        for memory in &cluster {
            let stored = fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .expect("a superseded memory is retained");
            assert_eq!(stored.superseded_by(), Some(replacement));
        }
    }

    #[tokio::test]
    async fn a_merged_memory_keeps_the_clusters_subcategory() {
        // Clustering happens within a (category, subcategory) group, so a
        // merge must not silently drop the sub-label to `None` — that
        // would erase the memory from subcategory-scoped recall and undo
        // the finer labelling over time.
        let fixture = Fixture::new();
        let cluster: Vec<Memory> = ["Prefers pnpm", "User uses pnpm", "pnpm is the one here"]
            .into_iter()
            .map(|content| {
                let mut new = new_memory(content);
                new.subcategory = Some("tooling".to_string());
                fixture.saver().execute(&fixture.alex, new, "test").unwrap()
            })
            .collect();

        let (merger, _) = merger(&fixture, ScriptedChatModel::new().queue(merged_reply()));
        let outcome = merger.execute(&fixture.alex, &cluster).await.unwrap();

        let replacement = match outcome {
            MergeOutcome::Merged { replacement, .. } => replacement,
            other => panic!("expected a merge, got {other:?}"),
        };

        let merged = fixture
            .memories
            .find(&fixture.alex, replacement)
            .unwrap()
            .expect("the merged memory exists");
        assert_eq!(
            merged.subcategory(),
            Some("tooling"),
            "the merge dropped the cluster's subcategory"
        );
    }

    #[tokio::test]
    async fn every_retired_memory_gets_a_merge_audit_entry_with_the_reason() {
        // "Why did five of my memories become one overnight?" is only
        // answerable if the trail says so, in a way distinguishable from
        // the user having changed their mind.
        let fixture = Fixture::new();
        let cluster = five_phrasings(&fixture);
        let (merger, _) = merger(&fixture, ScriptedChatModel::new().queue(merged_reply()));

        merger.execute(&fixture.alex, &cluster).await.unwrap();

        let audit = fixture.memories.audit_trail(&fixture.alex, 100).unwrap();
        let merges: Vec<_> = audit
            .iter()
            .filter(|entry| entry.operation == AuditOperation::Merge)
            .collect();

        assert_eq!(merges.len(), 5);
        for entry in &merges {
            assert_eq!(entry.actor, ACTOR);
            assert!(
                entry.detail.contains("five phrasings"),
                "the model's reasoning must reach the trail: {:?}",
                entry.detail
            );
        }
    }

    #[tokio::test]
    async fn a_cluster_the_model_declines_is_left_entirely_alone() {
        // The expensive mistake this guards: two tooling preferences sit
        // very close in embedding space and are both true.
        let fixture = Fixture::new();
        let cluster = vec![
            fixture.save(&fixture.alex, "Prefers pnpm"),
            fixture.save(&fixture.alex, "Prefers Vitest"),
        ];
        let (merger, _) = merger(
            &fixture,
            ScriptedChatModel::new().queue(json!({
                "merge": false,
                "reason": "two different tools, both true",
            })),
        );

        let outcome = merger.execute(&fixture.alex, &cluster).await.unwrap();

        assert!(matches!(outcome, MergeOutcome::KeptSeparate { .. }));
        assert_eq!(recall(&fixture, "prefers").len(), 2);
        assert!(
            fixture
                .memories
                .audit_trail(&fixture.alex, 100)
                .unwrap()
                .iter()
                .all(|entry| entry.operation != AuditOperation::Merge),
            "a declined merge should leave no trace"
        );
    }

    #[tokio::test]
    async fn the_merged_memory_takes_the_clusters_highest_confidence() {
        // Every member asserts the same thing, so agreement is evidence
        // for it. Averaging would make a memory less certain for having
        // been said twice.
        let fixture = Fixture::new();
        let mut low = new_memory("Prefers pnpm");
        low.confidence = 0.4;
        let mut high = new_memory("User uses pnpm, never npm");
        high.confidence = 0.95;
        let cluster = vec![
            fixture.saver().execute(&fixture.alex, low, "test").unwrap(),
            fixture
                .saver()
                .execute(&fixture.alex, high, "test")
                .unwrap(),
        ];

        let (merger, _) = merger(&fixture, ScriptedChatModel::new().queue(merged_reply()));
        let outcome = merger.execute(&fixture.alex, &cluster).await.unwrap();

        let MergeOutcome::Merged { replacement, .. } = outcome else {
            panic!("expected a merge");
        };
        let merged = fixture
            .memories
            .find(&fixture.alex, replacement)
            .unwrap()
            .unwrap();
        assert!((merged.confidence() - 0.95).abs() < 1e-6);
    }

    #[tokio::test]
    async fn a_member_deleted_since_the_snapshot_is_skipped_not_fatal() {
        // A cluster is assembled from a snapshot and applied later. The
        // user is allowed to have deleted one of its members in between.
        let fixture = Fixture::new();
        let cluster = five_phrasings(&fixture);
        fixture
            .forgetter()
            .execute(&fixture.alex, cluster[0].id(), "user", "")
            .unwrap();

        let (merger, _) = merger(&fixture, ScriptedChatModel::new().queue(merged_reply()));
        let outcome = merger.execute(&fixture.alex, &cluster).await.unwrap();

        assert_eq!(
            outcome,
            MergeOutcome::Merged {
                replacement: match outcome {
                    MergeOutcome::Merged { replacement, .. } => replacement,
                    _ => unreachable!(),
                },
                retired: 4,
            },
            "the deleted member should be skipped, the rest merged"
        );
    }

    #[tokio::test]
    async fn a_cluster_of_one_never_reaches_the_model() {
        let fixture = Fixture::new();
        let cluster = vec![fixture.save(&fixture.alex, "Prefers pnpm")];
        let (merger, model) = merger(&fixture, ScriptedChatModel::new());

        assert!(merger.execute(&fixture.alex, &cluster).await.is_err());
        assert_eq!(model.call_count(), 0, "an unnecessary model call was made");
    }

    #[tokio::test]
    async fn the_merged_memory_is_attributed_to_consolidation() {
        let fixture = Fixture::new();
        let cluster = five_phrasings(&fixture);
        let (merger, _) = merger(&fixture, ScriptedChatModel::new().queue(merged_reply()));

        let outcome = merger.execute(&fixture.alex, &cluster).await.unwrap();
        let MergeOutcome::Merged { replacement, .. } = outcome else {
            panic!("expected a merge");
        };

        let merged = fixture
            .memories
            .find(&fixture.alex, replacement)
            .unwrap()
            .unwrap();
        assert_eq!(merged.source().client.as_deref(), Some(ACTOR));
        assert_eq!(merged.category(), &Category::PreferenceCoding);
    }

    #[tokio::test]
    async fn a_provider_failure_leaves_the_cluster_untouched() {
        // The night's run should be retryable, not half-applied.
        let fixture = Fixture::new();
        let cluster = five_phrasings(&fixture);
        let (merger, _) = merger(
            &fixture,
            ScriptedChatModel::new().queue_error(
                crate::understanding::domain::chat_model::ChatError::Transient("429".to_string()),
            ),
        );

        assert!(merger.execute(&fixture.alex, &cluster).await.is_err());
        assert_eq!(
            recall(&fixture, "package manager").len(),
            5,
            "a failed merge modified the store"
        );
    }
}
