//! Groups memories into clusters from pairwise "these two are similar"
//! links — union-find, and nothing else.
//!
//! # Why transitive grouping rather than pairs
//!
//! Five phrasings of one preference produce up to ten similar pairs, and
//! merging them pair-by-pair would run five merges and leave a chain of
//! supersessions behind. Worse, similarity is not transitive in
//! *practice*: A may pass the threshold against B and C without B and C
//! passing it against each other. Treating the pairs independently would
//! merge A with B, then find A already superseded when C's turn came.
//!
//! Union-find resolves all of that up front: whatever is connected,
//! however indirectly, is one cluster and gets one merge.
//!
//! # The cost of transitivity
//!
//! It is also the risk. A chain of individually-plausible links can drag
//! two unrelated memories into one cluster — the classic single-linkage
//! failure. The defence is the threshold, which is set high
//! (`[consolidation].similarity_threshold`, 0.92 by default) precisely
//! because it is the only thing standing between a chain and a bad merge.
//! The model gets the last word: `MemoryMerger` may look at a cluster and
//! decline.

use crate::shared::ids::MemoryId;
use std::collections::HashMap;

/// Accumulates similarity links, then resolves them into clusters.
#[derive(Debug, Default)]
pub struct ClusterBuilder {
    /// Insertion order, so the output is deterministic — a test that
    /// asserts on cluster contents should not depend on hash iteration.
    order: Vec<MemoryId>,
    index: HashMap<MemoryId, usize>,
    parent: Vec<usize>,
}

impl ClusterBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that two memories are similar enough to consider merging.
    ///
    /// Linking a memory to itself is ignored rather than rejected: a
    /// caller comparing every pair in a list is entitled to hand over the
    /// diagonal, and making that an error would push the check outward to
    /// every caller.
    pub fn link(&mut self, a: MemoryId, b: MemoryId) {
        if a == b {
            return;
        }
        let (a, b) = (self.slot(a), self.slot(b));
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            // Attach the later-seen root under the earlier one, so a
            // cluster's representative is its first-seen member.
            let (earlier, later) = if a < b { (a, b) } else { (b, a) };
            self.parent[later] = earlier;
        }
    }

    /// The clusters, in first-seen order, members likewise.
    ///
    /// Only groups of two or more: a memory similar to nothing is not a
    /// cluster of one, it is just a memory, and returning it would make
    /// every caller filter.
    pub fn clusters(mut self) -> Vec<Vec<MemoryId>> {
        let mut groups: Vec<Vec<MemoryId>> = Vec::new();
        let mut group_of: HashMap<usize, usize> = HashMap::new();

        for position in 0..self.order.len() {
            let root = self.root(position);
            match group_of.get(&root) {
                Some(&group) => groups[group].push(self.order[position]),
                None => {
                    group_of.insert(root, groups.len());
                    groups.push(vec![self.order[position]]);
                }
            }
        }

        groups.retain(|group| group.len() > 1);
        groups
    }

    fn slot(&mut self, id: MemoryId) -> usize {
        *self.index.entry(id).or_insert_with(|| {
            self.order.push(id);
            self.parent.push(self.parent.len());
            self.parent.len() - 1
        })
    }

    /// Find with path compression.
    fn root(&mut self, mut position: usize) -> usize {
        while self.parent[position] != position {
            self.parent[position] = self.parent[self.parent[position]];
            position = self.parent[position];
        }
        position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ids in a fixed order, so a test can talk about "the first one".
    fn ids(count: usize) -> Vec<MemoryId> {
        (0..count).map(|_| MemoryId::new()).collect()
    }

    #[test]
    fn no_links_means_no_clusters() {
        assert!(ClusterBuilder::new().clusters().is_empty());
    }

    #[test]
    fn one_link_makes_one_pair() {
        let id = ids(2);
        let mut builder = ClusterBuilder::new();
        builder.link(id[0], id[1]);

        assert_eq!(builder.clusters(), vec![vec![id[0], id[1]]]);
    }

    #[test]
    fn a_memory_linked_to_nothing_is_not_a_cluster_of_one() {
        let id = ids(3);
        let mut builder = ClusterBuilder::new();
        builder.link(id[0], id[1]);
        // id[2] never appears at all.

        let clusters = builder.clusters();
        assert_eq!(clusters.len(), 1);
        assert!(!clusters[0].contains(&id[2]));
    }

    #[test]
    fn similarity_is_grouped_transitively() {
        // The case the whole type exists for: A~B and B~C without A~C.
        // Handled pairwise this would be two merges, the second of them
        // against a memory the first had already superseded.
        let id = ids(3);
        let mut builder = ClusterBuilder::new();
        builder.link(id[0], id[1]);
        builder.link(id[1], id[2]);

        assert_eq!(builder.clusters(), vec![vec![id[0], id[1], id[2]]]);
    }

    #[test]
    fn a_long_chain_collapses_into_one_cluster() {
        // Five phrasings of one preference, linked in sequence — the
        // DoD scenario for the consolidation job.
        let id = ids(5);
        let mut builder = ClusterBuilder::new();
        for pair in id.windows(2) {
            builder.link(pair[0], pair[1]);
        }

        assert_eq!(builder.clusters(), vec![id.clone()]);
    }

    #[test]
    fn separate_groups_stay_separate() {
        let id = ids(4);
        let mut builder = ClusterBuilder::new();
        builder.link(id[0], id[1]);
        builder.link(id[2], id[3]);

        assert_eq!(
            builder.clusters(),
            vec![vec![id[0], id[1]], vec![id[2], id[3]]]
        );
    }

    #[test]
    fn two_groups_merge_when_a_link_bridges_them() {
        let id = ids(4);
        let mut builder = ClusterBuilder::new();
        builder.link(id[0], id[1]);
        builder.link(id[2], id[3]);
        builder.link(id[1], id[2]);

        assert_eq!(builder.clusters(), vec![id.clone()]);
    }

    #[test]
    fn repeated_and_reversed_links_change_nothing() {
        // Callers comparing every pair produce both (a,b) and (b,a), and
        // an idempotent `link` is what lets them not care.
        let id = ids(2);
        let mut builder = ClusterBuilder::new();
        builder.link(id[0], id[1]);
        builder.link(id[1], id[0]);
        builder.link(id[0], id[1]);

        assert_eq!(builder.clusters(), vec![vec![id[0], id[1]]]);
    }

    #[test]
    fn linking_a_memory_to_itself_is_ignored() {
        let id = ids(1);
        let mut builder = ClusterBuilder::new();
        builder.link(id[0], id[0]);

        assert!(
            builder.clusters().is_empty(),
            "a memory is not similar to itself in any useful sense"
        );
    }

    #[test]
    fn output_order_follows_first_appearance() {
        // Determinism, so the merge order — and therefore which memory a
        // cluster is judged against — does not change between runs.
        let id = ids(4);
        let mut builder = ClusterBuilder::new();
        builder.link(id[2], id[3]);
        builder.link(id[0], id[1]);

        assert_eq!(
            builder.clusters(),
            vec![vec![id[2], id[3]], vec![id[0], id[1]]],
            "clusters should appear in the order their members were first linked"
        );
    }

    #[test]
    fn a_dense_web_is_still_one_cluster() {
        // Every pair linked, which is what a group of near-identical
        // memories actually produces.
        let id = ids(4);
        let mut builder = ClusterBuilder::new();
        for (position, left) in id.iter().enumerate() {
            for right in &id[position + 1..] {
                builder.link(*left, *right);
            }
        }

        assert_eq!(builder.clusters(), vec![id.clone()]);
    }

    #[test]
    fn a_star_shape_is_one_cluster() {
        // One memory similar to four others that are not similar to each
        // other — single-linkage at its most aggressive, and the reason
        // the threshold is set high.
        let id = ids(5);
        let mut builder = ClusterBuilder::new();
        for other in &id[1..] {
            builder.link(id[0], *other);
        }

        assert_eq!(builder.clusters(), vec![id.clone()]);
    }

    #[test]
    fn many_links_stay_linear_rather_than_degenerating() {
        // Path compression is not decoration: without it a chain of
        // links makes `root` walk the whole chain every time, and a
        // category with a few thousand memories turns quadratic.
        let id = ids(2_000);
        let mut builder = ClusterBuilder::new();
        for pair in id.windows(2) {
            builder.link(pair[0], pair[1]);
        }

        let clusters = builder.clusters();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].len(), 2_000);
    }
}
