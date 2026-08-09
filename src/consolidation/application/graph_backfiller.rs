//! `graph backfill` — give an existing corpus a graph, cheaply (Task 7.3.5).
//!
//! A store that ran before the graph existed, or with `[graph].enabled =
//! false`, has memories carrying `entities` JSON but no rows in the edge
//! tables. Two passes fill them:
//!
//! * **`--entities`** projects each memory's stored `entities` into
//!   `memory_entities`. It makes **no model call** — the entities were
//!   extracted at ingest and are already on the row — so every install can
//!   run it for free, and it is idempotent: re-running it just re-projects.
//!
//! * **`--relations`** re-extracts relations for memories that have none,
//!   one model call each, bounded by `[graph].backfill_budget` and resuming
//!   from a per-user watermark so a stopped run is picked up rather than
//!   repeated. Relations are re-anchored to the memory's *stored* entities,
//!   so this pass can never contradict what `--entities` projected.
//!
//! It lives in `consolidation` because it is the same kind of job: an
//! offline, budgeted, resumable sweep over every user's corpus, reusing that
//! context's `ConsolidationBudget` and the `BackgroundUserResolver` verbatim.

use crate::consolidation::application::consolidation_runner::{BudgetLimits, ConsolidationBudget};
use crate::identity::application::background_user_resolver::BackgroundUserResolver;
use crate::identity::domain::user_repository::UserRepository;
use crate::memories::domain::entity_graph::{EntityGraph, Relation};
use crate::memories::domain::entity_key::EntityKey;
use crate::memories::domain::graph_backfill_state::GraphBackfillState;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::shared::error::{RaError, Result};
use crate::understanding::application::candidate_extractor::CandidateExtractor;
use crate::understanding::domain::candidate::MAX_RELATIONS;
use crate::understanding::domain::extraction_prompt::SourceHints;
use std::collections::HashSet;
use std::sync::Arc;

/// Which pass a report describes, so it can render the fields that matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackfillPass {
    #[default]
    Entities,
    Relations,
}

/// What a pass did, or (in a dry run) would do.
#[derive(Debug, Default, PartialEq)]
pub struct BackfillReport {
    pub pass: BackfillPass,
    pub dry_run: bool,
    pub users: usize,
    pub memories_examined: usize,
    /// `--entities`: memories whose stored entities were projected.
    pub memories_projected: usize,
    /// `--relations`: memories that gained at least one edge. Zero in a dry
    /// run, which cannot know the answer without spending the call.
    pub relations_added: usize,
    /// `--relations`: model calls made, or in a dry run, model calls the
    /// real run would make.
    pub llm_calls: usize,
    pub budget_exhausted: bool,
    pub budget_reason: Option<String>,
}

pub struct GraphBackfiller {
    users: Arc<dyn UserRepository>,
    resolver: Arc<BackgroundUserResolver>,
    memories: Arc<dyn MemoryRepository>,
    graph: Arc<dyn EntityGraph>,
    state: Arc<dyn GraphBackfillState>,
    /// Present only when a provider is configured. `--relations` needs it;
    /// `--entities` does not, so an install with no model can still backfill
    /// entities.
    extractor: Option<Arc<CandidateExtractor>>,
    budget: BudgetLimits,
}

impl GraphBackfiller {
    pub fn new(
        users: Arc<dyn UserRepository>,
        memories: Arc<dyn MemoryRepository>,
        graph: Arc<dyn EntityGraph>,
        state: Arc<dyn GraphBackfillState>,
        extractor: Option<Arc<CandidateExtractor>>,
        budget: BudgetLimits,
    ) -> Self {
        Self {
            resolver: Arc::new(BackgroundUserResolver::new(Arc::clone(&users))),
            users,
            memories,
            graph,
            state,
            extractor,
            budget,
        }
    }

    /// Projects every memory's stored entities into the graph. Zero model
    /// calls; safe to run repeatedly.
    pub fn backfill_entities(&self, dry_run: bool) -> Result<BackfillReport> {
        let mut report = BackfillReport {
            pass: BackfillPass::Entities,
            dry_run,
            ..Default::default()
        };

        let users = self.users.list()?;
        report.users = users.len();
        for user in users {
            let context = match self.resolver.execute(user.id()) {
                Ok(context) => context,
                Err(error) => {
                    tracing::warn!(user = %user.handle(), %error, "skipping a user");
                    continue;
                }
            };
            // Every stored memory, superseded ones included: ingest recorded
            // an edge for each, so the projection has to match to be a true
            // rebuild. Recall filters superseded memories after the hop
            // regardless.
            for memory in self.memories.list(&context, true)? {
                report.memories_examined += 1;
                if memory.entities().is_empty() {
                    continue;
                }
                report.memories_projected += 1;
                if !dry_run {
                    self.graph
                        .record_entities(&context, memory.id(), memory.entities())?;
                }
            }
        }
        Ok(report)
    }

    /// Re-extracts relations for memories that have none, bounded by the
    /// backfill budget and resuming from each user's watermark.
    pub async fn backfill_relations(&self, dry_run: bool) -> Result<BackfillReport> {
        let mut report = BackfillReport {
            pass: BackfillPass::Relations,
            dry_run,
            ..Default::default()
        };

        // Only a real run needs the model. A dry run just counts the calls
        // it *would* make, which is the whole point of `--dry-run`: preview
        // the spend before a provider is even configured.
        if !dry_run && self.extractor.is_none() {
            return Err(RaError::Validation(
                "graph backfill --relations needs a configured [understanding] provider; \
                 --entities does not, and --relations --dry-run only previews the cost"
                    .to_string(),
            ));
        }

        let users = self.users.list()?;
        report.users = users.len();
        let mut budget = ConsolidationBudget::new(self.budget);

        for user in users {
            if budget.is_exhausted() {
                report.budget_exhausted = true;
                report.budget_reason = budget.reason();
                break;
            }
            let context = match self.resolver.execute(user.id()) {
                Ok(context) => context,
                Err(error) => {
                    tracing::warn!(user = %user.handle(), %error, "skipping a user");
                    continue;
                }
            };

            let cursor = self.state.relations_cursor(&context)?;
            // Oldest first, id-tiebroken, so the watermark advances
            // monotonically and two memories sharing an instant have a
            // stable order.
            let mut memories = self.memories.list(&context, true)?;
            memories.sort_by(|a, b| {
                a.created_at()
                    .cmp(&b.created_at())
                    .then_with(|| a.id().to_string().cmp(&b.id().to_string()))
            });

            for memory in memories {
                if cursor.is_some_and(|cursor| memory.created_at() <= cursor) {
                    continue;
                }
                if budget.is_exhausted() {
                    report.budget_exhausted = true;
                    report.budget_reason = budget.reason();
                    // The cursor already sits at the last memory finished, so
                    // the next run resumes here.
                    return Ok(report);
                }
                report.memories_examined += 1;

                // Ingest already gave this one edges: nothing to spend, just
                // step the watermark past it.
                if self.graph.has_relations(&context, memory.id())? {
                    if !dry_run {
                        self.state
                            .set_relations_cursor(&context, memory.created_at())?;
                    }
                    continue;
                }

                // A model call is due. In a dry run we count it and touch
                // nothing; otherwise we extract, write, and advance.
                report.llm_calls += 1;
                if !dry_run {
                    // Present on any non-dry run: the entry guard above
                    // returns before here when the extractor is missing.
                    let extractor = self
                        .extractor
                        .as_ref()
                        .expect("a real relation backfill requires an extractor");
                    let relations = self.extract_relations(extractor, &memory).await?;
                    self.graph.record_relations(
                        &context,
                        memory.id(),
                        &relations,
                        memory.created_at(),
                    )?;
                    if !relations.is_empty() {
                        report.relations_added += 1;
                    }
                    self.state
                        .set_relations_cursor(&context, memory.created_at())?;
                    budget.record_llm_call();
                }
                // A memory finished, in either mode, so the preview a dry run
                // prints stops at the same place a real run would.
                if budget.record_retired(1) {
                    report.budget_exhausted = true;
                    report.budget_reason = budget.reason();
                    return Ok(report);
                }
            }
        }
        Ok(report)
    }

    /// Re-extracts a memory's relations and re-anchors them to its *stored*
    /// entities: an edge survives only when both endpoints are entities the
    /// memory already declared, by the same [`EntityKey`] the graph files
    /// them under. That keeps the relation backfill from rewriting a
    /// memory's entity set or wiring in something the fresh extraction
    /// merely name-dropped, and caps the count like ingest does.
    async fn extract_relations(
        &self,
        extractor: &CandidateExtractor,
        memory: &Memory,
    ) -> Result<Vec<Relation>> {
        let candidates = extractor
            .execute(memory.content(), &SourceHints::default())
            .await?;

        let declared: HashSet<String> = memory
            .entities()
            .iter()
            .map(|entity| EntityKey::new(&entity.name).as_str().to_string())
            .filter(|key| !key.is_empty())
            .collect();

        let mut seen = HashSet::new();
        let mut kept = Vec::new();
        for relation in candidates.iter().flat_map(|candidate| &candidate.relations) {
            let subject = EntityKey::new(&relation.subject);
            let object = EntityKey::new(&relation.object);
            if subject.is_empty()
                || object.is_empty()
                || subject == object
                || !declared.contains(subject.as_str())
                || !declared.contains(object.as_str())
            {
                continue;
            }
            let identity = format!(
                "{}|{}|{}",
                subject.as_str(),
                relation.predicate,
                object.as_str()
            );
            if seen.insert(identity) {
                kept.push(relation.clone());
            }
            if kept.len() >= MAX_RELATIONS {
                break;
            }
        }
        Ok(kept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::domain::user_context::UserContext;
    use crate::memories::application::test_doubles::{Fixture, now};
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory::{Entity, MemorySource, NewMemory};
    use crate::memories::domain::memory_repository::MemoryRepository;
    use crate::understanding::application::scripted_chat_model::ScriptedChatModel;
    use crate::understanding::domain::chat_model::{ChatModel, ChatResult, StructuredRequest};
    use crate::understanding::domain::taxonomy::Taxonomy;
    use chrono::{DateTime, Duration, Utc};
    use serde_json::{Value, json};

    /// A model that must never be reached — the entity pass makes no call,
    /// and a dry run of the relation pass spends nothing.
    struct PanickingChatModel;

    #[async_trait::async_trait]
    impl ChatModel for PanickingChatModel {
        fn model_id(&self) -> &str {
            "panicking"
        }
        async fn complete_structured(&self, _: &StructuredRequest) -> ChatResult<Value> {
            panic!("the model must not be called on this path");
        }
    }

    fn entity(name: &str, kind: &str) -> Entity {
        Entity {
            name: name.to_string(),
            kind: kind.to_string(),
        }
    }

    /// Inserts a memory carrying `entities` straight into the repository —
    /// no vector or keyword indexing, which the backfill does not need.
    fn memory_with(
        fixture: &Fixture,
        context: &UserContext,
        content: &str,
        entities: Vec<Entity>,
        created_at: DateTime<Utc>,
    ) -> crate::memories::domain::memory::Memory {
        let memory = crate::memories::domain::memory::Memory::create(
            context.user_id(),
            NewMemory {
                content: content.to_string(),
                category: Category::FactProject,
                subcategory: None,
                tags: vec![],
                entities,
                confidence: 1.0,
                source: MemorySource::default(),
                expires_at: None,
            },
            created_at,
        )
        .unwrap();
        fixture.memories.insert(context, &memory, "test").unwrap();
        memory
    }

    fn extractor(model: impl ChatModel + 'static) -> Arc<CandidateExtractor> {
        Arc::new(CandidateExtractor::new(
            Arc::new(model),
            Arc::new(Taxonomy::new(vec![])),
            true,
        ))
    }

    /// One scripted extraction that asserts `backend deploys_on Hetzner`.
    fn deploys_on_reply() -> Value {
        json!({"candidates": [{
            "content": "the backend deploys on Hetzner",
            "category": "fact.project",
            "entities": [
                {"name": "backend", "kind": "component"},
                {"name": "Hetzner", "kind": "service"},
            ],
            "relations": [{"subject": "backend", "predicate": "deploys_on", "object": "Hetzner"}],
        }]})
    }

    // The real SQLite watermark store, reached by an inline path rather than
    // a `use` — a `use crate::…::infrastructure` in an application module is
    // what the boundary check forbids, and the memories fixture reaches its
    // graph adapter the same way.
    fn state(
        fixture: &Fixture,
    ) -> Arc<crate::memories::infrastructure::sqlite_graph_backfill_state::SqliteGraphBackfillState>
    {
        Arc::new(
            crate::memories::infrastructure::sqlite_graph_backfill_state::SqliteGraphBackfillState::new(
                Arc::clone(&fixture.database),
            ),
        )
    }

    fn unlimited() -> BudgetLimits {
        BudgetLimits::default()
    }

    #[test]
    fn entity_backfill_writes_edges_and_never_calls_the_model() {
        let fixture = Fixture::new();
        let graph = fixture.graph();
        memory_with(
            &fixture,
            &fixture.alex,
            "the backend runs on Hetzner",
            vec![entity("backend", "component"), entity("Hetzner", "service")],
            now(),
        );

        let backfiller = GraphBackfiller::new(
            Arc::clone(&fixture.users),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&graph) as Arc<dyn EntityGraph>,
            state(&fixture) as Arc<dyn GraphBackfillState>,
            // A model that panics if touched: the entity pass must not reach it.
            Some(extractor(PanickingChatModel)),
            unlimited(),
        );

        let report = backfiller.backfill_entities(false).unwrap();
        assert_eq!(report.memories_projected, 1);
        assert_eq!(report.memories_examined, 1);

        // The entity is now a seed the graph knows.
        let seeds = graph
            .seeds(&fixture.alex, &[EntityKey::new("backend")])
            .unwrap();
        assert_eq!(seeds.len(), 1, "the entity was not projected");
    }

    #[test]
    fn an_entity_dry_run_counts_but_writes_nothing() {
        let fixture = Fixture::new();
        let graph = fixture.graph();
        memory_with(
            &fixture,
            &fixture.alex,
            "the backend runs on Hetzner",
            vec![entity("backend", "component")],
            now(),
        );

        let backfiller = GraphBackfiller::new(
            Arc::clone(&fixture.users),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&graph) as Arc<dyn EntityGraph>,
            state(&fixture) as Arc<dyn GraphBackfillState>,
            None,
            unlimited(),
        );

        let report = backfiller.backfill_entities(true).unwrap();
        assert_eq!(report.memories_projected, 1);
        assert!(
            graph
                .seeds(&fixture.alex, &[EntityKey::new("backend")])
                .unwrap()
                .is_empty(),
            "a dry run wrote entity rows"
        );
    }

    #[tokio::test]
    async fn relation_backfill_stops_at_its_budget_and_resumes_where_it_stopped() {
        let fixture = Fixture::new();
        let graph = fixture.graph();
        let ents = || vec![entity("backend", "component"), entity("Hetzner", "service")];
        let first = memory_with(&fixture, &fixture.alex, "backend one", ents(), now());
        let second = memory_with(
            &fixture,
            &fixture.alex,
            "backend two",
            ents(),
            now() + Duration::seconds(1),
        );
        let third = memory_with(
            &fixture,
            &fixture.alex,
            "backend three",
            ents(),
            now() + Duration::seconds(2),
        );

        // Three replies queued; one call is allowed per run.
        let model = ScriptedChatModel::new()
            .queue(deploys_on_reply())
            .queue(deploys_on_reply())
            .queue(deploys_on_reply());
        let backfiller = GraphBackfiller::new(
            Arc::clone(&fixture.users),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&graph) as Arc<dyn EntityGraph>,
            state(&fixture) as Arc<dyn GraphBackfillState>,
            Some(extractor(model)),
            BudgetLimits {
                max_llm_calls: Some(1),
                ..BudgetLimits::default()
            },
        );

        let run_one = backfiller.backfill_relations(false).await.unwrap();
        assert_eq!(run_one.llm_calls, 1);
        assert!(
            run_one.budget_exhausted,
            "the budget should have stopped it"
        );
        assert_eq!(run_one.relations_added, 1);
        assert!(graph.has_relations(&fixture.alex, first.id()).unwrap());
        assert!(!graph.has_relations(&fixture.alex, second.id()).unwrap());

        // Resumes past the first memory rather than re-doing it.
        let run_two = backfiller.backfill_relations(false).await.unwrap();
        assert_eq!(run_two.llm_calls, 1);
        assert!(graph.has_relations(&fixture.alex, second.id()).unwrap());
        assert!(
            !graph.has_relations(&fixture.alex, third.id()).unwrap(),
            "the third memory should still be untouched after two capped runs"
        );
    }

    #[tokio::test]
    async fn a_relation_dry_run_makes_no_calls_and_writes_nothing() {
        let fixture = Fixture::new();
        let graph = fixture.graph();
        let one = memory_with(
            &fixture,
            &fixture.alex,
            "backend one",
            vec![entity("backend", "component")],
            now(),
        );
        let two = memory_with(
            &fixture,
            &fixture.alex,
            "backend two",
            vec![entity("backend", "component")],
            now() + Duration::seconds(1),
        );

        let backfiller = GraphBackfiller::new(
            Arc::clone(&fixture.users),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&graph) as Arc<dyn EntityGraph>,
            state(&fixture) as Arc<dyn GraphBackfillState>,
            // Panics if a call is made — a dry run must not spend.
            Some(extractor(PanickingChatModel)),
            unlimited(),
        );

        let report = backfiller.backfill_relations(true).await.unwrap();
        assert_eq!(report.memories_examined, 2);
        assert_eq!(
            report.llm_calls, 2,
            "it should report the calls it would make"
        );
        assert_eq!(report.relations_added, 0);
        assert!(!graph.has_relations(&fixture.alex, one.id()).unwrap());
        assert!(!graph.has_relations(&fixture.alex, two.id()).unwrap());
        // And the watermark did not move, so a real run still starts here.
        assert_eq!(
            state(&fixture).relations_cursor(&fixture.alex).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn a_relation_dry_run_needs_no_provider() {
        // Previewing the cost must not require a model to be configured —
        // the point of --dry-run is to decide *whether* to spend.
        let fixture = Fixture::new();
        let graph = fixture.graph();
        memory_with(
            &fixture,
            &fixture.alex,
            "backend one",
            vec![entity("backend", "component")],
            now(),
        );

        let backfiller = GraphBackfiller::new(
            Arc::clone(&fixture.users),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&graph) as Arc<dyn EntityGraph>,
            state(&fixture) as Arc<dyn GraphBackfillState>,
            // No extractor at all — no provider configured.
            None,
            unlimited(),
        );

        let report = backfiller.backfill_relations(true).await.unwrap();
        assert_eq!(report.memories_examined, 1);
        assert_eq!(report.llm_calls, 1, "it reports the call it would make");
    }

    #[tokio::test]
    async fn a_real_relation_backfill_without_a_provider_errors() {
        // A run that would actually spend still refuses without a model.
        let fixture = Fixture::new();
        let graph = fixture.graph();
        let backfiller = GraphBackfiller::new(
            Arc::clone(&fixture.users),
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            Arc::clone(&graph) as Arc<dyn EntityGraph>,
            state(&fixture) as Arc<dyn GraphBackfillState>,
            None,
            unlimited(),
        );

        let error = backfiller.backfill_relations(false).await.unwrap_err();
        assert!(
            matches!(error, crate::shared::error::RaError::Validation(_)),
            "got {error:?}"
        );
    }
}
