//! What one consolidation pass did — the value the CLI prints, the
//! scheduler logs, and tests assert on.
//!
//! A report rather than a return code because the interesting question
//! after a nightly run is not "did it work" but "what did it change to my
//! memories while I was asleep".

/// One cluster as a dry run describes it. Contents rather than ids: this
/// is read by a person deciding whether to let the run proceed, and a
/// list of UUIDs tells them nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterPreview {
    pub category: String,
    pub contents: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsolidationReport {
    /// True when nothing was written. A dry run stops before the model
    /// call, so it also costs nothing.
    pub dry_run: bool,
    pub users: usize,
    /// Active memories the run looked at, across all users.
    pub memories_examined: usize,
    pub clusters_found: usize,
    /// Clusters the model agreed to merge.
    pub merged: usize,
    /// Memories superseded into a merged memory.
    pub retired: usize,
    /// Clusters the model looked at and declined.
    pub kept_separate: usize,
    /// Populated only on a dry run.
    pub previews: Vec<ClusterPreview>,
}

impl ConsolidationReport {
    /// One line for a log or a terminal.
    pub fn summary(&self) -> String {
        if self.dry_run {
            return format!(
                "dry run: {} cluster(s) of duplicates across {} memories from {} user(s); \
                 nothing was changed",
                self.clusters_found, self.memories_examined, self.users
            );
        }

        format!(
            "consolidated {} user(s): {} cluster(s) found, {} merged ({} memories retired), \
             {} left alone",
            self.users, self.clusters_found, self.merged, self.retired, self.kept_separate
        )
    }

    pub fn absorb(&mut self, other: ConsolidationReport) {
        self.memories_examined += other.memories_examined;
        self.clusters_found += other.clusters_found;
        self.merged += other.merged;
        self.retired += other.retired;
        self.kept_separate += other.kept_separate;
        self.previews.extend(other.previews);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dry_run_summary_says_nothing_changed() {
        // The reassurance an operator is looking for when they passed
        // --dry-run precisely because they did not trust it yet.
        let report = ConsolidationReport {
            dry_run: true,
            users: 1,
            memories_examined: 40,
            clusters_found: 3,
            ..ConsolidationReport::default()
        };

        let summary = report.summary();
        assert!(summary.contains("nothing was changed"), "{summary}");
        assert!(summary.contains("3 cluster"), "{summary}");
    }

    #[test]
    fn a_real_summary_reports_what_changed() {
        let report = ConsolidationReport {
            users: 2,
            clusters_found: 3,
            merged: 2,
            retired: 7,
            kept_separate: 1,
            ..ConsolidationReport::default()
        };

        let summary = report.summary();
        assert!(summary.contains("2 merged"), "{summary}");
        assert!(summary.contains("7 memories retired"), "{summary}");
        assert!(summary.contains("1 left alone"), "{summary}");
    }

    #[test]
    fn absorbing_accumulates_counts_but_not_the_user_total() {
        // Users are counted by the caller iterating them; absorbing a
        // per-user report must not double-count them.
        let mut total = ConsolidationReport {
            users: 1,
            merged: 1,
            retired: 2,
            ..ConsolidationReport::default()
        };
        total.absorb(ConsolidationReport {
            users: 1,
            merged: 2,
            retired: 3,
            memories_examined: 10,
            ..ConsolidationReport::default()
        });

        assert_eq!(total.users, 1);
        assert_eq!(total.merged, 3);
        assert_eq!(total.retired, 5);
        assert_eq!(total.memories_examined, 10);
    }
}
