//! The topics a user may file a memory under: the built-ins, plus their own
//! (ai-specs/memory-structure-plan.md §3.3). A save naming any other topic is
//! refused with the list, so a typo cannot create one.

use super::topic::{BUILT_IN_TOPICS, Topic};
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::{RaError, Result};

/// The longest label a user may give a topic.
pub const MAX_TOPIC_LABEL_LEN: usize = 64;

/// A topic as the user sees it: what it is, and their own wording for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicEntry {
    pub topic: Topic,
    /// The user's label, if they relabelled it.
    pub label: Option<String>,
}

impl TopicEntry {
    pub fn label(&self) -> String {
        self.label
            .clone()
            .unwrap_or_else(|| self.topic.default_label())
    }
}

/// A label as typed: trimmed, `None` when blank (back to the default).
pub fn normalise_topic_label(raw: Option<&str>) -> Result<Option<String>> {
    let Some(label) = raw.map(str::trim).filter(|label| !label.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > MAX_TOPIC_LABEL_LEN || label.chars().any(char::is_control) {
        return Err(RaError::Validation(format!(
            "a topic label is 1 to {MAX_TOPIC_LABEL_LEN} characters, with no control characters"
        )));
    }
    Ok(Some(label.to_string()))
}

pub trait TopicCatalog: Send + Sync {
    /// The user's own topics, sorted by id.
    fn custom(&self, context: &UserContext) -> Result<Vec<Topic>>;

    /// Every topic with the user's label for it: built-ins in their order,
    /// then the user's own by id.
    fn entries(&self, context: &UserContext) -> Result<Vec<TopicEntry>>;

    /// Adds a topic of the user's own. `Conflict` when they already have it
    /// or it is a built-in's id.
    fn create(&self, context: &UserContext, topic: &Topic, label: Option<&str>) -> Result<()>;

    /// Sets (or, with `None`, clears) the user's label for any topic they
    /// may use, built-ins included. `NotFound` for one they do not have.
    fn relabel(&self, context: &UserContext, topic: &Topic, label: Option<&str>) -> Result<()>;

    /// Built-ins in their order, then the user's own.
    fn all(&self, context: &UserContext) -> Result<Vec<Topic>> {
        let mut all = BUILT_IN_TOPICS.to_vec();
        all.extend(self.custom(context)?);
        Ok(all)
    }

    /// The topic with this id, if the user may use it.
    fn find(&self, context: &UserContext, raw: &str) -> Result<Option<Topic>> {
        let id = raw.trim().to_ascii_lowercase();
        if let Some(topic) = Topic::built_in(&id) {
            return Ok(Some(topic));
        }
        Ok(self
            .custom(context)?
            .into_iter()
            .find(|topic| topic.as_str() == id))
    }
}
