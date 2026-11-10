//! The shape of a user's memories as a graph — what the web UI's mental map
//! draws (ai-specs/web-ui-plan.md §4.1).
//!
//! Pure functions over vectors and memories; the use case
//! (`memory_mapper.rs`) loads the inputs and caches the result.
//!
//! # Why *mutual* nearest neighbours
//!
//! The obvious graph — link each memory to its k most similar — is pulled
//! into a star by hub memories. Recall shows them: a short, generically
//! phrased preference ("The user prefers pnpm") sits near the top of most
//! "the user prefers…" questions, so it is in almost everyone's top k.
//! Drawn as plain kNN it is wired to half the store and the layout collapses
//! around it. Keeping an edge only when *each* memory is in the other's top k
//! drops those one-sided links: a hub keeps only the neighbours that also
//! chose it.
//!
//! # Why a floor, and why it comes from the model
//!
//! Every pair of memories has *some* cosine similarity, and for a given
//! model unrelated text does not sit at 0 — bge-small puts it near 0.5. The
//! floor is that model's `similarity_baseline` (the same number recall's
//! relevance calibration uses) plus a margin, and an edge's `weight` is its
//! similarity rescaled above the baseline, `0..=1`. So an edge means "more
//! alike than two unrelated memories are", whatever the model.

use crate::memories::domain::category::Category;
use crate::memories::domain::memory::Memory;
use crate::shared::ids::MemoryId;
use std::collections::{BTreeMap, HashMap};

/// A similarity link between two memories. `source < target` by id order,
/// so each pair appears once.
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarityEdge {
    pub source: MemoryId,
    pub target: MemoryId,
    /// Similarity rescaled above the baseline: `0.0` at the baseline, `1.0`
    /// for identical vectors.
    pub weight: f32,
}

/// Mutual k-nearest-neighbour edges over `vectors`, keeping a pair only when
/// its rescaled weight is at least `min_weight`.
///
/// `baseline` is the model's similarity for unrelated text. Vectors need not
/// be normalised; zero vectors link to nothing.
pub fn mutual_knn(
    vectors: &[(MemoryId, Vec<f32>)],
    k: usize,
    baseline: f32,
    min_weight: f32,
) -> Vec<SimilarityEdge> {
    let n = vectors.len();
    if n < 2 || k == 0 {
        return Vec::new();
    }

    let normalised: Vec<Option<Vec<f32>>> = vectors
        .iter()
        .map(|(_, vector)| {
            let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            (norm > 0.0).then(|| vector.iter().map(|x| x / norm).collect())
        })
        .collect();

    // Each memory's top-k neighbours by cosine, as (index, cosine).
    let mut top: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
    let mut row: Vec<(usize, f32)> = Vec::with_capacity(n);
    for i in 0..n {
        let Some(a) = &normalised[i] else { continue };
        row.clear();
        for (j, other) in normalised.iter().enumerate() {
            if i == j {
                continue;
            }
            if let Some(b) = other {
                row.push((j, a.iter().zip(b).map(|(x, y)| x * y).sum()));
            }
        }
        // Ties broken by index so the graph is deterministic.
        row.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        top[i] = row.iter().take(k).copied().collect();
    }

    let span = (1.0 - baseline).max(f32::EPSILON);
    let mut edges = Vec::new();
    for (i, neighbours) in top.iter().enumerate() {
        for &(j, cosine) in neighbours {
            // Each mutual pair is seen twice; keep the visit from the lower index.
            if j < i || !top[j].iter().any(|&(back, _)| back == i) {
                continue;
            }
            let weight = ((cosine - baseline) / span).clamp(0.0, 1.0);
            if weight < min_weight {
                continue;
            }
            let (a, b) = (vectors[i].0, vectors[j].0);
            let (source, target) = if a.to_string() <= b.to_string() {
                (a, b)
            } else {
                (b, a)
            };
            edges.push(SimilarityEdge {
                source,
                target,
                weight,
            });
        }
    }
    edges.sort_by(|x, y| {
        (x.source.to_string(), x.target.to_string())
            .cmp(&(y.source.to_string(), y.target.to_string()))
    });
    edges
}

/// One category present in the map, with its sub-labels.
#[derive(Debug, Clone, PartialEq)]
pub struct CategorySummary {
    pub id: String,
    pub label: String,
    pub built_in: bool,
    pub count: usize,
    /// Sub-label → count, alphabetical. Memories without one are not listed.
    pub subcategories: Vec<(String, usize)>,
}

/// The categories the memories actually use, largest first (ties by id).
pub fn category_summary(memories: &[Memory]) -> Vec<CategorySummary> {
    let mut by_category: BTreeMap<String, (Category, usize, BTreeMap<String, usize>)> =
        BTreeMap::new();
    for memory in memories {
        let entry = by_category
            .entry(memory.category().as_str().to_string())
            .or_insert_with(|| (memory.category().clone(), 0, BTreeMap::new()));
        entry.1 += 1;
        if let Some(sub) = memory.subcategory() {
            *entry.2.entry(sub.to_string()).or_default() += 1;
        }
    }

    let mut summary: Vec<CategorySummary> = by_category
        .into_iter()
        .map(|(id, (category, count, subs))| CategorySummary {
            label: category.label(),
            built_in: !matches!(category, Category::Custom(_)),
            id,
            count,
            subcategories: subs.into_iter().collect(),
        })
        .collect();
    summary.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.id.cmp(&b.id)));
    summary
}

/// How strongly two categories are related: the summed weight of the
/// similarity edges crossing between them, normalised by `√(|A|·|B|)` so a
/// big category is not related to everything merely by being big.
#[derive(Debug, Clone, PartialEq)]
pub struct CategoryEdge {
    pub source: String,
    pub target: String,
    pub weight: f32,
}

pub fn category_edges(memories: &[Memory], edges: &[SimilarityEdge]) -> Vec<CategoryEdge> {
    let category_of: HashMap<MemoryId, &str> = memories
        .iter()
        .map(|m| (m.id(), m.category().as_str()))
        .collect();
    let mut sizes: HashMap<&str, usize> = HashMap::new();
    for memory in memories {
        *sizes.entry(memory.category().as_str()).or_default() += 1;
    }

    let mut sums: BTreeMap<(&str, &str), f32> = BTreeMap::new();
    for edge in edges {
        let (Some(a), Some(b)) = (category_of.get(&edge.source), category_of.get(&edge.target))
        else {
            continue;
        };
        if a == b {
            continue;
        }
        let key = if a < b { (*a, *b) } else { (*b, *a) };
        *sums.entry(key).or_default() += edge.weight;
    }

    sums.into_iter()
        .map(|((a, b), sum)| {
            let norm = ((sizes[a] * sizes[b]) as f32).sqrt().max(1.0);
            CategoryEdge {
                source: a.to_string(),
                target: b.to_string(),
                weight: sum / norm,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::domain::memory::{MemorySource, NewMemory};
    use crate::shared::ids::UserId;
    use chrono::Utc;

    fn ids(n: usize) -> Vec<MemoryId> {
        let mut ids: Vec<MemoryId> = (0..n).map(|_| MemoryId::new()).collect();
        ids.sort_by_key(|id| id.to_string());
        ids
    }

    fn pairs(edges: &[SimilarityEdge], ids: &[MemoryId]) -> Vec<(usize, usize)> {
        let index = |id: &MemoryId| ids.iter().position(|x| x == id).unwrap();
        let mut pairs: Vec<(usize, usize)> = edges
            .iter()
            .map(|e| {
                let (a, b) = (index(&e.source), index(&e.target));
                (a.min(b), a.max(b))
            })
            .collect();
        pairs.sort();
        pairs
    }

    #[test]
    fn two_close_memories_link_and_a_far_one_does_not() {
        let ids = ids(3);
        let vectors = vec![
            (ids[0], vec![1.0, 0.0]),
            (ids[1], vec![0.98, 0.2]),
            (ids[2], vec![0.0, 1.0]),
        ];

        let edges = mutual_knn(&vectors, 1, 0.5, 0.0);

        assert_eq!(pairs(&edges, &ids), vec![(0, 1)]);
    }

    #[test]
    fn a_hub_keeps_only_the_neighbours_that_chose_it_back() {
        // 0 is a hub: close to everyone. 1–2 and 3–4 are tight pairs.
        let ids = ids(5);
        let vectors = vec![
            (ids[0], vec![1.0, 1.0, 1.0, 1.0]),
            (ids[1], vec![1.0, 0.0, 0.0, 0.0]),
            (ids[2], vec![0.95, 0.3, 0.0, 0.0]),
            (ids[3], vec![0.0, 0.0, 1.0, 0.0]),
            (ids[4], vec![0.0, 0.0, 0.95, 0.3]),
        ];

        let edges = mutual_knn(&vectors, 1, 0.0, 0.0);

        // Plain 1-NN would also link 0 to whichever pair member it prefers;
        // mutual keeps only the pairs.
        assert_eq!(pairs(&edges, &ids), vec![(1, 2), (3, 4)]);
    }

    #[test]
    fn weights_are_rescaled_above_the_baseline_and_floored() {
        let ids = ids(2);
        // cosine 0.8
        let vectors = vec![(ids[0], vec![1.0, 0.0]), (ids[1], vec![0.8, 0.6])];

        let edges = mutual_knn(&vectors, 1, 0.5, 0.0);
        assert_eq!(edges.len(), 1);
        assert!((edges[0].weight - 0.6).abs() < 1e-5, "{}", edges[0].weight);

        assert!(mutual_knn(&vectors, 1, 0.5, 0.7).is_empty());
        // Below the baseline the weight is 0, and a positive floor drops it.
        assert!(mutual_knn(&vectors, 1, 0.9, 0.01).is_empty());
    }

    #[test]
    fn each_pair_appears_once_in_a_stable_order() {
        let ids = ids(4);
        let vectors: Vec<(MemoryId, Vec<f32>)> =
            ids.iter().map(|id| (*id, vec![1.0, 0.1])).collect();

        let first = mutual_knn(&vectors, 3, 0.0, 0.0);
        let second = mutual_knn(&vectors, 3, 0.0, 0.0);

        assert_eq!(first.len(), 6, "all pairs of four");
        assert_eq!(first, second);
        assert!(
            first
                .iter()
                .all(|e| e.source.to_string() < e.target.to_string())
        );
    }

    #[test]
    fn degenerate_inputs_link_nothing() {
        let ids = ids(2);
        assert!(mutual_knn(&[], 5, 0.5, 0.0).is_empty());
        assert!(mutual_knn(&[(ids[0], vec![1.0])], 5, 0.5, 0.0).is_empty());
        assert!(
            mutual_knn(
                &[(ids[0], vec![0.0, 0.0]), (ids[1], vec![1.0, 0.0])],
                1,
                0.0,
                0.0
            )
            .is_empty()
        );
        assert!(mutual_knn(&[(ids[0], vec![1.0]), (ids[1], vec![1.0])], 0, 0.0, 0.0).is_empty());
    }

    fn memory(category: Category, subcategory: Option<&str>) -> Memory {
        Memory::create(
            UserId::new(),
            NewMemory {
                content: "x".to_string(),
                category,
                subcategory: subcategory.map(str::to_string),
                tags: vec![],
                entities: vec![],
                confidence: 1.0,
                source: MemorySource::default(),
                expires_at: None,
            },
            Utc::now(),
        )
        .unwrap()
    }

    #[test]
    fn the_summary_counts_categories_and_their_sub_labels() {
        let memories = vec![
            memory(Category::PreferenceCoding, Some("testing")),
            memory(Category::PreferenceCoding, Some("testing")),
            memory(Category::PreferenceCoding, None),
            memory(Category::Custom("health.sleep".to_string()), None),
        ];

        let summary = category_summary(&memories);

        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0].id, "preference.coding");
        assert_eq!(summary[0].label, "Coding preference");
        assert_eq!(summary[0].count, 3);
        assert_eq!(summary[0].subcategories, vec![("testing".to_string(), 2)]);
        assert!(summary[0].built_in);
        assert_eq!(summary[1].label, "Health sleep");
        assert!(!summary[1].built_in);
    }

    #[test]
    fn category_edges_sum_cross_links_normalised_by_size() {
        let memories = vec![
            memory(Category::PreferenceCoding, None),
            memory(Category::PreferenceCoding, None),
            memory(Category::Decision, None),
        ];
        let edge = |a: usize, b: usize, weight: f32| SimilarityEdge {
            source: memories[a].id(),
            target: memories[b].id(),
            weight,
        };
        let edges = vec![edge(0, 1, 0.9), edge(0, 2, 0.4), edge(1, 2, 0.4)];

        let categories = category_edges(&memories, &edges);

        assert_eq!(
            categories.len(),
            1,
            "the within-category edge is not a category edge"
        );
        assert_eq!(categories[0].source, "decision");
        assert_eq!(categories[0].target, "preference.coding");
        assert!((categories[0].weight - 0.8 / 2f32.sqrt()).abs() < 1e-5);
    }
}
