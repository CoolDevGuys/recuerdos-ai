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
//! answer says so (`edges_omitted`).
//!
//! # Large stores: the aggregate answer
//!
//! Above the detail limit (`[ui].map_detail_limit`, 5 000 by default) the
//! whole-store map carries no memories at all: only the category summary,
//! and category links from each category's mean vector (`O(n·d)`, not
//! `n²`). The client then asks for one category at a time
//! (`MapQuery::category`), which is an ordinary map of that category's
//! memories with the similarity links among them.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::entity_graph::{EntityGraph, MemoryLink};
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_map::{
    CategoryEdge, CategorySummary, SimilarityEdge, category_edges, category_summary,
    centroid_category_edges, mutual_knn,
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
/// Above this many memories a whole-store map is aggregate, unless
/// `[ui].map_detail_limit` says otherwise.
pub const DEFAULT_DETAIL_LIMIT: usize = 5_000;
/// How many related categories each category links to in an aggregate map.
pub const CATEGORY_NEIGHBOURS: usize = 3;
/// The longest category id a map query accepts (ids are at most 64).
const MAX_CATEGORY_PARAM: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct MapQuery {
    /// Neighbours considered per memory, `1..=MAX_K`.
    pub k: usize,
    pub include_superseded: bool,
    /// Only this category's memories. A category nobody uses is an empty map,
    /// not an error: it may have just been emptied.
    pub category: Option<String>,
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
            category: None,
        })
    }

    pub fn in_category(mut self, category: Option<String>) -> Result<Self> {
        if let Some(id) = &category
            && (id.trim().is_empty() || id.len() > MAX_CATEGORY_PARAM)
        {
            return Err(RaError::Validation(format!(
                "category must be a category id of 1 to {MAX_CATEGORY_PARAM} characters"
            )));
        }
        self.category = category;
        Ok(self)
    }
}

/// What a map answer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapDetail {
    /// Every memory in scope, with its links.
    Memories,
    /// The store is above the detail limit: categories and their links
    /// only. Ask for a category to see its memories.
    Categories,
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
    pub detail: MapDetail,
    /// How many memories are in scope, including any the answer leaves out.
    pub total: usize,
}

/// What a cached computation is for: the user and the category scope.
type CacheKey = (UserId, Option<String>);

struct Cached {
    generation: i64,
    query: MapQuery,
    similar: Arc<Vec<SimilarityEdge>>,
    /// Centroid links between categories, for an aggregate answer.
    category_edges: Option<Arc<Vec<CategoryEdge>>>,
}

pub struct MemoryMapper {
    memories: Arc<dyn MemoryRepository>,
    vectors: Arc<dyn VectorIndex>,
    graph: Option<Arc<dyn EntityGraph>>,
    /// The model's similarity for unrelated text (`[retrieval]` baseline).
    baseline: f32,
    /// Above this many memories a whole-store map is aggregate.
    detail_limit: usize,
    cache: Mutex<HashMap<CacheKey, Cached>>,
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
            detail_limit: DEFAULT_DETAIL_LIMIT,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Set the size above which a whole-store map is aggregate.
    pub fn with_detail_limit(mut self, limit: usize) -> Self {
        self.detail_limit = limit.max(1);
        self
    }

    pub fn execute(&self, context: &UserContext, query: MapQuery) -> Result<MemoryMap> {
        // Read the generation first: if a write lands between it and the
        // list, the cached edges are tagged older than they are and get
        // recomputed next time — never the reverse.
        let generation = self.memories.generation(context)?;
        let mut memories = self.memories.list(context, query.include_superseded)?;
        if let Some(category) = &query.category {
            memories.retain(|m| m.category().as_str() == category);
        }
        let total = memories.len();

        if query.category.is_none() && total > self.detail_limit {
            return self.aggregate(context, query, generation, memories);
        }

        let present: HashSet<MemoryId> = memories.iter().map(Memory::id).collect();
        let edges_omitted = memories.len() > MAX_SIMILARITY_NODES;
        let similar = if edges_omitted {
            Arc::new(Vec::new())
        } else {
            self.similarity_edges(context, &query, generation, &present)?
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
            detail: MapDetail::Memories,
            total,
        })
    }

    /// The categories-only answer for a store above the detail limit.
    fn aggregate(
        &self,
        context: &UserContext,
        query: MapQuery,
        generation: i64,
        memories: Vec<Memory>,
    ) -> Result<MemoryMap> {
        let key = (context.user_id(), None);
        let cached = self.cache.lock().unwrap().get(&key).and_then(|cached| {
            (cached.generation == generation && cached.query == query)
                .then(|| cached.category_edges.clone())
                .flatten()
        });
        let category_edges = match cached {
            Some(edges) => edges,
            None => {
                let vectors: HashMap<MemoryId, Vec<f32>> =
                    self.vectors.all(context)?.into_iter().collect();
                let edges = Arc::new(centroid_category_edges(
                    &memories,
                    &vectors,
                    CATEGORY_NEIGHBOURS,
                    self.baseline,
                ));
                self.remember(
                    key,
                    Cached {
                        generation,
                        query: query.clone(),
                        similar: Arc::new(Vec::new()),
                        category_edges: Some(Arc::clone(&edges)),
                    },
                );
                edges
            }
        };

        Ok(MemoryMap {
            generation,
            k: query.k,
            min_weight: MIN_EDGE_WEIGHT,
            categories: category_summary(&memories),
            category_edges: category_edges.as_ref().clone(),
            total: memories.len(),
            memories: Vec::new(),
            similar: Arc::new(Vec::new()),
            links: Vec::new(),
            edges_omitted: false,
            detail: MapDetail::Categories,
        })
    }

    /// Store a computation, dropping the user's entries for older
    /// generations: after a write they can never be served again.
    fn remember(&self, key: CacheKey, cached: Cached) {
        let mut cache = self.cache.lock().unwrap();
        cache.retain(|(user, _), entry| *user != key.0 || entry.generation >= cached.generation);
        cache.insert(key, cached);
    }

    fn similarity_edges(
        &self,
        context: &UserContext,
        query: &MapQuery,
        generation: i64,
        present: &HashSet<MemoryId>,
    ) -> Result<Arc<Vec<SimilarityEdge>>> {
        let key = (context.user_id(), query.category.clone());
        if let Some(cached) = self.cache.lock().unwrap().get(&key)
            && cached.generation == generation
            && cached.query == *query
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

        self.remember(
            key,
            Cached {
                generation,
                query: query.clone(),
                similar: Arc::clone(&similar),
                category_edges: None,
            },
        );
        Ok(similar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, new_memory};
    use crate::memories::domain::category::Category;

    fn save_in(fixture: &Fixture, category: Category, content: &str) -> Memory {
        let mut memory = new_memory(content);
        memory.category = category;
        fixture
            .saver()
            .execute(&fixture.alex, memory, "test")
            .unwrap()
    }

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
            mapper
                .execute(&fixture.alex, query.clone())
                .unwrap()
                .similar
                .len(),
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

        let first = mapper.execute(&fixture.alex, query.clone()).unwrap();
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

    #[test]
    fn above_the_detail_limit_the_map_is_categories_only() {
        let fixture = Fixture::new();
        save_in(&fixture, Category::Skill, "writes rust daily");
        save_in(&fixture, Category::Skill, "writes rust and go");
        save_in(&fixture, Category::Decision, "chose sqlite for storage");
        let mapper = mapper(&fixture).with_detail_limit(2);

        let map = mapper
            .execute(&fixture.alex, MapQuery::new(None, false).unwrap())
            .unwrap();

        assert_eq!(map.detail, MapDetail::Categories);
        assert!(map.memories.is_empty() && map.similar.is_empty());
        assert_eq!(map.total, 3);
        let counts: Vec<(&str, usize)> = map
            .categories
            .iter()
            .map(|c| (c.id.as_str(), c.count))
            .collect();
        assert_eq!(counts, vec![("skill", 2), ("decision", 1)]);
        assert_eq!(map.category_edges.len(), 1, "two categories, one link");
    }

    #[test]
    fn a_category_query_maps_only_that_category_whatever_the_size() {
        let fixture = Fixture::new();
        let a = save_in(&fixture, Category::Skill, "deploys on hetzner");
        let b = save_in(&fixture, Category::Skill, "deploys on hetzner too");
        save_in(&fixture, Category::Decision, "deploys on hetzner, decided");
        let mapper = mapper(&fixture).with_detail_limit(1);
        let query = MapQuery::new(Some(1), false)
            .unwrap()
            .in_category(Some("skill".to_string()))
            .unwrap();

        let map = mapper.execute(&fixture.alex, query).unwrap();

        assert_eq!(map.detail, MapDetail::Memories);
        assert_eq!(map.total, 2);
        let ids: HashSet<MemoryId> = map.memories.iter().map(Memory::id).collect();
        assert_eq!(ids, [a.id(), b.id()].into_iter().collect());
        assert_eq!(map.similar.len(), 1);
        assert_eq!(map.categories.len(), 1);
    }

    #[test]
    fn whole_store_and_category_maps_are_cached_side_by_side() {
        let fixture = Fixture::new();
        save_in(&fixture, Category::Skill, "deploys on hetzner");
        save_in(&fixture, Category::Skill, "deploys on hetzner too");
        let mapper = mapper(&fixture);
        let whole = MapQuery::new(Some(1), false).unwrap();
        let skill = whole.clone().in_category(Some("skill".into())).unwrap();

        let first = mapper.execute(&fixture.alex, whole.clone()).unwrap();
        mapper.execute(&fixture.alex, skill.clone()).unwrap();
        let again = mapper.execute(&fixture.alex, whole).unwrap();

        assert!(Arc::ptr_eq(&first.similar, &again.similar));
    }

    #[test]
    fn a_blank_category_is_a_validation_error() {
        let query = MapQuery::new(None, false).unwrap();
        assert!(matches!(
            query.clone().in_category(Some(" ".into())),
            Err(RaError::Validation(_))
        ));
        assert!(matches!(
            query.in_category(Some("x".repeat(200))),
            Err(RaError::Validation(_))
        ));
    }
}
