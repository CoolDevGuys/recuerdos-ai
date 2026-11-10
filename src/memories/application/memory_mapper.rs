//! Builds the map of a user's memories: nodes, similarity and relation
//! edges, and the category summary (web-ui-plan.md §4.1; the arithmetic is
//! in `domain/memory_map.rs`).
//!
//! # Cost, and the cache
//!
//! The similarity edges compare every memory with every other: `n²·d`
//! multiply-adds, ~20 ms for 200 memories of 384 dimensions and a few
//! hundred ms for 1 000 in a release build. They are cached per user,
//! keyed on the store's generation (`MemoryRepository::generation`, which
//! moves on every mutation), so reopening the map is free until something
//! changes. Relation links are a single indexed query and are read fresh:
//! a graph backfill adds them without writing the audit trail.
//!
//! Above [`MAX_SIMILARITY_NODES`] the similarity edges are skipped and the
//! answer says so (`edges_omitted`); the large-store aggregate mode is P4.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::entity_graph::{EntityGraph, MemoryLink};
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_map::{
    CategoryEdge, CategorySummary, SimilarityEdge, category_edges, category_summary, mutual_knn,
};
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{MemoryId, UserId};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Above this many memories the pairwise comparison is skipped.
pub const MAX_SIMILARITY_NODES: usize = 3_000;
/// The most relation links one map carries.
pub const MAX_LINKS: usize = 5_000;
/// The least rescaled similarity an edge needs: a little above "as alike as
/// two unrelated memories". The UI can raise it; it cannot go lower.
pub const MIN_EDGE_WEIGHT: f32 = 0.15;
pub const DEFAULT_K: usize = 5;
pub const MAX_K: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapQuery {
    /// Neighbours considered per memory, `1..=MAX_K`.
    pub k: usize,
    pub include_superseded: bool,
}

impl MapQuery {
    pub fn new(k: Option<usize>, include_superseded: bool) -> Result<Self> {
        let k = k.unwrap_or(DEFAULT_K);
        if !(1..=MAX_K).contains(&k) {
            return Err(RaError::Validation(format!(
                "k must be between 1 and {MAX_K}, got {k}"
            )));
        }
        Ok(Self {
            k,
            include_superseded,
        })
    }
}

#[derive(Debug, Clone)]
pub struct MemoryMap {
    pub generation: i64,
    pub k: usize,
    pub min_weight: f32,
    pub memories: Vec<Memory>,
    pub similar: Arc<Vec<SimilarityEdge>>,
    pub links: Vec<MemoryLink>,
    pub categories: Vec<CategorySummary>,
    pub category_edges: Vec<CategoryEdge>,
    /// The store is too big for the pairwise comparison; no similarity edges.
    pub edges_omitted: bool,
}

struct Cached {
    generation: i64,
    query: MapQuery,
    similar: Arc<Vec<SimilarityEdge>>,
}

pub struct MemoryMapper {
    memories: Arc<dyn MemoryRepository>,
    vectors: Arc<dyn VectorIndex>,
    graph: Option<Arc<dyn EntityGraph>>,
    /// The model's similarity for unrelated text (`[retrieval]` baseline).
    baseline: f32,
    cache: Mutex<HashMap<UserId, Cached>>,
}

impl MemoryMapper {
    pub fn new(
        memories: Arc<dyn MemoryRepository>,
        vectors: Arc<dyn VectorIndex>,
        graph: Option<Arc<dyn EntityGraph>>,
        baseline: f32,
    ) -> Self {
        Self {
            memories,
            vectors,
            graph,
            baseline,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn execute(&self, context: &UserContext, query: MapQuery) -> Result<MemoryMap> {
        // Read the generation first: if a write lands between it and the
        // list, the cached edges are tagged older than they are and get
        // recomputed next time — never the reverse.
        let generation = self.memories.generation(context)?;
        let memories = self.memories.list(context, query.include_superseded)?;
        let present: HashSet<MemoryId> = memories.iter().map(Memory::id).collect();

        let edges_omitted = memories.len() > MAX_SIMILARITY_NODES;
        let similar = if edges_omitted {
            Arc::new(Vec::new())
        } else {
            self.similarity_edges(context, query, generation, &present)?
        };

        let links = match &self.graph {
            Some(graph) => graph
                .memory_links(context, MAX_LINKS)?
                .into_iter()
                .filter(|link| present.contains(&link.from) && present.contains(&link.to))
                .collect(),
            None => Vec::new(),
        };

        Ok(MemoryMap {
            generation,
            k: query.k,
            min_weight: MIN_EDGE_WEIGHT,
            categories: category_summary(&memories),
            category_edges: category_edges(&memories, &similar),
            memories,
            similar,
            links,
            edges_omitted,
        })
    }

    fn similarity_edges(
        &self,
        context: &UserContext,
        query: MapQuery,
        generation: i64,
        present: &HashSet<MemoryId>,
    ) -> Result<Arc<Vec<SimilarityEdge>>> {
        let user = context.user_id();
        if let Some(cached) = self.cache.lock().unwrap().get(&user)
            && cached.generation == generation
            && cached.query == query
        {
            return Ok(Arc::clone(&cached.similar));
        }

        let vectors: Vec<(MemoryId, Vec<f32>)> = self
            .vectors
            .all(context)?
            .into_iter()
            .filter(|(id, _)| present.contains(id))
            .collect();
        let similar = Arc::new(mutual_knn(
            &vectors,
            query.k,
            self.baseline,
            MIN_EDGE_WEIGHT,
        ));

        self.cache.lock().unwrap().insert(
            user,
            Cached {
                generation,
                query,
                similar: Arc::clone(&similar),
            },
        );
        Ok(similar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;

    fn mapper(fixture: &Fixture) -> MemoryMapper {
        fixture.mapper(0.0)
    }

    #[test]
    fn similar_memories_are_linked_and_every_memory_is_a_node() {
        let fixture = Fixture::new();
        let a = fixture.save(&fixture.alex, "deploys on hetzner with docker");
        let b = fixture.save(&fixture.alex, "deploys on hetzner with docker compose");
        fixture.save(&fixture.alex, "prefers dark chocolate");

        let map = mapper(&fixture)
            .execute(&fixture.alex, MapQuery::new(Some(1), false).unwrap())
            .unwrap();

        assert_eq!(map.memories.len(), 3);
        assert!(
            map.similar
                .iter()
                .any(|e| [e.source, e.target].contains(&a.id())
                    && [e.source, e.target].contains(&b.id())),
            "{:?}",
            map.similar
        );
    }

    #[test]
    fn the_map_holds_only_the_callers_memories() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "alex's");
        fixture.save(&fixture.sam, "sam's");

        let map = mapper(&fixture)
            .execute(&fixture.alex, MapQuery::new(None, false).unwrap())
            .unwrap();

        assert_eq!(map.memories.len(), 1);
        assert_eq!(map.memories[0].content(), "alex's");
    }

    #[test]
    fn a_forgotten_memory_leaves_the_map_and_its_edges() {
        let fixture = Fixture::new();
        let a = fixture.save(&fixture.alex, "deploys on hetzner");
        fixture.save(&fixture.alex, "deploys on hetzner too");
        let mapper = mapper(&fixture);
        let query = MapQuery::new(Some(1), false).unwrap();
        assert_eq!(
            mapper.execute(&fixture.alex, query).unwrap().similar.len(),
            1
        );

        fixture
            .forgetter()
            .execute(&fixture.alex, a.id(), "test", "")
            .unwrap();

        let map = mapper.execute(&fixture.alex, query).unwrap();
        assert_eq!(map.memories.len(), 1);
        assert!(
            map.similar.is_empty(),
            "a cached edge to a forgotten memory survived"
        );
    }

    #[test]
    fn an_unchanged_store_reuses_the_computed_edges() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "deploys on hetzner");
        fixture.save(&fixture.alex, "deploys on hetzner too");
        let mapper = mapper(&fixture);
        let query = MapQuery::new(Some(1), false).unwrap();

        let first = mapper.execute(&fixture.alex, query).unwrap();
        let second = mapper.execute(&fixture.alex, query).unwrap();

        assert!(Arc::ptr_eq(&first.similar, &second.similar));
        assert_eq!(first.generation, second.generation);
    }

    #[test]
    fn k_out_of_range_is_a_validation_error() {
        assert!(matches!(
            MapQuery::new(Some(0), false),
            Err(RaError::Validation(_))
        ));
        assert!(matches!(
            MapQuery::new(Some(16), false),
            Err(RaError::Validation(_))
        ));
        assert_eq!(MapQuery::new(None, false).unwrap().k, DEFAULT_K);
    }
}
