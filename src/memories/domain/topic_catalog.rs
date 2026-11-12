//! The topics a user may file a memory under: the built-ins, plus their own
//! (ai-specs/memory-structure-plan.md §3.3). A save naming any other topic is
//! refused with the list, so a typo cannot create one.

use super::topic::{BUILT_IN_TOPICS, Topic};
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;

pub trait TopicCatalog: Send + Sync {
    /// The user's own topics, sorted by id.
    fn custom(&self, context: &UserContext) -> Result<Vec<Topic>>;

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
