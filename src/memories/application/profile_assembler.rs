//! Builds the `memory://profile` digest — "who is this user?" in one read.
//!
//! # Why a profile at all
//!
//! Recall answers a question. But an agent starting a session hasn't asked
//! anything yet, and the most valuable memories are exactly the ones it
//! doesn't know to ask for: that you forbid barrel files, that you deploy
//! on Hetzner. The profile is the cheapest way to be useful on turn one —
//! one resource read instead of a dozen speculative searches.
//!
//! # Why it is assembled, not generated
//!
//! Phase 5 replaces the internals with an LLM-written digest. This version
//! is deterministic assembly: highest-value memories per category,
//! grouped, truncated to a token budget. That means the resource contract
//! exists now, agents can depend on it now, and it costs no tokens and no
//! provider to produce.
//!
//! # The budget is the hard part
//!
//! Whatever this returns is spent from the agent's context window before
//! the conversation starts. A profile that grows with the corpus would
//! silently eat that window as a user's memory store matures — so it is
//! capped, and what survives the cap is chosen rather than arbitrary.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::category::Category;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::shared::clock::Clock;
use crate::shared::error::Result;
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Rough characters-per-token for English prose. Used only to turn the
/// token budget into a character budget — a real tokenizer would be more
/// accurate and is not worth a dependency for a truncation heuristic.
const CHARS_PER_TOKEN: usize = 4;

/// The plan's budget (implementation-plan.md §3.2). Deliberately small:
/// this is rent paid on every session, forever.
pub const DEFAULT_TOKEN_BUDGET: usize = 1_500;

/// Cap per category, so one prolific category cannot crowd out the rest.
/// A profile listing forty coding preferences and no decisions is a worse
/// profile than one showing eight of each.
const MAX_PER_CATEGORY: usize = 8;

/// Order categories appear in. Preferences first because they are the ones
/// that change what an agent *does*; references last because they are
/// lookups, not context.
const CATEGORY_ORDER: &[Category] = &[
    Category::PreferenceCoding,
    Category::PreferencePersonal,
    Category::Decision,
    Category::FactProject,
    Category::FactPerson,
    Category::Skill,
    Category::Experience,
    Category::Reference,
];

pub struct ProfileAssembler {
    memories: Arc<dyn MemoryRepository>,
    clock: Arc<dyn Clock>,
    token_budget: usize,
}

impl ProfileAssembler {
    pub fn new(memories: Arc<dyn MemoryRepository>, clock: Arc<dyn Clock>) -> Self {
        Self {
            memories,
            clock,
            token_budget: DEFAULT_TOKEN_BUDGET,
        }
    }

    pub fn execute(&self, context: &UserContext) -> Result<String> {
        let now = self.clock.now();
        let memories = self.memories.list(context, false)?;
        let active: Vec<Memory> = memories
            .into_iter()
            .filter(|memory| memory.is_active_at(now))
            .collect();

        Ok(render(context.handle(), &active, now, self.char_budget()))
    }

    fn char_budget(&self) -> usize {
        self.token_budget * CHARS_PER_TOKEN
    }
}

fn render(handle: &str, memories: &[Memory], now: DateTime<Utc>, char_budget: usize) -> String {
    let mut output = format!(
        "# Memory profile: {handle} (updated {})\n",
        now.format("%Y-%m-%d")
    );

    if memories.is_empty() {
        output.push_str(
            "\nNo memories stored yet. Save one with the `memory_save` tool when \
             the user states a durable preference, decision or fact.\n",
        );
        return output;
    }

    let mut included = 0usize;
    let mut omitted = 0usize;

    for category in CATEGORY_ORDER {
        let mut group: Vec<&Memory> = memories
            .iter()
            .filter(|memory| memory.category() == category)
            .collect();
        if group.is_empty() {
            continue;
        }

        rank_for_profile(&mut group, now);
        let total = group.len();
        let shown = group.len().min(MAX_PER_CATEGORY);
        omitted += total - shown;

        let mut section = format!("\n## {}\n", heading(category));
        for memory in group.iter().take(shown) {
            section.push_str(&format!("- {}\n", one_line(memory)));
        }

        // Stop at the budget rather than truncating mid-section: half a
        // heading tells an agent nothing.
        if output.len() + section.len() > char_budget {
            omitted += total.min(MAX_PER_CATEGORY);
            continue;
        }

        output.push_str(&section);
        included += shown;
    }

    // Custom categories aren't in CATEGORY_ORDER; gather them last so a
    // configured extra category is never silently invisible.
    let mut extras: Vec<&Memory> = memories
        .iter()
        .filter(|memory| !CATEGORY_ORDER.contains(memory.category()))
        .collect();
    if !extras.is_empty() {
        rank_for_profile(&mut extras, now);
        let shown = extras.len().min(MAX_PER_CATEGORY);
        let mut section = String::from("\n## other\n");
        for memory in extras.iter().take(shown) {
            section.push_str(&format!("- {}\n", one_line(memory)));
        }
        if output.len() + section.len() <= char_budget {
            output.push_str(&section);
            included += shown;
        } else {
            omitted += shown;
        }
    }

    if omitted > 0 {
        output.push_str(&format!(
            "\n_{included} of {} memories shown. Use `memory_recall` to search the rest._\n",
            included + omitted
        ));
    }

    output
}

/// Orders a category's memories by what deserves the budget.
///
/// Confidence first (an uncertain memory is a poor thing to assert at
/// session start), then recency. Access count would be the better signal
/// and arrives with Phase 5's importance decay.
fn rank_for_profile(memories: &mut [&Memory], _now: DateTime<Utc>) {
    memories.sort_by(|a, b| {
        b.confidence()
            .partial_cmp(&a.confidence())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.created_at().cmp(&a.created_at()))
            .then_with(|| a.id().to_string().cmp(&b.id().to_string()))
    });
}

/// One memory, one line — newlines would break the markdown list and make
/// the profile unreadable to both humans and agents.
fn one_line(memory: &Memory) -> String {
    let content = memory.content().replace('\n', " ");
    if memory.tags().is_empty() {
        content
    } else {
        format!("{content} _({})_", memory.tags().join(", "))
    }
}

fn heading(category: &Category) -> &str {
    match category {
        Category::PreferenceCoding => "Coding preferences",
        Category::PreferencePersonal => "Personal preferences",
        Category::Decision => "Decisions",
        Category::FactProject => "Project facts",
        Category::FactPerson => "People",
        Category::Skill => "Skills",
        Category::Experience => "Experiences",
        Category::Reference => "References",
        Category::Custom(name) => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, new_memory, now};

    fn assembler(fixture: &Fixture) -> ProfileAssembler {
        ProfileAssembler::new(
            Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
            crate::memories::application::test_doubles::fixed_clock(),
        )
    }

    fn save_in(fixture: &Fixture, category: Category, content: &str) {
        let mut memory = new_memory(content);
        memory.category = category;
        fixture
            .saver()
            .execute(&fixture.alex, memory, "test")
            .unwrap();
    }

    #[test]
    fn groups_memories_under_readable_headings() {
        let fixture = Fixture::new();
        save_in(
            &fixture,
            Category::PreferenceCoding,
            "Prefers pnpm; never npm",
        );
        save_in(
            &fixture,
            Category::Decision,
            "SQLite over Postgres for installer size",
        );

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        assert!(profile.starts_with("# Memory profile: alex"), "{profile}");
        assert!(profile.contains("## Coding preferences"), "{profile}");
        assert!(profile.contains("- Prefers pnpm; never npm"), "{profile}");
        assert!(profile.contains("## Decisions"), "{profile}");
    }

    #[test]
    fn preferences_come_before_facts() {
        // Preferences change what an agent does; facts are background.
        let fixture = Fixture::new();
        save_in(
            &fixture,
            Category::FactProject,
            "the api is written in rust",
        );
        save_in(&fixture, Category::PreferenceCoding, "prefers pnpm");

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        let preferences = profile.find("## Coding preferences").unwrap();
        let facts = profile.find("## Project facts").unwrap();
        assert!(preferences < facts, "{profile}");
    }

    #[test]
    fn an_empty_profile_says_what_to_do_rather_than_nothing() {
        let fixture = Fixture::new();

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        assert!(profile.contains("No memories stored yet"), "{profile}");
        assert!(
            profile.contains("memory_save"),
            "an empty profile should point at the tool that fills it: {profile}"
        );
    }

    #[test]
    fn contains_only_the_callers_memories() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "alex's private preference");
        fixture.save(&fixture.sam, "sam's private preference");

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        assert!(profile.contains("alex's private preference"));
        assert!(
            !profile.contains("sam's private preference"),
            "the profile leaked another user's memories: {profile}"
        );
    }

    #[test]
    fn stays_within_the_token_budget_with_a_large_corpus() {
        let fixture = Fixture::new();
        for index in 0..1_000 {
            save_in(
                &fixture,
                Category::PreferenceCoding,
                &format!("coding preference number {index} with some explanatory text"),
            );
        }

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        let budget_chars = DEFAULT_TOKEN_BUDGET * CHARS_PER_TOKEN;
        assert!(
            profile.len() <= budget_chars,
            "profile was {} chars, budget is {budget_chars}",
            profile.len()
        );
    }

    #[test]
    fn says_how_much_it_left_out() {
        let fixture = Fixture::new();
        for index in 0..20 {
            save_in(
                &fixture,
                Category::PreferenceCoding,
                &format!("preference number {index}"),
            );
        }

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        assert!(
            profile.contains("of 20 memories shown"),
            "a truncated profile must say so: {profile}"
        );
        assert!(profile.contains("memory_recall"), "{profile}");
    }

    #[test]
    fn one_category_cannot_crowd_out_the_others() {
        let fixture = Fixture::new();
        for index in 0..50 {
            save_in(
                &fixture,
                Category::PreferenceCoding,
                &format!("coding preference {index}"),
            );
        }
        save_in(&fixture, Category::Decision, "the one decision");

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        assert!(
            profile.contains("the one decision"),
            "a prolific category buried the rest: {profile}"
        );
        assert_eq!(
            profile.matches("- coding preference").count(),
            MAX_PER_CATEGORY,
            "per-category cap not applied: {profile}"
        );
    }

    #[test]
    fn higher_confidence_memories_are_preferred() {
        let fixture = Fixture::new();
        for index in 0..MAX_PER_CATEGORY {
            let mut memory = new_memory(&format!("uncertain guess {index}"));
            memory.confidence = 0.2;
            fixture
                .saver()
                .execute(&fixture.alex, memory, "test")
                .unwrap();
        }
        let mut confident = new_memory("a confident preference");
        confident.confidence = 1.0;
        fixture
            .saver()
            .execute(&fixture.alex, confident, "test")
            .unwrap();

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        assert!(
            profile.contains("a confident preference"),
            "a confident memory lost its place to guesses: {profile}"
        );
    }

    #[test]
    fn superseded_and_deleted_memories_are_excluded() {
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "deploys on flyio");
        let new = fixture.save(&fixture.alex, "deploys on hetzner");
        fixture
            .memories
            .update(
                &fixture.alex,
                &old.clone().supersede(new.id(), now()),
                "test",
            )
            .unwrap();

        let deleted = fixture.save(&fixture.alex, "a deleted memory");
        fixture
            .memories
            .delete(&fixture.alex, deleted.id(), "test")
            .unwrap();

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        assert!(profile.contains("hetzner"));
        assert!(
            !profile.contains("flyio"),
            "superseded memory shown: {profile}"
        );
        assert!(!profile.contains("a deleted memory"), "{profile}");
    }

    #[test]
    fn multi_line_content_stays_on_one_line() {
        let fixture = Fixture::new();
        fixture.save(&fixture.alex, "a memory\nspanning lines");

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        assert!(profile.contains("- a memory spanning lines"), "{profile}");
    }

    #[test]
    fn a_configured_extra_category_is_not_invisible() {
        let fixture = Fixture::new();
        save_in(
            &fixture,
            Category::Custom("fact.homelab".to_string()),
            "the nas has 40tb",
        );

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        assert!(profile.contains("the nas has 40tb"), "{profile}");
    }
}
