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
use crate::memories::domain::applies_to::{AppliesTo, ProjectRef};
use crate::memories::domain::kind::Kind;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::memories::domain::self_names::SelfNames;
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

/// Cap per kind within a scope, so one prolific kind cannot crowd out the
/// rest. A profile listing forty coding preferences and no rules is a worse
/// profile than one showing eight of each.
const MAX_PER_KIND: usize = 8;

/// What a profile is made of, in the order it is read
/// (ai-specs/memory-structure-plan.md §9, D14): who the user is everywhere,
/// then the project the caller is in, then the user's world outside any
/// project. Within each, the kinds that change what an agent *does* come
/// first. Another project's memories are never in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileScope {
    Me,
    Project,
    General,
}

impl ProfileScope {
    pub fn kinds(&self) -> &'static [Kind] {
        match self {
            ProfileScope::Me => &[
                Kind::Preference,
                Kind::Rule,
                Kind::Goal,
                Kind::Fact,
                Kind::Learning,
                Kind::Experience,
                Kind::Procedure,
                Kind::Reference,
            ],
            ProfileScope::Project => &[
                Kind::Convention,
                Kind::Rule,
                Kind::Decision,
                Kind::Goal,
                Kind::Plan,
                Kind::Procedure,
                Kind::Fact,
                Kind::Learning,
                Kind::Experience,
                Kind::Reference,
            ],
            ProfileScope::General => &[
                Kind::Rule,
                Kind::Procedure,
                Kind::Plan,
                Kind::Decision,
                Kind::Reference,
                Kind::Fact,
                Kind::Learning,
                Kind::Experience,
            ],
        }
    }

    /// Which part of the profile a memory belongs to, for a caller in
    /// `project`; `None` for another project's memory.
    pub fn of(memory: &Memory, project: Option<&ProjectRef>) -> Option<ProfileScope> {
        match memory.structure().applies_to() {
            AppliesTo::Me => Some(ProfileScope::Me),
            AppliesTo::General => Some(ProfileScope::General),
            AppliesTo::Project(theirs) => {
                (project.map(|p| p.id) == Some(theirs.id)).then_some(ProfileScope::Project)
            }
        }
    }

    pub fn heading(&self, project: Option<&ProjectRef>) -> String {
        match (self, project) {
            (ProfileScope::Me, _) => "About you (everywhere)".to_string(),
            (ProfileScope::Project, Some(project)) => format!("This project: {}", project.name),
            (ProfileScope::Project, None) => "This project".to_string(),
            (ProfileScope::General, _) => "Your world (not tied to a project)".to_string(),
        }
    }
}

pub const PROFILE_SCOPES: &[ProfileScope] = &[
    ProfileScope::Me,
    ProfileScope::Project,
    ProfileScope::General,
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

    /// The profile for a caller in no project.
    #[cfg(test)]
    pub fn execute(&self, context: &UserContext) -> Result<String> {
        self.execute_in(context, None)
    }

    /// The profile for a caller working in `project`.
    pub fn execute_in(
        &self,
        context: &UserContext,
        project: Option<&ProjectRef>,
    ) -> Result<String> {
        let now = self.clock.now();
        let memories = self.memories.list(context, false)?;
        let active: Vec<Memory> = memories
            .into_iter()
            .filter(|memory| memory.is_active_at(now))
            .collect();

        Ok(render(
            context.handle(),
            &active,
            project,
            now,
            self.char_budget(),
        ))
    }

    fn char_budget(&self) -> usize {
        self.token_budget * CHARS_PER_TOKEN
    }
}

fn render(
    handle: &str,
    memories: &[Memory],
    project: Option<&ProjectRef>,
    now: DateTime<Utc>,
    char_budget: usize,
) -> String {
    let mut output = format!(
        "# Memory profile: {handle} (updated {})\n",
        now.format("%Y-%m-%d")
    );

    let relevant: Vec<(ProfileScope, &Memory)> = memories
        .iter()
        .filter_map(|memory| ProfileScope::of(memory, project).map(|scope| (scope, memory)))
        .collect();
    if relevant.is_empty() {
        output.push_str(
            "\nNo memories stored yet. Save one with the `memory_save` tool when \
             the user states a durable preference, decision or fact.\n",
        );
        return output;
    }

    let mut included = 0usize;
    let mut omitted = 0usize;

    for scope in PROFILE_SCOPES {
        let theirs: Vec<&Memory> = relevant
            .iter()
            .filter(|(of, _)| of == scope)
            .map(|(_, memory)| *memory)
            .collect();
        if theirs.is_empty() {
            continue;
        }
        let heading = format!("\n## {}\n", scope.heading(project));
        let mut wrote_heading = false;

        // The scope's own kinds first, in order, then any it does not list
        // (a stored row the rules have since moved), so none is invisible.
        let mut kinds: Vec<Kind> = scope.kinds().to_vec();
        for memory in &theirs {
            if !kinds.contains(&memory.structure().kind()) {
                kinds.push(memory.structure().kind());
            }
        }

        for kind in kinds {
            let mut group: Vec<&Memory> = theirs
                .iter()
                .copied()
                .filter(|memory| memory.structure().kind() == kind)
                .collect();
            if group.is_empty() {
                continue;
            }
            rank_for_profile(&mut group, now);
            let total = group.len();
            let shown = total.min(MAX_PER_KIND);
            omitted += total - shown;

            let mut section = String::new();
            if !wrote_heading {
                section.push_str(&heading);
            }
            section.push_str(&format!("\n### {}\n", kind_heading(*scope, kind)));
            for memory in group.iter().take(shown) {
                section.push_str(&format!("- {}\n", one_line(memory)));
            }

            // Stop at the budget rather than truncating mid-section: half a
            // heading tells an agent nothing.
            if output.len() + section.len() > char_budget {
                omitted += shown;
                continue;
            }
            output.push_str(&section);
            wrote_heading = true;
            included += shown;
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

/// A kind's heading inside a scope. A fact about the user is what they call
/// their personal information: who they are, their skills, their work.
fn kind_heading(scope: ProfileScope, kind: Kind) -> &'static str {
    if scope == ProfileScope::Me && kind == Kind::Fact {
        return "Personal information";
    }
    match kind {
        Kind::Preference => "Preferences",
        Kind::Rule => "Rules",
        Kind::Convention => "Conventions",
        Kind::Decision => "Decisions",
        Kind::Fact => "Facts",
        Kind::Experience => "Experiences",
        Kind::Learning => "Learnings",
        Kind::Procedure => "Procedures",
        Kind::Reference => "References",
        Kind::Goal => "Goals",
        Kind::Plan => "Plans",
    }
}

/// Says who "the user" is right under the profile's title, so an agent reading
/// "The user prefers…" and a memory naming them knows both are one person.
/// Unchanged with no names.
pub fn introduce(profile: String, names: &SelfNames) -> String {
    let Some(sentence) = names.sentence() else {
        return profile;
    };
    match profile.split_once('\n') {
        Some((title, rest)) => format!("{title}\n\n{sentence}\n{rest}"),
        None => format!("{profile}\n\n{sentence}\n"),
    }
}

/// Orders a kind's memories by what deserves the budget.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::{Fixture, new_memory, now};
    use crate::memories::domain::category::Category;

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
        assert!(profile.contains("## About you (everywhere)"), "{profile}");
        assert!(profile.contains("### Preferences"), "{profile}");
        assert!(profile.contains("- Prefers pnpm; never npm"), "{profile}");
        assert!(profile.contains("### Decisions"), "{profile}");
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

        let preferences = profile.find("### Preferences").unwrap();
        let facts = profile.find("### Facts").unwrap();
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
            MAX_PER_KIND,
            "per-kind cap not applied: {profile}"
        );
    }

    #[test]
    fn higher_confidence_memories_are_preferred() {
        let fixture = Fixture::new();
        for index in 0..MAX_PER_KIND {
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
            .delete(&fixture.alex, deleted.id(), "test", "")
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

    fn in_project(fixture: &Fixture, project: &ProjectRef, kind: Kind, content: &str) {
        use crate::memories::domain::memory_structure::{MemoryStructure, Origin};
        use crate::memories::domain::topic::Topic;
        let structure = MemoryStructure::new(
            kind,
            Topic::Coding,
            AppliesTo::Project(project.clone()),
            Origin::UserSaid,
            None,
        )
        .unwrap();
        fixture
            .saver()
            .execute_structured(&fixture.alex, new_memory(content), structure, "test")
            .unwrap();
    }

    #[test]
    fn the_current_projects_memories_follow_yours_and_other_projects_are_left_out() {
        let fixture = Fixture::new();
        let registry = fixture.project_registry();
        let quill = registry.create(&fixture.alex, "quill", &[]).unwrap();
        let fern = registry.create(&fixture.alex, "fern", &[]).unwrap();
        save_in(&fixture, Category::PreferenceCoding, "prefers pnpm");
        save_in(
            &fixture,
            Category::Reference,
            "the nas backups are in /mnt/tank",
        );
        in_project(
            &fixture,
            &quill,
            Kind::Convention,
            "use cases are named by intent",
        );
        in_project(
            &fixture,
            &fern,
            Kind::Convention,
            "fern names handlers after commands",
        );

        let profile = assembler(&fixture)
            .execute_in(&fixture.alex, Some(&quill))
            .unwrap();

        let you = profile.find("## About you").unwrap();
        let here = profile.find("## This project: quill").unwrap();
        let general = profile.find("## Your world").unwrap();
        assert!(you < here && here < general, "{profile}");
        assert!(
            profile.contains("use cases are named by intent"),
            "{profile}"
        );
        assert!(
            !profile.contains("fern names handlers"),
            "another project's convention is in the profile: {profile}"
        );

        let nowhere = assembler(&fixture).execute(&fixture.alex).unwrap();
        assert!(!nowhere.contains("This project"), "{nowhere}");
        assert!(!nowhere.contains("named by intent"), "{nowhere}");
        assert!(nowhere.contains("prefers pnpm"), "{nowhere}");
    }

    #[test]
    fn a_closed_goal_is_not_in_the_profile() {
        use crate::memories::domain::memory_structure::{GoalStatus, MemoryStructure, Origin};
        use crate::memories::domain::topic::Topic;
        let fixture = Fixture::new();
        for (content, status) in [
            ("run a half marathon in april", GoalStatus::Active),
            ("learn the cello", GoalStatus::Abandoned),
        ] {
            let structure = MemoryStructure::new(
                Kind::Goal,
                Topic::Health,
                AppliesTo::Me,
                Origin::UserSaid,
                Some(status),
            )
            .unwrap();
            fixture
                .saver()
                .execute_structured(&fixture.alex, new_memory(content), structure, "test")
                .unwrap();
        }

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        assert!(profile.contains("### Goals"), "{profile}");
        assert!(profile.contains("half marathon"), "{profile}");
        assert!(!profile.contains("cello"), "{profile}");
    }

    #[test]
    fn facts_about_the_user_are_their_personal_information() {
        use crate::memories::domain::applies_to::AppliesTo;
        use crate::memories::domain::memory_structure::{MemoryStructure, Origin};
        use crate::memories::domain::topic::Topic;
        let fixture = Fixture::new();
        let memory = fixture.save(&fixture.alex, "The user is an expert in PHP");
        let structure = MemoryStructure::new(
            Kind::Fact,
            Topic::Work,
            AppliesTo::Me,
            Origin::UserSaid,
            None,
        )
        .unwrap();
        fixture
            .memories
            .update(&fixture.alex, &memory.with_structure(structure), "test")
            .unwrap();

        let profile = assembler(&fixture).execute(&fixture.alex).unwrap();

        let about_you = profile.find("## About you (everywhere)").unwrap();
        let personal = profile.find("### Personal information").unwrap();
        assert!(about_you < personal, "{profile}");
        assert!(
            profile.contains("- The user is an expert in PHP"),
            "{profile}"
        );
    }

    #[test]
    fn the_profile_says_who_the_user_is_under_its_title() {
        let names =
            SelfNames::parse(&["Alejandro Romero".to_string(), "Alex".to_string()]).unwrap();
        let profile = introduce(
            "# Memory profile: alex (updated 2026-11-14)\n\n## About you (everywhere)\n"
                .to_string(),
            &names,
        );
        assert_eq!(
            profile,
            "# Memory profile: alex (updated 2026-11-14)\n\n\
             The user is Alejandro Romero (also called Alex).\n\n\
             ## About you (everywhere)\n"
        );
        assert_eq!(
            introduce("# t\n".to_string(), &SelfNames::default()),
            "# t\n"
        );
    }
}
