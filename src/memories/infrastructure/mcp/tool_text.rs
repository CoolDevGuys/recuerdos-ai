//! Tool descriptions and result rendering.
//!
//! This file is product surface, not boilerplate. An MCP tool is used by
//! a model reading its description, so the description *is* the trigger
//! logic — there is no other code path that decides when a memory gets
//! saved. Vague wording here shows up as an agent that either never
//! remembers anything or fills the store with "user asked me to run the
//! tests".
//!
//! Two rules the wording follows:
//!
//! 1. **Say when to call it, with examples of the trigger.** Models act on
//!    concrete phrasings ("I prefer", "we decided") far more reliably than
//!    on abstract descriptions of purpose.
//! 2. **Say when *not* to call it.** Negative examples are what stop a
//!    memory store filling with transient task chatter, which is the
//!    failure mode that makes people turn memory off.
//!
//! The three tool descriptions live as doc comments on the tool methods
//! in `memory_mcp_server.rs`, because that is what rmcp's `#[tool]` macro
//! reads (its `description` argument takes only a string literal, and
//! duplicating multi-paragraph text there would guarantee drift). They
//! are asserted against the *generated* tool list in the tests there,
//! which is what an agent actually receives.
//!
//! Result rendering is terse for the same reason: tool output lands in
//! the agent's context window, and JSON spends tokens on punctuation the
//! model does not need.

use super::memory_toolbox::{
    BatchSaveOutcome, ProjectChange, SaveOutcome, SaveStatus, ToolMemory, ToolProject,
    UpdateOutcome,
};

pub const PROFILE_DESCRIPTION: &str = "\
A short digest of who this user is: their own preferences, rules and \
goals, then the current project's conventions and rules, then the general \
rules and procedures of their world. Other projects are left out.

Read this at the start of a session, before doing work. It is the \
context the user should not have to repeat.";

/// Renders recall results for an agent's context window.
///
/// Numbered, one line each, scope first: `[preference · coding · me]` is
/// the user's own instruction everywhere, `[convention · coding · quill]`
/// one project's, and the strongest signal for how much weight to give a
/// line. A daemon that predates structured memories shows the category.
pub fn render_recall(memories: &[ToolMemory]) -> String {
    if memories.is_empty() {
        // Careful not to claim nothing was *stored*: results below the
        // relevance floor are filtered rather than absent, and an agent that
        // tells the user "I never saved that" on the strength of a floor is
        // reporting the threshold as if it were the contents.
        return "No memories matched this query well enough to return. Try different \
words, or a category filter. If that finds nothing either, it was probably never \
stored."
            .to_string();
    }

    let mut output = String::new();
    for (index, memory) in memories.iter().enumerate() {
        output.push_str(&format!(
            "{}. [{}] {}",
            index + 1,
            memory.label(),
            memory.content.replace('\n', " ")
        ));

        let mut annotations = vec![format!("saved {}", memory.created_at.format("%Y-%m-%d"))];
        if let Some(relevance) = memory.relevance {
            annotations.push(format!("relevance {relevance:.2}"));
        }
        if !memory.tags.is_empty() {
            annotations.push(memory.tags.join(", "));
        }
        output.push_str(&format!(" ({})\n", annotations.join(", ")));
    }

    output.trim_end().to_string()
}

/// What a save did, in words an agent repeats accurately: how it was filed,
/// or that the store already knew it.
pub fn render_saved(outcome: &SaveOutcome) -> String {
    let Some(memory) = outcome.memories.first() else {
        return "Nothing was stored. Do not tell the user it was saved.".to_string();
    };
    let content = memory.content.replace('\n', " ");
    match (outcome.status, &outcome.message) {
        (SaveStatus::AlreadyKnown, message) => format!(
            "{} It is not lost: it comes back on recall. Do not save it again.",
            message.clone().unwrap_or_else(|| format!(
                "Already known: \"{content}\" (id {}) says this, so nothing was saved.",
                memory.id
            ))
        ),
        (_, Some(message)) => format!(
            "{message} (id {}): {content}\nIt will be available in future sessions.",
            memory.id
        ),
        (_, None) => format!(
            "Saved as [{}] (id {}): {content}\nIt will be available in future sessions.",
            memory.category, memory.id
        ),
    }
}

/// What a batch save did, per item, phrased so an agent reports it
/// accurately — including the items the store already knew and the ones
/// that were refused, which "saved N memories" alone would paper over.
pub fn render_saved_batch(outcome: &BatchSaveOutcome) -> String {
    let total = outcome.items.len();
    let stored: Vec<_> = outcome
        .items
        .iter()
        .filter(|item| item.error.is_none() && item.status != Some(SaveStatus::AlreadyKnown))
        .flat_map(|item| item.memories.iter().map(move |memory| (item, memory)))
        .collect();
    let known: Vec<(usize, &str)> = outcome
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.error.is_none() && item.status == Some(SaveStatus::AlreadyKnown))
        .map(|(index, item)| (index, item.message.as_deref().unwrap_or("already known")))
        .collect();
    let failures: Vec<(usize, &str)> = outcome
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| item.error.as_deref().map(|error| (index, error)))
        .collect();

    let mut output = if stored.is_empty() {
        format!(
            "Nothing new was stored from {total} {}. Do not tell the user they were saved.",
            plural(total, "item", "items")
        )
    } else {
        format!(
            "Saved {} {} from {total} {}.",
            stored.len(),
            plural(stored.len(), "memory", "memories"),
            plural(total, "item", "items")
        )
    };

    if !stored.is_empty() {
        output.push('\n');
        for (item, memory) in &stored {
            // A structured save says how it was filed; that is what the
            // user may want to correct.
            let filed = item
                .message
                .clone()
                .unwrap_or_else(|| format!("[{}]", memory.category));
            output.push_str(&format!(
                "\n- {filed} {} (id {})",
                memory.content.replace('\n', " "),
                memory.id
            ));
        }
        output.push_str("\n\nThey will be available in future sessions.");
    }

    if !known.is_empty() {
        output.push_str(&format!(
            "\n\n{} {} added nothing new:",
            known.len(),
            plural(known.len(), "item", "items")
        ));
        for (index, message) in known {
            output.push_str(&format!("\n- item {index}: {message}"));
        }
    }

    if !failures.is_empty() {
        output.push_str(&format!(
            "\n\n{} {} {} not saved:",
            failures.len(),
            plural(failures.len(), "item", "items"),
            plural(failures.len(), "was", "were")
        ));
        for (index, error) in failures {
            output.push_str(&format!("\n- item {index}: {error}"));
        }
    }

    output
}

/// What an amendment did: the daemon's sentence and the memory as it now
/// reads.
pub fn render_updated(outcome: &UpdateOutcome) -> String {
    let memory = &outcome.memory;
    format!(
        "{} (id {}): {}",
        outcome
            .message
            .clone()
            .unwrap_or_else(|| "Updated.".to_string()),
        memory.id,
        memory.content.replace('\n', " ")
    )
}

/// Singular or plural word for a count, so the summaries read as English
/// rather than "1 items".
fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 { one } else { many }
}

/// What a session left behind.
///
/// Zero is the ordinary outcome and is phrased as a success. A session
/// hook runs on every session a user has, most of which produce nothing
/// durable — an agent told this looks like a failure would start
/// reporting a broken memory service to the user once a day.
pub fn render_distilled(memories: &[ToolMemory]) -> String {
    if memories.is_empty() {
        return "Nothing from this session needed remembering. That is the usual \
                outcome — most sessions produce nothing that stays true after they \
                end."
            .to_string();
    }

    let mut output = format!(
        "Distilled {} memor{} from this session:\n\n",
        memories.len(),
        if memories.len() == 1 { "y" } else { "ies" }
    );
    for memory in memories {
        output.push_str(&format!(
            "- [{}] {} (id {})\n",
            memory.label(),
            memory.content.replace('\n', " "),
            memory.id
        ));
    }
    output.push_str("\nThey will be available in future sessions.");
    output
}

/// The first half of `memory_forget`: what would be deleted, and how to
/// actually do it. Explicitly states that nothing has been deleted yet,
/// because a model that assumes otherwise will tell the user it has.
pub fn render_forget_candidates(memories: &[ToolMemory]) -> String {
    if memories.is_empty() {
        // Deleting the wrong memory is unrecoverable, so candidates are
        // held to a stricter bar than a recall. Saying so is what turns a
        // miss into a retry rather than a shrug — or, worse, a delete aimed
        // at whatever the agent found instead.
        return "Nothing matched closely enough to be a safe thing to delete, so \
nothing is proposed. Describe it more specifically — its exact wording, category, \
or a distinctive phrase — rather than deleting a near miss."
            .to_string();
    }

    let mut output = String::from("Nothing has been deleted yet. These memories match:\n\n");
    for memory in memories {
        output.push_str(&format!(
            "- {} — [{}] {}\n",
            memory.id,
            memory.label(),
            memory.content.replace('\n', " ")
        ));
    }
    output.push_str(
        "\nTo delete, call memory_forget again with the ids you want removed and \
         confirm: true. Only do this if the user asked for it.",
    );
    output
}

pub fn render_forgotten(count: usize) -> String {
    match count {
        0 => "No memories were deleted.".to_string(),
        1 => "Deleted 1 memory. It will not appear in future sessions.".to_string(),
        many => format!("Deleted {many} memories. They will not appear in future sessions."),
    }
}

/// `project_list`: one line per project.
pub fn render_projects(projects: &[ToolProject]) -> String {
    if projects.is_empty() {
        return "No projects yet. One is created when the user confirms a save to a new \
                project."
            .to_string();
    }
    let mut text = format!(
        "{} project{}:\n",
        projects.len(),
        if projects.len() == 1 { "" } else { "s" }
    );
    for project in projects {
        text.push_str(&format!(
            "- {} — {} {}",
            project.name,
            project.memories,
            if project.memories == 1 {
                "memory"
            } else {
                "memories"
            }
        ));
        if !project.aliases.is_empty() {
            text.push_str(&format!("; also found by {}", project.aliases.join(", ")));
        }
        text.push('\n');
    }
    text
}

/// `project_rename` and `project_merge`: the daemon's own sentence.
pub fn render_project_change(change: &ProjectChange) -> String {
    change.message.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn memory(content: &str, relevance: Option<f32>) -> ToolMemory {
        ToolMemory {
            id: "019f7c5a-0000-7000-8000-000000000001".to_string(),
            content: content.to_string(),
            category: "preference.coding".to_string(),
            tags: vec!["typescript".to_string()],
            created_at: Utc.with_ymd_and_hms(2026, 6, 2, 12, 0, 0).unwrap(),
            relevance,
            scope: None,
        }
    }

    use chrono::Utc;

    #[test]
    fn recall_renders_one_compact_line_per_memory() {
        let rendered = render_recall(&[memory("User prefers pnpm", Some(0.91))]);

        assert!(rendered.starts_with("1. [preference.coding] User prefers pnpm"));
        assert!(rendered.contains("saved 2026-06-02"));
        assert!(rendered.contains("relevance 0.91"));
        assert_eq!(rendered.lines().count(), 1, "one memory, one line");
    }

    #[test]
    fn recall_flattens_multi_line_content() {
        // A memory containing newlines would otherwise break the numbered
        // list and make the result ambiguous to parse or read.
        let rendered = render_recall(&[memory("line one\nline two", None)]);

        assert_eq!(rendered.lines().count(), 1, "got: {rendered}");
        assert!(rendered.contains("line one line two"));
    }

    #[test]
    fn an_empty_recall_says_nothing_is_stored_rather_than_returning_blank() {
        // A bare empty string reads to a model as a failure; this says
        // what the absence means.
        let rendered = render_recall(&[]);

        assert!(rendered.contains("No memories matched"), "{rendered}");
        assert!(!rendered.trim().is_empty());
    }

    #[test]
    fn forget_candidates_state_plainly_that_nothing_was_deleted() {
        let rendered = render_forget_candidates(&[memory("User prefers pnpm", None)]);

        assert!(
            rendered.contains("Nothing has been deleted yet"),
            "a model must not report a deletion that hasn't happened: {rendered}"
        );
        assert!(rendered.contains("confirm: true"), "{rendered}");
        assert!(
            rendered.contains("019f7c5a-0000-7000-8000-000000000001"),
            "the ids to pass back must be visible: {rendered}"
        );
    }

    #[test]
    fn forget_with_no_matches_does_not_invite_a_confirmation() {
        let rendered = render_forget_candidates(&[]);

        assert!(rendered.contains("nothing is proposed"), "{rendered}");
        assert!(!rendered.contains("confirm: true"), "{rendered}");
        // The failure the strict bar can produce is "I couldn't find it", and
        // the wrong recovery from that is deleting the nearest thing it did
        // find. The message has to close that door.
        assert!(rendered.contains("more specifically"), "{rendered}");
    }

    #[test]
    fn deletion_counts_read_naturally() {
        assert!(render_forgotten(1).contains("1 memory."));
        assert!(render_forgotten(3).contains("3 memories"));
        assert!(render_forgotten(0).contains("No memories were deleted"));
    }

    fn saved(status: SaveStatus, message: Option<&str>) -> String {
        render_saved(&SaveOutcome {
            memories: vec![memory("User prefers pnpm", None)],
            understanding: false,
            status,
            message: message.map(str::to_string),
        })
    }

    #[test]
    fn a_stored_memory_repeats_how_it_was_filed() {
        let rendered = saved(
            SaveStatus::Stored,
            Some("Saved as your personal coding preference (applies to you everywhere)."),
        );

        assert!(
            rendered.starts_with("Saved as your personal coding preference"),
            "{rendered}"
        );
        assert!(rendered.contains("User prefers pnpm"), "{rendered}");
    }

    #[test]
    fn a_save_that_was_already_known_says_it_is_not_lost() {
        let rendered = saved(
            SaveStatus::AlreadyKnown,
            Some("Already known: \"User prefers pnpm\" (id 1) says this, so nothing was saved."),
        );

        assert!(rendered.starts_with("Already known"), "{rendered}");
        assert!(
            rendered.contains("comes back on recall"),
            "an agent that fears the memory was dropped will retry it: {rendered}"
        );
        assert!(
            !rendered.contains("available in future sessions"),
            "{rendered}"
        );
    }

    #[test]
    fn a_save_with_nothing_to_show_never_claims_it_saved() {
        let rendered = render_saved(&SaveOutcome {
            memories: vec![],
            understanding: false,
            status: SaveStatus::Unknown,
            message: None,
        });

        assert!(rendered.contains("Nothing was stored"), "{rendered}");
        assert!(!rendered.contains("Saved as"), "{rendered}");
    }

    #[test]
    fn a_batch_reports_saved_known_and_refused_items_distinctly() {
        use super::super::memory_toolbox::BatchItemOutcome;

        let outcome = BatchSaveOutcome {
            understanding: false,
            items: vec![
                BatchItemOutcome {
                    memories: vec![memory("User prefers pnpm", None)],
                    error: None,
                    status: Some(SaveStatus::Stored),
                    message: None,
                },
                BatchItemOutcome {
                    memories: vec![memory("User prefers Vitest", None)],
                    error: None,
                    status: Some(SaveStatus::AlreadyKnown),
                    message: Some("Already known: \"User prefers Vitest\"".to_string()),
                },
                BatchItemOutcome {
                    memories: vec![],
                    error: Some("not saved (secret): looks like a token".to_string()),
                    status: None,
                    message: None,
                },
            ],
        };

        let rendered = render_saved_batch(&outcome);

        assert!(
            rendered.contains("Saved 1 memory from 3 items"),
            "{rendered}"
        );
        assert!(rendered.contains("User prefers pnpm"), "{rendered}");
        assert!(
            rendered.contains("1 item added nothing new:\n- item 1: Already known"),
            "a known item must be reported, not counted as saved: {rendered}"
        );
        assert!(rendered.contains("1 item was not saved"), "{rendered}");
        assert!(rendered.contains("looks like a token"), "{rendered}");
    }

    #[test]
    fn a_batch_that_stored_nothing_does_not_claim_it_saved() {
        use super::super::memory_toolbox::BatchItemOutcome;

        let outcome = BatchSaveOutcome {
            understanding: false,
            items: vec![BatchItemOutcome {
                memories: vec![memory("User prefers pnpm", None)],
                error: None,
                status: Some(SaveStatus::AlreadyKnown),
                message: None,
            }],
        };

        let rendered = render_saved_batch(&outcome);

        assert!(rendered.contains("Nothing new was stored"), "{rendered}");
        assert!(rendered.contains("Do not tell the user"), "{rendered}");
    }
}
