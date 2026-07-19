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
    pub fn candidate_depth(&self) -> usize {
        (self.limit * 4).clamp(20, 200)
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
        assert!(RecallQuery::new("x", 1).unwrap().candidate_depth() >= 20);
        assert_eq!(RecallQuery::new("x", 50).unwrap().candidate_depth(), 200);
    }
}
