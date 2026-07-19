//! Exports a user's memories as markdown or JSON.
//!
//! This is the trust feature (project-plan.md §7.6): your memories are
//! yours, readable, greppable and portable. A memory service you cannot
//! walk away from is a memory service you have to take on faith. Markdown
//! also means a git-versioned backup is a `>` away.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::shared::error::Result;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
}

pub struct MemoryExporter {
    memories: Arc<dyn MemoryRepository>,
}

impl MemoryExporter {
    pub fn new(memories: Arc<dyn MemoryRepository>) -> Self {
        Self { memories }
    }

    pub fn execute(
        &self,
        context: &UserContext,
        format: ExportFormat,
        include_inactive: bool,
    ) -> Result<String> {
        let memories = self.memories.list(context, include_inactive)?;

        Ok(match format {
            ExportFormat::Markdown => render_markdown(context, &memories),
            ExportFormat::Json => render_json(&memories),
        })
    }
}

fn render_markdown(context: &UserContext, memories: &[Memory]) -> String {
    let mut output = format!("# RecordAgent memories: {}\n\n", context.handle());
    output.push_str(&format!("{} memories\n", memories.len()));

    // Grouped by category so the file reads like a profile rather than a
    // log — the categories are the point of having a taxonomy.
    let mut by_category: BTreeMap<&str, Vec<&Memory>> = BTreeMap::new();
    for memory in memories {
        by_category
            .entry(memory.category().as_str())
            .or_default()
            .push(memory);
    }

    for (category, group) in by_category {
        output.push_str(&format!("\n## {category}\n\n"));
        for memory in group {
            output.push_str(&format!("- {}", memory.content().replace('\n', " ")));

            let mut annotations = vec![format!("{}", memory.created_at().format("%Y-%m-%d"))];
            if !memory.tags().is_empty() {
                annotations.push(memory.tags().join(", "));
            }
            if memory.is_superseded() {
                annotations.push("superseded".to_string());
            }
            output.push_str(&format!("  _({})_\n", annotations.join(" · ")));
        }
    }

    output
}

fn render_json(memories: &[Memory]) -> String {
    let items: Vec<serde_json::Value> = memories
        .iter()
        .map(|memory| {
            serde_json::json!({
                "id": memory.id().to_string(),
                "content": memory.content(),
                "category": memory.category().as_str(),
                "tags": memory.tags(),
                "confidence": memory.confidence(),
                "created_at": memory.created_at().to_rfc3339(),
                "updated_at": memory.updated_at().to_rfc3339(),
                "superseded_by": memory.superseded_by().map(|id| id.to_string()),
            })
        })
        .collect();

    serde_json::to_string_pretty(&serde_json::json!({ "memories": items }))
        .unwrap_or_else(|_| "{\"memories\":[]}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, new_memory};
    use crate::memories::domain::category::Category;

    #[test]
    fn markdown_groups_memories_by_category() {
        let fixture = Fixture::new();
        let mut decision = new_memory("We chose SQLite over Postgres");
        decision.category = Category::Decision;
        fixture
            .saver()
            .execute(&fixture.alex, decision, "test")
            .unwrap();
        fixture.save(&fixture.alex, "User prefers pnpm");

        let markdown = fixture
            .exporter()
            .execute(&fixture.alex, ExportFormat::Markdown, false)
            .unwrap();

        assert!(markdown.contains("## decision"), "{markdown}");
        assert!(markdown.contains("## preference.coding"), "{markdown}");
        assert!(markdown.contains("We chose SQLite over Postgres"));
        assert!(markdown.contains("User prefers pnpm"));
    }

    #[test]
    fn markdown_is_greppable_one_memory_per_line() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "a memory\nspanning two lines");

        let markdown = fixture
            .exporter()
            .execute(&fixture.alex, ExportFormat::Markdown, false)
            .unwrap();

        assert!(
            markdown.contains("- a memory spanning two lines"),
            "newlines inside content would break line-based tools: {markdown}"
        );
    }

    #[test]
    fn json_export_round_trips_through_a_parser() {
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "User prefers pnpm");

        let json = fixture
            .exporter()
            .execute(&fixture.alex, ExportFormat::Json, false)
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let items = parsed["memories"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], memory.id().to_string());
        assert_eq!(items[0]["content"], "User prefers pnpm");
        assert_eq!(items[0]["category"], "preference.coding");
    }

    #[test]
    fn export_contains_only_the_callers_memories() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "alex's private memory");
        fixture.save(&fixture.sam, "sam's private memory");

        let markdown = fixture
            .exporter()
            .execute(&fixture.alex, ExportFormat::Markdown, false)
            .unwrap();

        assert!(markdown.contains("alex's private memory"));
        assert!(
            !markdown.contains("sam's private memory"),
            "export leaked another user's memories"
        );
    }

    #[test]
    fn superseded_memories_are_excluded_unless_requested() {
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "deploys on flyio");
        let new = fixture.save(&fixture.alex, "deploys on hetzner");
        fixture
            .memories
            .update(
                &fixture.alex,
                &old.clone()
                    .supersede(new.id(), crate::memories::application::test_doubles::now()),
                "test",
            )
            .unwrap();

        let active = fixture
            .exporter()
            .execute(&fixture.alex, ExportFormat::Markdown, false)
            .unwrap();
        assert!(!active.contains("flyio"));

        let everything = fixture
            .exporter()
            .execute(&fixture.alex, ExportFormat::Markdown, true)
            .unwrap();
        assert!(everything.contains("flyio"));
        assert!(everything.contains("superseded"));
    }

    #[test]
    fn exporting_nothing_is_valid_output_not_an_error() {
        let fixture = Fixture::new();

        let markdown = fixture
            .exporter()
            .execute(&fixture.alex, ExportFormat::Markdown, false)
            .unwrap();
        assert!(markdown.contains("0 memories"));

        let json = fixture
            .exporter()
            .execute(&fixture.alex, ExportFormat::Json, false)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["memories"].as_array().unwrap().is_empty());
    }
}
