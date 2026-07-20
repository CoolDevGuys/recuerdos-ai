//! Candidate → the store, deciding what it does to what is already there.
//!
//! The half of the pipeline that keeps a memory store from becoming a
//! log. Extraction says "this is worth remembering"; this decides whether
//! remembering it means adding, replacing, removing, or nothing at all.
//!
//! # The cost saver that also improves quality
//!
//! A candidate with no similar existing memories is added without asking
//! a model. That halves the token cost of ingesting a fresh corpus, but
//! the better argument is that the question is not worth asking: with
//! nothing to contradict, the only available answer is ADD, and giving a
//! model a decision it cannot get right only creates chances to get it
//! wrong.
//!
//! # Blocking work
//!
//! Recall and the writes are synchronous SQLite calls sitting between two
//! `await`s. They go through `spawn_blocking` rather than running inline:
//! an ingest worker shares its runtime with the HTTP server, and stalling
//! a runtime thread on a database call is how that server stops answering
//! under load. Hence the `Clone` — each blocking section takes its own
//! handle to the same `Arc`s.

use crate::identity::domain::user_context::UserContext;
use crate::memories::application::direct_memory_saver::DirectMemorySaver;
use crate::memories::application::memory_forgetter::MemoryForgetter;
use crate::memories::application::memory_recaller::MemoryRecaller;
use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::recall_query::RecallQuery;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use crate::understanding::domain::candidate::Candidate;
use crate::understanding::domain::chat_model::ChatModel;
use crate::understanding::domain::reconciliation::{
    Decision, decisions_array, parse_decision, reconciliation_request,
};
use std::sync::Arc;

/// How many existing memories the model is shown.
///
/// Small on purpose. Every neighbour is tokens spent on every candidate,
/// and a memory ranked eighth by hybrid search is rarely the one being
/// contradicted — while a longer list measurably increases the chance the
/// model picks a loosely-related memory and supersedes something it
/// should have left alone.
pub const NEIGHBOUR_LIMIT: usize = 5;

#[derive(Clone)]
pub struct MemoryReconciler {
    recaller: Arc<MemoryRecaller>,
    saver: Arc<DirectMemorySaver>,
    forgetter: Arc<MemoryForgetter>,
    memories: Arc<dyn MemoryRepository>,
    model: Arc<dyn ChatModel>,
    /// `[understanding].reconcile = false` — extract, but never supersede
    /// or delete. For a deployment that wants labelling without letting a
    /// model remove anything.
    enabled: bool,
}

/// What reconciling one candidate did.
#[derive(Debug, Default)]
pub struct ReconcileOutcome {
    /// Memories created. Reported back to the caller through the job.
    pub stored: Vec<MemoryId>,
    pub superseded: Vec<MemoryId>,
    pub deleted: Vec<MemoryId>,
}

impl ReconcileOutcome {
    fn absorb(&mut self, other: ReconcileOutcome) {
        self.stored.extend(other.stored);
        self.superseded.extend(other.superseded);
        self.deleted.extend(other.deleted);
    }
}

impl MemoryReconciler {
    pub fn new(
        recaller: Arc<MemoryRecaller>,
        saver: Arc<DirectMemorySaver>,
        forgetter: Arc<MemoryForgetter>,
        memories: Arc<dyn MemoryRepository>,
        model: Arc<dyn ChatModel>,
        enabled: bool,
    ) -> Self {
        Self {
            recaller,
            saver,
            forgetter,
            memories,
            model,
            enabled,
        }
    }

    /// Reconciles every candidate, in order.
    ///
    /// Sequentially rather than concurrently, and that is load-bearing:
    /// two candidates from one submission are often about the same thing,
    /// and running them in parallel means the second cannot see what the
    /// first just stored. It would then re-add its own copy of a memory
    /// that had just been written.
    pub async fn execute(
        &self,
        context: &UserContext,
        candidates: &[Candidate],
        actor: &str,
    ) -> Result<ReconcileOutcome> {
        let mut outcome = ReconcileOutcome::default();
        for candidate in candidates {
            outcome.absorb(self.reconcile_one(context, candidate, actor).await?);
        }
        Ok(outcome)
    }

    async fn reconcile_one(
        &self,
        context: &UserContext,
        candidate: &Candidate,
        actor: &str,
    ) -> Result<ReconcileOutcome> {
        let neighbours = if self.enabled {
            let this = self.clone();
            let (context, candidate) = (context.clone(), candidate.clone());
            blocking(move || this.neighbours(&context, &candidate)).await?
        } else {
            Vec::new()
        };

        if neighbours.is_empty() {
            // Nothing to contradict.
            let this = self.clone();
            let (context, candidate, actor) =
                (context.clone(), candidate.clone(), actor.to_string());
            let id = blocking(move || this.store(&context, &candidate, &actor)).await?;
            return Ok(ReconcileOutcome {
                stored: vec![id],
                ..Default::default()
            });
        }

        let shown: Vec<MemoryId> = neighbours.iter().map(Memory::id).collect();
        let answer = self
            .model
            .complete_structured(&reconciliation_request(candidate, &neighbours))
            .await?;

        let this = self.clone();
        let (context, candidate, actor) = (context.clone(), candidate.clone(), actor.to_string());
        blocking(move || this.apply(&context, &candidate, &answer, &shown, &actor)).await
    }

    /// The existing memories most similar to this candidate.
    ///
    /// A recall failure is not fatal: reconciliation without neighbours
    /// degrades to ADD, which stores a possible duplicate. Losing the
    /// memory entirely would be the worse outcome, and a duplicate is
    /// something consolidation can clean up later.
    fn neighbours(&self, context: &UserContext, candidate: &Candidate) -> Result<Vec<Memory>> {
        let query = match RecallQuery::new(&candidate.content, NEIGHBOUR_LIMIT) {
            Ok(query) => query,
            Err(error) => {
                // Content longer than a query is allowed to be. Comparing
                // its opening is better than comparing nothing.
                tracing::debug!(%error, "truncating a candidate to use it as a recall query");
                let head: String = candidate
                    .content
                    .chars()
                    .take(crate::memories::domain::recall_query::MAX_QUERY_LEN)
                    .collect();
                RecallQuery::new(&head, NEIGHBOUR_LIMIT)?
            }
        };

        match self.recaller.execute(context, &query) {
            Ok(scored) => Ok(scored.into_iter().map(|scored| scored.memory).collect()),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "could not look up neighbours; the candidate will be added without \
                     checking for contradictions"
                );
                Ok(Vec::new())
            }
        }
    }

    fn apply(
        &self,
        context: &UserContext,
        candidate: &Candidate,
        answer: &serde_json::Value,
        shown: &[MemoryId],
        actor: &str,
    ) -> Result<ReconcileOutcome> {
        let Some(raw) = decisions_array(answer) else {
            tracing::warn!("reconciliation returned no decisions; storing the candidate");
            return self
                .store(context, candidate, actor)
                .map(|id| ReconcileOutcome {
                    stored: vec![id],
                    ..Default::default()
                });
        };

        let mut decisions = Vec::new();
        for value in raw {
            match parse_decision(value, shown) {
                Ok(decision) => decisions.push(decision),
                Err(error) => {
                    // Dropped, not fatal. One unusable decision among
                    // several should not cost the whole reconciliation —
                    // and a decision naming a memory the model was never
                    // shown is exactly the one to discard.
                    tracing::warn!(%error, "ignoring a reconciliation decision");
                }
            }
        }

        if decisions.is_empty() {
            tracing::warn!("no usable reconciliation decisions; treating it as NOOP");
            return Ok(ReconcileOutcome::default());
        }

        let mut outcome = ReconcileOutcome::default();

        // Deletions first: a retraction should take effect even if
        // storing the candidate afterwards fails.
        for decision in &decisions {
            if let Decision::Delete { target, reason } = decision {
                self.forgetter.execute(context, *target, actor, reason)?;
                outcome.deleted.push(*target);
            }
        }

        // At most one memory per candidate, however many ADD/UPDATE
        // decisions came back. A model that returns two UPDATEs is asking
        // for the candidate to be stored twice, which is never right.
        let stored = decisions
            .iter()
            .find(|decision| decision.stores_the_candidate())
            .map(|decision| {
                self.store(context, candidate, actor).inspect(|id| {
                    tracing::debug!(
                        memory_id = %id,
                        reason = decision.reason(),
                        "reconciliation stored a candidate"
                    );
                })
            })
            .transpose()?;

        if let Some(id) = stored {
            outcome.stored.push(id);

            for decision in &decisions {
                if let Decision::Update { superseded, reason } = decision {
                    self.memories
                        .supersede(context, *superseded, id, actor, reason)?;
                    outcome.superseded.push(*superseded);
                }
            }
        }

        Ok(outcome)
    }

    fn store(&self, context: &UserContext, candidate: &Candidate, actor: &str) -> Result<MemoryId> {
        self.saver
            .execute(
                context,
                NewMemory {
                    content: candidate.content.clone(),
                    category: candidate.category.clone(),
                    tags: candidate.tags.clone(),
                    entities: candidate.entities.clone(),
                    confidence: candidate.confidence,
                    source: MemorySource {
                        client: Some(actor.to_string()),
                        session_id: None,
                    },
                    expires_at: None,
                },
                actor,
            )
            .map(|memory| memory.id())
    }
}

/// Runs a synchronous section off the runtime's worker threads.
async fn blocking<T, F>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work).await.map_err(|error| {
        crate::shared::error::RaError::Internal(format!("a reconciliation task panicked: {error}"))
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory_repository::AuditOperation;
    use crate::understanding::application::scripted_chat_model::ScriptedChatModel;
    use serde_json::json;

    const ACTOR: &str = "pipeline";

    fn candidate(content: &str) -> Candidate {
        Candidate {
            content: content.to_string(),
            category: Category::FactProject,
            tags: vec![],
            entities: vec![],
            confidence: 0.9,
        }
    }

    fn reconciler(
        fixture: &Fixture,
        model: ScriptedChatModel,
        enabled: bool,
    ) -> (MemoryReconciler, Arc<ScriptedChatModel>) {
        let model = Arc::new(model);
        (
            MemoryReconciler::new(
                Arc::new(fixture.recaller()),
                Arc::new(fixture.saver()),
                Arc::new(fixture.forgetter()),
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                Arc::clone(&model) as Arc<dyn ChatModel>,
                enabled,
            ),
            model,
        )
    }

    fn recall(fixture: &Fixture, query: &str) -> Vec<String> {
        fixture
            .recaller()
            .execute(&fixture.alex, &RecallQuery::new(query, 10).unwrap())
            .unwrap()
            .into_iter()
            .map(|scored| scored.memory.content().to_string())
            .collect()
    }

    #[tokio::test]
    async fn a_candidate_with_no_neighbours_is_added_without_calling_a_model() {
        // Both a cost saver and a correctness argument: with nothing to
        // contradict, ADD is the only available answer.
        let fixture = Fixture::new();
        let (reconciler, model) = reconciler(&fixture, ScriptedChatModel::new(), true);

        let outcome = reconciler
            .execute(
                &fixture.alex,
                &[candidate("The backend runs on Hetzner")],
                ACTOR,
            )
            .await
            .unwrap();

        assert_eq!(outcome.stored.len(), 1);
        assert_eq!(model.call_count(), 0, "an unnecessary model call was made");
        assert_eq!(
            recall(&fixture, "where does the backend run"),
            ["The backend runs on Hetzner"]
        );
    }

    #[tokio::test]
    async fn a_contradiction_supersedes_the_old_memory() {
        // The scenario the whole design exists for: after this, asking
        // "where do we deploy?" must not return Fly.io.
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "Backend deploys on Fly.io");

        let (reconciler, _) = reconciler(
            &fixture,
            ScriptedChatModel::new().queue(json!({"decisions": [{
                "action": "UPDATE",
                "memory_id": old.id().to_string(),
                "reason": "the deployment target changed from Fly.io to Hetzner",
            }]})),
            true,
        );

        let outcome = reconciler
            .execute(
                &fixture.alex,
                &[candidate("Backend deploys on Hetzner")],
                ACTOR,
            )
            .await
            .unwrap();

        assert_eq!(outcome.superseded, [old.id()]);
        assert_eq!(outcome.stored.len(), 1);

        let recalled = recall(&fixture, "Backend deploys");
        assert_eq!(
            recalled,
            ["Backend deploys on Hetzner"],
            "the superseded memory is still being recalled"
        );
    }

    #[tokio::test]
    async fn a_superseded_memory_is_retained_and_reachable_deliberately() {
        // Supersede is not delete. "What did we used to think?" has to
        // stay answerable.
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "Backend deploys on Fly.io");

        let (reconciler, _) = reconciler(
            &fixture,
            ScriptedChatModel::new().queue(json!({"decisions": [
                {"action": "UPDATE", "memory_id": old.id().to_string(), "reason": "moved"}
            ]})),
            true,
        );
        reconciler
            .execute(
                &fixture.alex,
                &[candidate("Backend deploys on Hetzner")],
                ACTOR,
            )
            .await
            .unwrap();

        let including = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &RecallQuery::new("Backend deploys", 10)
                    .unwrap()
                    .including_superseded(),
            )
            .unwrap();

        assert_eq!(including.len(), 2, "the old memory should still be there");
    }

    #[tokio::test]
    async fn the_rationale_reaches_the_audit_trail() {
        // "Why did my memory change?" is the question the trail exists to
        // answer, and an automated decision is exactly the case where the
        // user was not there to see it happen.
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "Backend deploys on Fly.io");

        let (reconciler, _) = reconciler(
            &fixture,
            ScriptedChatModel::new().queue(json!({"decisions": [{
                "action": "UPDATE",
                "memory_id": old.id().to_string(),
                "reason": "the deployment target changed to Hetzner",
            }]})),
            true,
        );
        reconciler
            .execute(
                &fixture.alex,
                &[candidate("Backend deploys on Hetzner")],
                ACTOR,
            )
            .await
            .unwrap();

        let audit = fixture.memories.audit_trail(&fixture.alex, 20).unwrap();
        let entry = audit
            .iter()
            .find(|entry| entry.operation == AuditOperation::Supersede)
            .expect("a supersede entry");

        assert_eq!(entry.memory_id, old.id());
        assert!(
            entry.detail.contains("Hetzner"),
            "detail was {:?}",
            entry.detail
        );
    }

    #[tokio::test]
    async fn a_duplicate_is_a_noop_and_stores_nothing() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm");

        let (reconciler, _) = reconciler(
            &fixture,
            ScriptedChatModel::new().queue(json!({"decisions": [
                {"action": "NOOP", "reason": "already known"}
            ]})),
            true,
        );

        let outcome = reconciler
            .execute(&fixture.alex, &[candidate("User prefers pnpm")], ACTOR)
            .await
            .unwrap();

        assert!(outcome.stored.is_empty());
        assert_eq!(recall(&fixture, "pnpm").len(), 1, "a duplicate was stored");
    }

    #[tokio::test]
    async fn a_retraction_deletes_without_storing_the_retraction_itself() {
        // "I no longer use Docker" should remove the Docker memory, not
        // add a memory recording that the user said so.
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "User builds images with Docker");

        let (reconciler, _) = reconciler(
            &fixture,
            ScriptedChatModel::new().queue(json!({"decisions": [{
                "action": "DELETE",
                "memory_id": old.id().to_string(),
                "reason": "the user retracted it",
            }]})),
            true,
        );

        let outcome = reconciler
            .execute(
                &fixture.alex,
                &[candidate("User no longer uses Docker")],
                ACTOR,
            )
            .await
            .unwrap();

        assert_eq!(outcome.deleted, [old.id()]);
        assert!(
            outcome.stored.is_empty(),
            "the retraction was stored as a memory"
        );
        assert!(recall(&fixture, "Docker").is_empty());
    }

    #[tokio::test]
    async fn a_related_but_different_memory_is_added_alongside() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm");

        let (reconciler, _) = reconciler(
            &fixture,
            ScriptedChatModel::new().queue(json!({"decisions": [
                {"action": "ADD", "reason": "a different tool, both are true"}
            ]})),
            true,
        );

        reconciler
            .execute(&fixture.alex, &[candidate("User prefers Vitest")], ACTOR)
            .await
            .unwrap();

        assert_eq!(recall(&fixture, "prefers").len(), 2);
    }

    #[tokio::test]
    async fn a_decision_naming_a_memory_the_model_was_not_shown_is_ignored() {
        // The hallucination case: an id that exists nowhere in the store.
        // Acting on it would delete at random; refusing costs nothing,
        // because the model had no grounds to judge a memory it never saw.
        let fixture = Fixture::new();
        let real = fixture.save(&fixture.alex, "User prefers pnpm");
        let invented = MemoryId::new();

        let (reconciler, _) = reconciler(
            &fixture,
            ScriptedChatModel::new().queue(json!({"decisions": [{
                "action": "DELETE",
                "memory_id": invented.to_string(),
                "reason": "invented",
            }]})),
            true,
        );

        let outcome = reconciler
            .execute(&fixture.alex, &[candidate("User prefers pnpm")], ACTOR)
            .await
            .unwrap();

        assert!(outcome.deleted.is_empty());
        assert!(
            outcome.stored.is_empty(),
            "an unusable decision list is a NOOP, not an ADD"
        );
        assert!(
            fixture
                .memories
                .find(&fixture.alex, real.id())
                .unwrap()
                .is_some(),
            "the real memory was collateral damage"
        );
    }

    #[tokio::test]
    async fn several_updates_still_store_the_candidate_only_once() {
        let fixture = Fixture::new();
        let first = fixture.save(&fixture.alex, "Backend deploys on Fly.io");
        let second = fixture.save(&fixture.alex, "Backend deploys on Fly.io machines");

        let (reconciler, _) = reconciler(
            &fixture,
            ScriptedChatModel::new().queue(json!({"decisions": [
                {"action": "UPDATE", "memory_id": first.id().to_string(), "reason": "moved"},
                {"action": "UPDATE", "memory_id": second.id().to_string(), "reason": "moved"}
            ]})),
            true,
        );

        let outcome = reconciler
            .execute(
                &fixture.alex,
                &[candidate("Backend deploys on Hetzner")],
                ACTOR,
            )
            .await
            .unwrap();

        assert_eq!(outcome.stored.len(), 1, "the candidate was stored twice");
        assert_eq!(
            outcome.superseded.len(),
            2,
            "both old memories should supersede"
        );
    }

    #[tokio::test]
    async fn reconciliation_can_be_turned_off_entirely() {
        // `reconcile = false`: label and store, never let a model remove
        // anything.
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "Backend deploys on Fly.io");

        let (reconciler, model) = reconciler(&fixture, ScriptedChatModel::new(), false);

        reconciler
            .execute(
                &fixture.alex,
                &[candidate("Backend deploys on Hetzner")],
                ACTOR,
            )
            .await
            .unwrap();

        assert_eq!(model.call_count(), 0);
        assert_eq!(
            recall(&fixture, "Backend deploys").len(),
            2,
            "with reconciliation off nothing should be superseded"
        );
    }

    #[tokio::test]
    async fn candidates_are_reconciled_in_order_so_each_sees_the_last() {
        // Running these concurrently would mean the second candidate
        // cannot see what the first just stored, and re-adds it.
        let fixture = Fixture::new();

        let (reconciler, _) = reconciler(
            &fixture,
            ScriptedChatModel::new().queue(json!({"decisions": [
                {"action": "NOOP", "reason": "the first candidate already said this"}
            ]})),
            true,
        );

        let outcome = reconciler
            .execute(
                &fixture.alex,
                &[
                    candidate("User prefers pnpm"),
                    candidate("User prefers pnpm as their package manager"),
                ],
                ACTOR,
            )
            .await
            .unwrap();

        assert_eq!(
            outcome.stored.len(),
            1,
            "the second candidate did not see the first"
        );
    }

    #[tokio::test]
    async fn unusable_decisions_leave_the_store_untouched() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm");

        let (reconciler, _) = reconciler(
            &fixture,
            ScriptedChatModel::new().queue(json!({"decisions": [
                {"action": "MERGE", "reason": "not a real action"}
            ]})),
            true,
        );

        let outcome = reconciler
            .execute(&fixture.alex, &[candidate("User prefers pnpm")], ACTOR)
            .await
            .unwrap();

        assert!(outcome.stored.is_empty());
        assert!(outcome.deleted.is_empty());
        assert_eq!(recall(&fixture, "pnpm").len(), 1);
    }
}
