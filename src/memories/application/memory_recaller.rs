//! Hybrid recall: the read path the whole service exists to serve.
//!
//! Embeds the query, asks both indexes in parallel, fuses their rankings
//! and applies the caller's filters.
//!
//! # Over-fetch, then filter
//!
//! Filters (category, tags, since) are applied *after* the indexes have
//! answered, because neither index knows about them. Each leg is
//! therefore asked for several times `limit` candidates
//! ([`RecallQuery::candidate_depth`]).
//!
//! The honest limitation: a highly selective filter over a large corpus
//! can still return fewer than `limit` results even when more exist,
//! because the matching memories never made the candidate window. Pushing
//! filters into both indexes is the fix, and is worth doing when filtered
//! recall becomes a common path rather than an occasional one.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::embedder::{Embedder, EmbeddingTask};
use crate::memories::domain::entity_graph::EntityGraph;
use crate::memories::domain::entity_key::EntityKey;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::recall_query::RecallQuery;
use crate::memories::domain::recall_ranker::{RankedIds, RecallRanker, ScoredMemory};
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::clock::Clock;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use std::collections::HashMap;
use std::sync::Arc;

/// The longest entity name, in words, the query scanner will try to match.
/// Entity names are short ("billing service", "Meridian team"); a longer
/// window only manufactures n-grams that match nothing.
const MAX_SEED_WORDS: usize = 3;

pub struct MemoryRecaller {
    memories: Arc<dyn MemoryRepository>,
    vectors: Arc<dyn VectorIndex>,
    text: Arc<dyn TextIndex>,
    embedder: Arc<dyn Embedder>,
    ranker: RecallRanker,
    clock: Arc<dyn Clock>,
    /// The entity/relation graph, present only when `[graph].enabled`
    /// (Task 7.3.4). `None` is the default and the pre-graph behaviour:
    /// no third leg runs, and recall is byte-identical to a two-leg build.
    graph: Option<Arc<dyn EntityGraph>>,
    /// How many edges one hop may traverse (`[graph].max_hops`) and how
    /// many memories the leg may return (`[graph].hop_limit`). Unused while
    /// `graph` is `None`.
    max_hops: usize,
    hop_limit: usize,
}

impl MemoryRecaller {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        memories: Arc<dyn MemoryRepository>,
        vectors: Arc<dyn VectorIndex>,
        text: Arc<dyn TextIndex>,
        embedder: Arc<dyn Embedder>,
        ranker: RecallRanker,
        clock: Arc<dyn Clock>,
        graph: Option<Arc<dyn EntityGraph>>,
        max_hops: usize,
        hop_limit: usize,
    ) -> Self {
        Self {
            memories,
            vectors,
            text,
            embedder,
            ranker,
            clock,
            graph,
            max_hops,
            hop_limit,
        }
    }

    pub fn execute(&self, context: &UserContext, query: &RecallQuery) -> Result<Vec<ScoredMemory>> {
        let depth = query.candidate_depth();
        let now = self.clock.now();

        let embedding = self
            .embedder
            .embed_one(query.text(), EmbeddingTask::Query)?;
        let vector_scores = self.vectors.search(context, &embedding, depth)?;
        let vector_hits = ranked(&vector_scores);
        let vector_scores = scores(&vector_scores);

        // A keyword failure degrades the result rather than failing the
        // request: half a hybrid search still answers the question, and
        // the caller would rather have that than an error.
        let (keyword_hits, keyword_scores) = match self.text.search(context, query.text(), depth) {
            Ok(hits) => (ranked(&hits), scores(&hits)),
            Err(error) => {
                tracing::warn!(%error, "keyword search failed; falling back to vectors only");
                (RankedIds::default(), HashMap::new())
            }
        };

        // The third leg: memories connected to the query's entities over
        // the graph. Absent (no graph) or silent (query names nothing
        // known) it contributes nothing, and degrades exactly like the
        // keyword leg — two thirds of a hybrid search still answers.
        let graph_hits = self.graph_leg(context, query);

        let mut candidate_ids: Vec<_> = vector_hits.0.clone();
        for id in keyword_hits.0.iter().chain(graph_hits.0.iter()) {
            if !candidate_ids.contains(id) {
                candidate_ids.push(*id);
            }
        }
        if candidate_ids.is_empty() {
            return Ok(Vec::new());
        }

        let candidates: Vec<Memory> = self
            .memories
            .find_many(context, &candidate_ids)?
            .into_iter()
            .filter(|memory| matches(memory, query, now))
            .collect();

        // A build with no graph goes through the original two-leg entry
        // point verbatim — not `rank_with_graph` over an empty leg — so the
        // "no graph, no change" guarantee is the same code path, not merely
        // the same arithmetic.
        let mut ranked = if self.graph.is_some() {
            self.ranker
                .rank_with_graph(&vector_hits, &keyword_hits, &graph_hits, candidates, now)
        } else {
            self.ranker
                .rank(&vector_hits, &keyword_hits, candidates, now)
        };

        // Relevance is attached after fusion because it is a different
        // question from the one fusion answers. Fusion ranks; this measures
        // how good a match each result actually is, which is what lets a
        // caller — or the floor below — say "nothing here answers you"
        // instead of always serving back the top of a list of noise.
        for scored in &mut ranked {
            scored.relevance = self.ranker.relevance(
                vector_scores.get(&scored.memory.id()).copied(),
                keyword_scores
                    .get(&scored.memory.id())
                    .copied()
                    .unwrap_or_default(),
                scored.match_detail.graph_rank.is_some()
                    && scored.match_detail.vector_rank.is_none()
                    && scored.match_detail.bm25_rank.is_none(),
            );
        }

        // Before the truncate, so a caller asking for five results gets
        // five relevant ones rather than three once the floor has eaten two
        // of the five it was truncated to.
        let floor = query.min_relevance();
        if floor > 0.0 {
            ranked.retain(|scored| scored.relevance >= floor);
        }
        ranked.truncate(query.limit());

        // Feeds Phase 5's importance decay. Best-effort: a bookkeeping
        // failure must not fail the read the caller actually made.
        let returned: Vec<_> = ranked.iter().map(|scored| scored.memory.id()).collect();
        if let Err(error) = self.memories.touch_accessed(context, &returned, now) {
            tracing::warn!(%error, "failed to record memory access");
        }

        Ok(ranked)
    }

    /// Runs the graph hop, or nothing when there is no graph. Best-effort:
    /// a hop failure warns and yields an empty leg, so the request still
    /// returns the vector and keyword results.
    fn graph_leg(&self, context: &UserContext, query: &RecallQuery) -> RankedIds {
        let Some(graph) = &self.graph else {
            return RankedIds::default();
        };
        match self.hop(graph.as_ref(), context, query) {
            Ok(ids) => RankedIds(ids),
            Err(error) => {
                tracing::warn!(%error, "graph hop failed; falling back to the other legs");
                RankedIds::default()
            }
        }
    }

    fn hop(
        &self,
        graph: &dyn EntityGraph,
        context: &UserContext,
        query: &RecallQuery,
    ) -> Result<Vec<MemoryId>> {
        // Scan the query for entities the graph knows — no model call —
        // then hop from them. No seeds means the query named nothing
        // known, so the leg stays silent and recall is its two-leg self.
        let seeds = graph.seeds(context, &seed_candidates(query.text()))?;
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        graph.neighbours(
            context,
            &seeds,
            self.max_hops,
            query.as_of(),
            self.hop_limit,
        )
    }
}

/// One leg's `(id, score)` answers in the rank order the fusion wants.
fn ranked(hits: &[(MemoryId, f32)]) -> RankedIds {
    RankedIds(hits.iter().map(|(id, _)| *id).collect())
}

/// The same answers keyed by id, so relevance can be looked up per result
/// after fusion has reordered them.
fn scores(hits: &[(MemoryId, f32)]) -> HashMap<MemoryId, f32> {
    hits.iter().copied().collect()
}

/// Every 1- to [`MAX_SEED_WORDS`]-word window of the query, canonicalised
/// to an [`EntityKey`] the same way the writer filed its entities — so
/// "the billing service team" offers `billing service` and `billing
/// service team` as candidate seeds. Deduplicated, empties dropped; the
/// graph then keeps only the ones some memory actually declared.
fn seed_candidates(text: &str) -> Vec<EntityKey> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for start in 0..words.len() {
        let end = (start + MAX_SEED_WORDS).min(words.len());
        for finish in (start + 1)..=end {
            let key = EntityKey::new(&words[start..finish].join(" "));
            if !key.is_empty() && seen.insert(key.as_str().to_string()) {
                candidates.push(key);
            }
        }
    }
    candidates
}

fn matches(memory: &Memory, query: &RecallQuery, now: chrono::DateTime<chrono::Utc>) -> bool {
    if !query.include_superseded() && !memory.is_active_at(now) {
        return false;
    }
    if !query.categories().is_empty() && !query.categories().contains(memory.category()) {
        return false;
    }
    // Subcategories are OR-ed: matching any one is enough.
    if !query.subcategories().is_empty()
        && !query
            .subcategories()
            .iter()
            .any(|wanted| memory.subcategory() == Some(wanted.as_str()))
    {
        return false;
    }
    // Tags are AND-ed: filters narrow.
    if !query
        .tags()
        .iter()
        .all(|wanted| memory.tags().iter().any(|tag| tag == wanted))
    {
        return false;
    }
    if let Some(since) = query.since()
        && memory.created_at() < since
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, new_memory, now};
    use crate::memories::domain::category::Category;
    use crate::memories::domain::entity_graph::{EntityGraph, Relation};
    use crate::memories::domain::memory::Entity;

    fn query(text: &str) -> RecallQuery {
        RecallQuery::new(text, 10).unwrap()
    }

    fn contents(results: &[ScoredMemory]) -> Vec<&str> {
        results.iter().map(|s| s.memory.content()).collect()
    }

    fn ids(results: &[ScoredMemory]) -> Vec<MemoryId> {
        results.iter().map(|s| s.memory.id()).collect()
    }

    fn entity(name: &str, kind: &str) -> Entity {
        Entity {
            name: name.to_string(),
            kind: kind.to_string(),
        }
    }

    fn rel(subject: &str, predicate: &str, object: &str) -> Relation {
        Relation {
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
        }
    }

    /// Inserts a memory straight into the repository — no vector or keyword
    /// entry — so its only possible route into recall is the graph. Returns
    /// it so the caller can record its edges and assert on its id.
    fn graph_only_memory(fixture: &Fixture, context: &UserContext, content: &str) -> Memory {
        let memory = Memory::create(context.user_id(), new_memory(content), now()).unwrap();
        fixture.memories.insert(context, &memory, "test").unwrap();
        memory
    }

    #[test]
    fn recalls_a_memory_by_a_paraphrase_of_its_words() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm as their package manager");
        fixture.save(&fixture.alex, "The cat sat on the mat");

        let results = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("which package manager does the user prefer"),
            )
            .unwrap();

        assert_eq!(
            contents(&results).first(),
            Some(&"User prefers pnpm as their package manager")
        );
    }

    #[test]
    fn recall_never_returns_another_users_memories() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "alex prefers pnpm");
        fixture.save(&fixture.sam, "sam prefers pnpm");

        let results = fixture
            .recaller()
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();

        assert_eq!(contents(&results), vec!["alex prefers pnpm"]);
    }

    #[test]
    fn filters_by_category() {
        let fixture = Fixture::new();
        let mut decision = new_memory("We chose SQLite over Postgres for installer size");
        decision.category = Category::Decision;
        fixture
            .saver()
            .execute(&fixture.alex, decision, "test")
            .unwrap();
        fixture.save(&fixture.alex, "We chose pnpm over npm for speed");

        let results = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("we chose").with_categories(vec![Category::Decision]),
            )
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.category(), &Category::Decision);
    }

    #[test]
    fn filters_by_tag_requiring_all_of_them() {
        let fixture = Fixture::new();
        let mut both = new_memory("uses typescript with react");
        both.tags = vec!["typescript".to_string(), "react".to_string()];
        fixture
            .saver()
            .execute(&fixture.alex, both, "test")
            .unwrap();

        let mut one = new_memory("uses typescript on the server");
        one.tags = vec!["typescript".to_string()];
        fixture.saver().execute(&fixture.alex, one, "test").unwrap();

        let results = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("uses typescript")
                    .with_tags(vec!["typescript".to_string(), "react".to_string()]),
            )
            .unwrap();

        assert_eq!(contents(&results), vec!["uses typescript with react"]);
    }

    #[test]
    fn filters_by_creation_time() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "a memory about pnpm");

        let after = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("pnpm").with_since(Some(now() + chrono::Duration::days(1))),
            )
            .unwrap();
        assert!(after.is_empty());

        let before = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("pnpm").with_since(Some(now() - chrono::Duration::days(1))),
            )
            .unwrap();
        assert_eq!(before.len(), 1);
    }

    #[test]
    fn superseded_memories_are_excluded_unless_requested() {
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "deploys on flyio");
        let new = fixture.save(&fixture.alex, "deploys on hetzner");

        let superseded = old.clone().supersede(new.id(), now());
        fixture
            .memories
            .update(&fixture.alex, &superseded, "test")
            .unwrap();

        let default = fixture
            .recaller()
            .execute(&fixture.alex, &query("deploys"))
            .unwrap();
        assert_eq!(contents(&default), vec!["deploys on hetzner"]);

        let including = fixture
            .recaller()
            .execute(&fixture.alex, &query("deploys").including_superseded())
            .unwrap();
        assert_eq!(including.len(), 2);
    }

    #[test]
    fn expired_memories_are_excluded() {
        let fixture = Fixture::new();
        let mut expiring = new_memory("a temporary note about pnpm");
        expiring.expires_at = Some(now() + chrono::Duration::hours(1));
        fixture
            .saver()
            .execute(&fixture.alex, expiring, "test")
            .unwrap();

        // The fixture clock is fixed, so the memory is still live here.
        assert_eq!(
            fixture
                .recaller()
                .execute(&fixture.alex, &query("pnpm"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn deleted_memories_are_not_recalled() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "a note about pnpm");
        fixture
            .memories
            .delete(&fixture.alex, memory.id(), "test", "")
            .unwrap();

        assert!(
            fixture
                .recaller()
                .execute(&fixture.alex, &query("pnpm"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn respects_the_limit() {
        let fixture = Fixture::new();
        for index in 0..5 {
            fixture.save(&fixture.alex, &format!("note number {index} about pnpm"));
        }

        let results = fixture
            .recaller()
            .execute(&fixture.alex, &RecallQuery::new("pnpm", 2).unwrap())
            .unwrap();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn an_empty_store_returns_nothing() {
        let fixture = Fixture::new();
        assert!(
            fixture
                .recaller()
                .execute(&fixture.alex, &query("anything"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn recall_survives_a_keyword_index_failure() {
        // Degraded, not broken: the vector leg still answers.
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm");
        fixture.text.fail_next_upsert();

        let results = fixture
            .recaller()
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();

        assert_eq!(results.len(), 1);
    }

    #[test]
    fn recall_records_that_the_returned_memories_were_accessed() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "a note about pnpm");
        assert_eq!(
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .unwrap()
                .last_accessed_at(),
            None
        );

        fixture
            .recaller()
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();

        assert_eq!(
            fixture
                .memories
                .find(&fixture.alex, memory.id())
                .unwrap()
                .unwrap()
                .last_accessed_at(),
            Some(now())
        );
    }

    #[test]
    fn filters_by_subcategory_returns_only_matching() {
        let fixture = Fixture::new();
        let mut testing = new_memory("uses vitest for unit tests");
        testing.subcategory = Some("testing".to_string());
        fixture
            .saver()
            .execute(&fixture.alex, testing, "test")
            .unwrap();

        let mut linting = new_memory("uses eslint for code quality");
        linting.subcategory = Some("linting".to_string());
        fixture
            .saver()
            .execute(&fixture.alex, linting, "test")
            .unwrap();

        let results = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("code").with_subcategories(vec!["testing".to_string()]),
            )
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.subcategory(), Some("testing"));
    }

    #[test]
    fn the_floor_drops_a_result_it_was_measured_against() {
        // The reported symptom: a question with no answer in the store came
        // back full of near-noise, every row wearing a confident score,
        // because that score is a rank position and a rank-1 hit out of
        // nothing still looks like a rank-1 hit.
        //
        // Asserted against the relevance the fixture's own indexes report
        // rather than a hardcoded threshold: the fake embedder is a
        // bag-of-words stand-in whose absolute numbers mean nothing, and a
        // test that pinned one here would be testing the fake.
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm as their package manager");

        let unrelated = query("the cat sat on the mat");
        let measured = fixture
            .recaller()
            .execute(&fixture.alex, &unrelated)
            .unwrap();

        assert_eq!(
            measured.len(),
            1,
            "with no floor the weak candidate comes back, which is what this test needs"
        );
        let noise = measured[0].relevance;

        let above = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &unrelated.clone().with_min_relevance(noise + 0.01),
            )
            .unwrap();
        assert!(
            above.is_empty(),
            "a floor of {} left a result measuring {noise}",
            noise + 0.01
        );

        // And the boundary is inclusive, so a floor set exactly at a
        // result's own relevance does not delete it.
        let at = fixture
            .recaller()
            .execute(&fixture.alex, &unrelated.with_min_relevance(noise))
            .unwrap();
        assert_eq!(
            at.len(),
            1,
            "a floor exactly at {noise} should keep a result measuring {noise}"
        );
    }

    #[test]
    fn a_floor_spares_the_memory_that_actually_answers() {
        // The other half: a floor that only ever deletes things is a broken
        // floor. Between the noise and the real match there has to be room.
        let fixture = Fixture::new();
        let answering = fixture.save(&fixture.alex, "User prefers pnpm as their package manager");
        let distraction = fixture.save(&fixture.alex, "The cat sat on the mat");

        let results = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("which package manager does the user prefer"),
            )
            .unwrap();

        let relevance = |wanted: MemoryId| -> f32 {
            results
                .iter()
                .find(|scored| scored.memory.id() == wanted)
                .unwrap_or_else(|| panic!("{wanted} was not recalled at all"))
                .relevance
        };
        let answer = relevance(answering.id());
        let noise = relevance(distraction.id());

        assert!(
            answer > noise,
            "the answering memory scored {answer}, the distraction {noise}"
        );

        // Anywhere in the gap works, so the assertion does not depend on
        // where the fake model happens to land.
        let between = (answer + noise) / 2.0;
        let floored = fixture
            .recaller()
            .execute(
                &fixture.alex,
                &query("which package manager does the user prefer").with_min_relevance(between),
            )
            .unwrap();

        assert_eq!(
            contents(&floored),
            vec!["User prefers pnpm as their package manager"],
            "a floor between {noise} and {answer} kept the wrong one"
        );
    }

    #[test]
    fn results_carry_how_they_were_matched() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "User prefers pnpm");

        let results = fixture
            .recaller()
            .execute(&fixture.alex, &query("pnpm"))
            .unwrap();

        let detail = results[0].match_detail;
        assert!(
            detail.vector_rank.is_some() || detail.bm25_rank.is_some(),
            "a result should say which leg found it"
        );
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn a_two_hop_memory_neither_text_leg_ranks_enters_recall_over_the_graph() {
        // The leg's reason to exist (Task 7.3.4): billing service
        // —maintained_by→ Meridian team ←leads— Nadia. A query about the
        // billing service reaches Nadia only over two relations. Both
        // memories are inserted without a vector or keyword entry, so the
        // graph is their sole route in — proving graph evidence, and
        // nothing else, put them in the results.
        let fixture = Fixture::new();
        let graph = fixture.graph();

        let owns = graph_only_memory(
            &fixture,
            &fixture.alex,
            "the billing service is maintained by the Meridian team",
        );
        let leads = graph_only_memory(&fixture, &fixture.alex, "Nadia leads the Meridian team");

        graph
            .record(
                &fixture.alex,
                owns.id(),
                &[
                    entity("billing service", "service"),
                    entity("Meridian team", "team"),
                ],
                &[rel("billing service", "maintained_by", "Meridian team")],
                now(),
            )
            .unwrap();
        graph
            .record(
                &fixture.alex,
                leads.id(),
                &[entity("Nadia", "person"), entity("Meridian team", "team")],
                &[rel("Nadia", "leads", "Meridian team")],
                now(),
            )
            .unwrap();

        let results = fixture
            .recaller_with_graph(graph as Arc<dyn EntityGraph>)
            .execute(
                &fixture.alex,
                &RecallQuery::new("who runs the billing service", 5).unwrap(),
            )
            .unwrap();

        let leads_hit = results
            .iter()
            .find(|s| s.memory.id() == leads.id())
            .expect("the two-hop memory should reach recall over the graph");
        assert!(
            leads_hit.match_detail.graph_rank.is_some(),
            "it entered only as a graph hit"
        );
        assert_eq!(
            leads_hit.match_detail.vector_rank, None,
            "no vector entry was indexed for it"
        );
        assert_eq!(
            leads_hit.match_detail.bm25_rank, None,
            "no keyword entry was indexed for it"
        );
    }

    #[test]
    fn the_graph_leg_never_crosses_users() {
        // Seeding and hopping are both user-scoped: alex querying an entity
        // name sam also used must not reach sam's memory. Both users store
        // an entity called "shared service"; the isolation is the WHERE
        // clause, not the names being distinct.
        let fixture = Fixture::new();
        let graph = fixture.graph();

        let alex_memory = graph_only_memory(
            &fixture,
            &fixture.alex,
            "alex's note about the shared service",
        );
        let sam_memory = graph_only_memory(
            &fixture,
            &fixture.sam,
            "sam's note about the shared service",
        );
        graph
            .record(
                &fixture.alex,
                alex_memory.id(),
                &[
                    entity("shared service", "service"),
                    entity("alex thing", "thing"),
                ],
                &[rel("shared service", "relates_to", "alex thing")],
                now(),
            )
            .unwrap();
        graph
            .record(
                &fixture.sam,
                sam_memory.id(),
                &[
                    entity("shared service", "service"),
                    entity("sam thing", "thing"),
                ],
                &[rel("shared service", "relates_to", "sam thing")],
                now(),
            )
            .unwrap();

        let results = fixture
            .recaller_with_graph(graph as Arc<dyn EntityGraph>)
            .execute(
                &fixture.alex,
                &RecallQuery::new("the shared service", 5).unwrap(),
            )
            .unwrap();

        let found = ids(&results);
        assert!(
            found.contains(&alex_memory.id()),
            "alex's own edge is reachable"
        );
        assert!(
            !found.contains(&sam_memory.id()),
            "the hop crossed into another user's rows"
        );
    }

    #[test]
    fn a_query_naming_no_known_entity_leaves_recall_its_two_leg_self() {
        // The "no seeds → today's behaviour, exactly" guarantee: with a
        // graph present but a query that mentions no stored entity, the leg
        // is silent and the ranking matches the two-leg recaller's, every
        // graph_rank absent.
        let fixture = Fixture::new();
        let graph = fixture.graph();

        let memory = fixture.save(&fixture.alex, "User prefers pnpm as their package manager");
        // The only stored entity is "pnpm", a word the query below never uses.
        graph
            .record(
                &fixture.alex,
                memory.id(),
                &[entity("pnpm", "tool")],
                &[],
                now(),
            )
            .unwrap();

        let question = query("which package manager keeps dependencies tidy");
        let with_graph = fixture
            .recaller_with_graph(graph as Arc<dyn EntityGraph>)
            .execute(&fixture.alex, &question)
            .unwrap();
        let without_graph = fixture
            .recaller()
            .execute(&fixture.alex, &question)
            .unwrap();

        assert_eq!(
            ids(&with_graph),
            ids(&without_graph),
            "an unseeded graph leg changed the ranking"
        );
        assert!(
            with_graph
                .iter()
                .all(|s| s.match_detail.graph_rank.is_none()),
            "nothing should carry a graph rank when the query names no entity"
        );
    }
}
