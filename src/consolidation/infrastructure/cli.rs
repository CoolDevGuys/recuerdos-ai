//! `recuerdos-ai consolidate` — run the nightly job now, by hand.
//!
//! Exists for two reasons beyond impatience. An operator who has just
//! turned consolidation on wants to see what it would do before trusting
//! it to a timer, which is what `--dry-run` is for. And a memory store
//! that has accumulated duplicates for months should not have to wait
//! until tomorrow to be tidied.

use crate::consolidation::application::consolidation_runner::ConsolidationRunner;
use crate::consolidation::domain::consolidation_run::ConsolidationReport;
use crate::shared::error::Result;
use std::sync::Arc;

/// Renders a finished run for a terminal.
pub fn render(report: &ConsolidationReport) -> String {
    let mut output = report.summary();

    if !report.previews.is_empty() {
        output.push_str("\n\nwould merge:\n");
        for (position, preview) in report.previews.iter().enumerate() {
            output.push_str(&format!("\n{}. [{}]\n", position + 1, preview.category));
            for content in &preview.contents {
                output.push_str(&format!("   - {content}\n"));
            }
        }
        output.push_str("\nRe-run without --dry-run to apply.");
    }

    output
}

pub async fn run(runner: Arc<ConsolidationRunner>, dry_run: bool) -> Result<()> {
    let report = runner.execute(dry_run).await?;
    println!("{}", render(&report));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consolidation::domain::consolidation_run::ClusterPreview;

    #[test]
    fn a_dry_run_prints_what_it_would_merge_and_how_to_apply_it() {
        let report = ConsolidationReport {
            dry_run: true,
            users: 1,
            memories_examined: 12,
            clusters_found: 1,
            previews: vec![ClusterPreview {
                category: "preference.coding".to_string(),
                contents: vec!["Prefers pnpm".to_string(), "User uses pnpm".to_string()],
            }],
            ..ConsolidationReport::default()
        };

        let output = render(&report);

        assert!(output.contains("nothing was changed"), "{output}");
        assert!(output.contains("Prefers pnpm"), "{output}");
        assert!(output.contains("preference.coding"), "{output}");
        assert!(
            output.contains("Re-run without --dry-run"),
            "a preview has to say how to act on it: {output}"
        );
    }

    #[test]
    fn a_real_run_prints_only_the_summary() {
        let report = ConsolidationReport {
            users: 1,
            clusters_found: 2,
            merged: 2,
            retired: 6,
            ..ConsolidationReport::default()
        };

        let output = render(&report);

        assert!(output.contains("6 memories retired"), "{output}");
        assert!(!output.contains("would merge"), "{output}");
    }

    #[test]
    fn a_quiet_run_still_says_it_ran() {
        // Printing nothing would leave an operator unsure whether the
        // command did anything at all.
        let output = render(&ConsolidationReport {
            users: 1,
            ..ConsolidationReport::default()
        });

        assert!(output.contains("consolidated 1 user"), "{output}");
    }
}
