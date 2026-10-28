//! What text a memory's vector is built from.
//!
//! # The problem it solves
//!
//! A memory carries more meaning than its sentence. "Testing follows TDD
//! methodology" is filed as a *coding preference* and tagged `testing,
//! tdd`; "The user prefers pnpm" is a coding preference tagged
//! `package-management`. Embedding the sentence alone throws those labels
//! away, so a broad question — "what is the user's preferred tech stack?",
//! "how does the user approach testing?" — can only reach a memory through
//! whatever words its sentence happens to use. The labels still help the
//! keyword leg (tantivy indexes category and tags), but the vector leg,
//! the one that is supposed to match on *meaning*, never sees them.
//!
//! [`DocumentText::Enriched`] embeds the labels with the sentence, as a
//! short natural-language header the model can read:
//!
//! ```text
//! Coding preference (testing, tdd). Testing follows TDD methodology: …
//! ```
//!
//! # Why the format is part of the pin
//!
//! A vector built from `content` and one built from the enriched text are
//! not comparable even under the same model: the store's existing vectors
//! would all sit a little off from a query embedded against the new form.
//! So the format joins the model in the collection pin (see [`pin`]), and a
//! store built under one format refuses to open under the other until
//! `reindex` rebuilds it — the same path, and the same message, as a model
//! switch. `Content` keeps the bare model id, so every existing store opens
//! unchanged.
//!
//! [`pin`]: DocumentText::pin

use crate::memories::domain::category::Category;
use crate::memories::domain::memory::Memory;
use crate::shared::error::{RaError, Result};

/// Suffix that marks an enriched collection in the pin. Versioned so a
/// later change to the header is a new pin — and therefore a `reindex` —
/// rather than a silent mix of two formats in one index.
const ENRICHED_PIN_SUFFIX: &str = "+enriched-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DocumentText {
    /// The memory's sentence only — what every store before this option
    /// was built from.
    #[default]
    Content,
    /// A header naming the category, sub-label and tags, then the sentence.
    Enriched,
}

impl DocumentText {
    /// The accepted spellings, for config validation messages.
    pub const NAMES: &'static [&'static str] = &["content", "enriched"];

    pub fn parse(name: &str) -> Result<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "content" => Ok(Self::Content),
            "enriched" => Ok(Self::Enriched),
            other => Err(RaError::Validation(format!(
                "unknown document text {other:?} (expected content or enriched)"
            ))),
        }
    }

    /// The text to embed for `memory`.
    pub fn of(&self, memory: &Memory) -> String {
        self.render(
            memory.category(),
            memory.subcategory(),
            memory.tags(),
            memory.content(),
        )
    }

    /// The text to embed for a memory given as its parts — for callers that
    /// read rows rather than [`Memory`] values (`reindex`, `calibrate`).
    pub fn render(
        &self,
        category: &Category,
        subcategory: Option<&str>,
        tags: &[String],
        content: &str,
    ) -> String {
        match self {
            Self::Content => content.to_string(),
            Self::Enriched => {
                let mut labels: Vec<&str> = Vec::new();
                if let Some(subcategory) = subcategory.map(str::trim).filter(|s| !s.is_empty()) {
                    labels.push(subcategory);
                }
                labels.extend(tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()));

                let kind = describe(category);
                if labels.is_empty() {
                    format!("{kind}. {content}")
                } else {
                    format!("{kind} ({}). {content}", labels.join(", "))
                }
            }
        }
    }

    /// Whether an edit to the labels alone (tags, category, sub-label)
    /// changes the embedded text — and so needs the vector rebuilt.
    pub fn embeds_labels(&self) -> bool {
        matches!(self, Self::Enriched)
    }

    /// The collection pin for vectors built by `model_id` in this format.
    pub fn pin(&self, model_id: &str) -> String {
        match self {
            Self::Content => model_id.to_string(),
            Self::Enriched => format!("{model_id}{ENRICHED_PIN_SUFFIX}"),
        }
    }
}

/// The category as words a sentence-embedding model reads naturally. The
/// dotted ids (`preference.coding`) are a filter key, not language.
fn describe(category: &Category) -> String {
    match category {
        Category::PreferenceCoding => "Coding preference".to_string(),
        Category::PreferencePersonal => "Personal preference".to_string(),
        Category::Decision => "Decision".to_string(),
        Category::FactProject => "Project fact".to_string(),
        Category::FactPerson => "Fact about a person".to_string(),
        Category::Experience => "Experience".to_string(),
        Category::Skill => "Skill".to_string(),
        Category::Reference => "Reference".to_string(),
        Category::Custom(name) => {
            let words = name.replace(['.', '_', '-'], " ");
            let mut chars = words.trim().chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => "Memory".to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn content_embeds_the_sentence_unchanged() {
        let text = DocumentText::Content.render(
            &Category::PreferenceCoding,
            Some("testing"),
            &tags(&["tdd"]),
            "Testing follows TDD",
        );
        assert_eq!(text, "Testing follows TDD");
    }

    #[test]
    fn enriched_puts_the_labels_in_front_of_the_sentence() {
        let text = DocumentText::Enriched.render(
            &Category::PreferenceCoding,
            None,
            &tags(&["testing", "tdd"]),
            "Testing follows TDD",
        );
        assert_eq!(
            text,
            "Coding preference (testing, tdd). Testing follows TDD"
        );
    }

    #[test]
    fn enriched_lists_the_sub_label_before_the_tags() {
        let text = DocumentText::Enriched.render(
            &Category::FactProject,
            Some("frontend"),
            &tags(&["nextjs"]),
            "Uses Next.js",
        );
        assert_eq!(text, "Project fact (frontend, nextjs). Uses Next.js");
    }

    #[test]
    fn enriched_without_labels_still_names_the_category() {
        let text = DocumentText::Enriched.render(
            &Category::FactPerson,
            Some("  "),
            &tags(&["", " "]),
            "Lives in Berlin",
        );
        assert_eq!(text, "Fact about a person. Lives in Berlin");
    }

    #[test]
    fn a_custom_category_reads_as_words() {
        let text = DocumentText::Enriched.render(
            &Category::Custom("health.sleep_log".to_string()),
            None,
            &[],
            "Slept 7h",
        );
        assert_eq!(text, "Health sleep log. Slept 7h");
    }

    #[test]
    fn content_keeps_the_bare_model_pin_so_existing_stores_open() {
        assert_eq!(
            DocumentText::Content.pin("bge-small-en-v1.5"),
            "bge-small-en-v1.5"
        );
    }

    #[test]
    fn enriched_pins_a_distinct_collection() {
        assert_eq!(
            DocumentText::Enriched.pin("bge-small-en-v1.5"),
            "bge-small-en-v1.5+enriched-v1"
        );
    }

    #[test]
    fn parses_both_names_and_rejects_others() {
        assert_eq!(
            DocumentText::parse(" Enriched ").unwrap(),
            DocumentText::Enriched
        );
        assert_eq!(
            DocumentText::parse("content").unwrap(),
            DocumentText::Content
        );
        assert!(matches!(
            DocumentText::parse("tags"),
            Err(RaError::Validation(_))
        ));
    }

    #[test]
    fn only_enriched_text_depends_on_the_labels() {
        assert!(DocumentText::Enriched.embeds_labels());
        assert!(!DocumentText::Content.embeds_labels());
    }
}
