//! What a caller is asking for.

use super::category::Category;
use crate::shared::error::{RaError, Result};
use chrono::{DateTime, Utc};

/// Hard ceiling on `limit`. Results go into an agent's context window,
/// where a hundred memories would crowd out the actual conversation —
/// and an unbounded limit is a trivial way to make the server do work.
pub const MAX_LIMIT: usize = 50;
pub const MAX_QUERY_LEN: usize = 1_000;

#[derive(Debug, Clone, PartialEq)]
pub struct RecallQuery {
    text: String,
    categories: Vec<Category>,
    /// Optional finer sub-labels under a category. OR-ed: a memory
    /// matching any one of them is included.
    subcategories: Vec<String>,
    tags: Vec<String>,
    since: Option<DateTime<Utc>>,
    limit: usize,
    include_superseded: bool,
}

impl RecallQuery {
    pub fn new(text: &str, limit: usize) -> Result<Self> {
        let text = text.trim();
        if text.is_empty() {
            return Err(RaError::Validation("query is empty".to_string()));
        }
        if text.chars().count() > MAX_QUERY_LEN {
            return Err(RaError::Validation(format!(
                "query is longer than {MAX_QUERY_LEN} characters"
            )));
        }
        if limit == 0 {
            return Err(RaError::Validation("limit is 0".to_string()));
        }

        Ok(Self {
            text: text.to_string(),
            categories: Vec::new(),
            subcategories: Vec::new(),
            tags: Vec::new(),
            since: None,
            // Clamped rather than rejected: a client asking for 200 wants
            // "as many as you'll give me", not an error.
            limit: limit.min(MAX_LIMIT),
            include_superseded: false,
        })
    }

    pub fn with_categories(mut self, categories: Vec<Category>) -> Self {
        self.categories = categories;
        self
    }

    /// Tags are AND-ed: a memory must carry every one of them. Filters
    /// are for narrowing, and OR-ing would widen instead.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags
            .into_iter()
            .map(|tag| tag.trim().to_ascii_lowercase())
            .filter(|tag| !tag.is_empty())
            .collect();
        self
    }

    /// Subcategories are OR-ed: a memory matching any one is included.
    pub fn with_subcategories(mut self, subcategories: Vec<String>) -> Self {
        self.subcategories = subcategories
            .into_iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        self
    }

    pub fn with_since(mut self, since: Option<DateTime<Utc>>) -> Self {
        self.since = since;
        self
    }

    pub fn including_superseded(mut self) -> Self {
        self.include_superseded = true;
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn categories(&self) -> &[Category] {
        &self.categories
    }

    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    pub fn subcategories(&self) -> &[String] {
        &self.subcategories
    }

    pub fn since(&self) -> Option<DateTime<Utc>> {
        self.since
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn include_superseded(&self) -> bool {
        self.include_superseded
    }

    /// How many candidates each leg should fetch.
    ///
    /// Wider than `limit` on purpose: fusion and post-filtering both
    /// discard candidates, so asking each leg for exactly `limit` would
    /// return fewer than asked for whenever the legs disagree.
    ///
    /// The floor is what matters for a *filtered* recall. A
    /// category/tag/subcategory filter is applied only after both legs
    /// answer (see [`MemoryRecaller`](crate::memories::application)), so a
    /// selective filter can discard nearly the whole window — and a memory
    /// that matches the filter but is only a weak match for the query text
    /// never survives to be filtered if it fell outside the window. At a
    /// floor of 20, a corpus larger than 20 in the queried scope could
    /// silently drop such a memory; a limit-5 query over a routine
    /// single-category working set already exceeds that. Widening only the
    /// window can add lower-ranked candidates, never displace a top one, so
    /// it costs a few extra row fetches and strictly helps filtered recall.
    /// The real fix — pushing filters into both indexes — is the scalable
    /// version; this floor is the cheap one that keeps filtered recall
    /// honest at personal scale.
    pub fn candidate_depth(&self) -> usize {
        (self.limit * 8).clamp(40, 200)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_query_with_defaults() {
        let query = RecallQuery::new("  package manager  ", 5).unwrap();

        assert_eq!(query.text(), "package manager");
        assert_eq!(query.limit(), 5);
        assert!(query.categories().is_empty());
        assert!(!query.include_superseded());
    }

    #[test]
    fn rejects_an_empty_query() {
        assert!(RecallQuery::new("   ", 5).is_err());
    }

    #[test]
    fn rejects_an_overlong_query() {
        assert!(RecallQuery::new(&"a".repeat(MAX_QUERY_LEN + 1), 5).is_err());
    }

    #[test]
    fn rejects_a_zero_limit() {
        assert!(RecallQuery::new("x", 0).is_err());
    }

    #[test]
    fn clamps_an_excessive_limit_rather_than_failing() {
        let query = RecallQuery::new("x", 10_000).unwrap();
        assert_eq!(query.limit(), MAX_LIMIT);
    }

    #[test]
    fn normalizes_tag_filters() {
        let query = RecallQuery::new("x", 5)
            .unwrap()
            .with_tags(vec![" Rust ".to_string(), "".to_string()]);

        assert_eq!(query.tags(), &["rust".to_string()]);
    }

    #[test]
    fn candidate_depth_exceeds_the_limit_but_stays_bounded() {
        // The floor is what rescues a filtered recall over a corpus larger
        // than the window, so a small limit still over-fetches generously.
        assert!(RecallQuery::new("x", 1).unwrap().candidate_depth() >= 40);
        // Between floor and ceiling the multiplier governs.
        assert_eq!(RecallQuery::new("x", 10).unwrap().candidate_depth(), 80);
        assert_eq!(RecallQuery::new("x", 50).unwrap().candidate_depth(), 200);
    }
}
