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

use super::memory_toolbox::{SaveOutcome, ToolMemory};

pub const PROFILE_DESCRIPTION: &str = "\
A short digest of who this user is: their standing preferences, \
decisions, and durable project facts, grouped by category.

Read this at the start of a session, before doing work. It is the \
context the user should not have to repeat.";

/// Renders recall results for an agent's context window.
///
/// Numbered, one line each, category first. The category is the strongest
/// signal for how much weight to give a line — a `preference.coding` is an
/// instruction, a `fact.project` is background.
pub fn render_recall(memories: &[ToolMemory]) -> String {
    if memories.is_empty() {
        return "No memories matched. Nothing has been stored on this subject.".to_string();
    }

    let mut output = String::new();
    for (index, memory) in memories.iter().enumerate() {
        output.push_str(&format!(
            "{}. [{}] {}",
            index + 1,
            memory.category,
            memory.content.replace('\n', " ")
        ));

        let mut annotations = vec![format!("saved {}", memory.created_at.format("%Y-%m-%d"))];
        if let Some(score) = memory.score {
            annotations.push(format!("score {score:.2}"));
        }
        if !memory.tags.is_empty() {
            annotations.push(memory.tags.join(", "));
        }
        output.push_str(&format!(" ({})\n", annotations.join(", ")));
    }

    output.trim_end().to_string()
}

/// What a save did, phrased so an agent can repeat it to the user
/// without overstating it.
///
/// The interesting case is zero memories. With understanding enabled that
/// means the store already knew this, or there was nothing durable in it
/// — both legitimate, and both very different from "saved". An agent told
/// "saved" after a NOOP goes on to tell the user something untrue.
pub fn render_saved(outcome: &SaveOutcome) -> String {
    if outcome.memories.is_empty() {
        return if outcome.understanding {
            "Nothing new was stored — either this is already known, or there was              nothing in it that stays true beyond this conversation. Do not tell the              user it was saved."
                .to_string()
        } else {
            "Nothing was stored.".to_string()
        };
    }

    if outcome.memories.len() == 1 {
        let memory = &outcome.memories[0];
        return format!(
            "Saved as [{}] (id {}): {}
It will be available in future sessions.",
            memory.category,
            memory.id,
            memory.content.replace('\n', " ")
        );
    }

    // Several memories from one submission means extraction split it.
    // Showing each is how the agent learns that its one sentence became
    // three separately-recallable facts.
    let mut output = format!("Saved {} memories:\n\n", outcome.memories.len());
    for memory in &outcome.memories {
        output.push_str(&format!(
            "- [{}] {} (id {})\n",
            memory.category,
            memory.content.replace('\n', " "),
            memory.id
        ));
    }
    output.push_str("\nThey will be available in future sessions.");
    output
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
            memory.category,
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
        return "No memories matched, so there is nothing to forget.".to_string();
    }

    let mut output = String::from("Nothing has been deleted yet. These memories match:\n\n");
    for memory in memories {
        output.push_str(&format!(
            "- {} — [{}] {}\n",
            memory.id,
            memory.category,
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn memory(content: &str, score: Option<f32>) -> ToolMemory {
        ToolMemory {
            id: "019f7c5a-0000-7000-8000-000000000001".to_string(),
            content: content.to_string(),
            category: "preference.coding".to_string(),
            tags: vec!["typescript".to_string()],
            created_at: Utc.with_ymd_and_hms(2026, 6, 2, 12, 0, 0).unwrap(),
            score,
        }
    }

    use chrono::Utc;

    #[test]
    fn recall_renders_one_compact_line_per_memory() {
        let rendered = render_recall(&[memory("User prefers pnpm", Some(0.91))]);

        assert!(rendered.starts_with("1. [preference.coding] User prefers pnpm"));
        assert!(rendered.contains("saved 2026-06-02"));
        assert!(rendered.contains("score 0.91"));
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

        assert!(rendered.contains("nothing to forget"), "{rendered}");
        assert!(!rendered.contains("confirm: true"), "{rendered}");
    }

    #[test]
    fn deletion_counts_read_naturally() {
        assert!(render_forgotten(1).contains("1 memory."));
        assert!(render_forgotten(3).contains("3 memories"));
        assert!(render_forgotten(0).contains("No memories were deleted"));
    }
}
