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
    /// The point in *valid* time to read the graph at (Task 7.3.4). `None`
    /// means "as it stands now"; a value asks the graph hop what was true
    /// then — "what did we deploy on *before* the migration?". It filters
    /// only the graph leg's edge liveness; the vector and keyword legs, and
    /// the `since` transaction-time filter, are unaffected.
    as_of: Option<DateTime<Utc>>,
    limit: usize,
    include_superseded: bool,
    /// Results scoring below this are dropped rather than shown.
    ///
    /// `0.0` (the default) means no floor, which is the behaviour a
    /// reconciler or merger wants: they are asking "what is related to
    /// this?", and a weak relation is still an answer. An agent-facing
    /// surface wants the opposite — its default should be a floor, because
    /// a confident-looking weak result gets believed, and in the
    /// [`memory_forget`](crate::memories::infrastructure::mcp) case
    /// believed results are how the wrong memory gets deleted.
    min_relevance: f32,
}

impl RecallQuery {
    /// `limit` is signed so a client asking for `-5` reaches the domain's
    /// explanation instead of dying in a deserializer that describes Rust's
    /// number types to the caller.
    pub fn new(text: &str, limit: i64) -> Result<Self> {
        let text = text.trim();
        if text.is_empty() {
            return Err(RaError::Validation("query is empty".to_string()));
        }
        if text.chars().count() > MAX_QUERY_LEN {
            return Err(RaError::Validation(format!(
                "query is longer than {MAX_QUERY_LEN} characters"
            )));
        }
        if limit < 0 {
            return Err(RaError::Validation(format!(
                "limit must be at least 1, got {limit}"
            )));
        }
        if limit == 0 {
            return Err(RaError::Validation(
                "limit must be at least 1: ask for the results you want to read".to_string(),
            ));
        }

        Ok(Self {
            text: text.to_string(),
            categories: Vec::new(),
            subcategories: Vec::new(),
            tags: Vec::new(),
            since: None,
            as_of: None,
            // Clamped rather than rejected: a client asking for 200 wants
            // "as many as you'll give me", not an error.
            limit: (limit as usize).min(MAX_LIMIT),
            include_superseded: false,
            min_relevance: 0.0,
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

    /// Reads the graph hop at a point in valid time. `None` (the default)
    /// is "now".
    pub fn with_as_of(mut self, as_of: Option<DateTime<Utc>>) -> Self {
        self.as_of = as_of;
        self
    }

    pub fn including_superseded(mut self) -> Self {
        self.include_superseded = true;
        self
    }

    /// Drops results below `min_relevance`. Clamped into `0.0..=1.0`, so a
    /// misconfigured `1.5` means "show me the perfect ones only" rather
    /// than silently meaning "show me nothing at all", which is what an
    /// unclamped comparison above the maximum relevance would amount to.
    pub fn with_min_relevance(mut self, min_relevance: f32) -> Self {
        self.min_relevance = min_relevance.clamp(0.0, 1.0);
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

    pub fn as_of(&self) -> Option<DateTime<Utc>> {
        self.as_of
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn include_superseded(&self) -> bool {
        self.include_superseded
    }

    pub fn min_relevance(&self) -> f32 {
        self.min_relevance
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
    fn rejects_a_negative_limit_explaining_the_range() {
        // The number arrives from JSON, so this is the only place a
        // human-readable complaint can be made about it.
        let error = RecallQuery::new("x", -5).unwrap_err().to_string();
        assert!(error.contains("at least 1"), "got {error}");
        assert!(
            error.contains("-5"),
            "must quote what it was given: {error}"
        );
    }

    #[test]
    fn clamps_an_excessive_min_relevance() {
        // Above the maximum relevance an unclamped comparison would match
        // nothing at all, which is not what the caller meant.
        let query = RecallQuery::new("x", 5).unwrap().with_min_relevance(1.5);
        assert_eq!(query.min_relevance(), 1.0);

        let negative = RecallQuery::new("x", 5).unwrap().with_min_relevance(-1.0);
        assert_eq!(negative.min_relevance(), 0.0, "0.0 means no floor");
    }

    #[test]
    fn has_no_floor_by_default() {
        // Reconciliation asks for related memories, not relevant ones; an
        // inherited floor would quietly stop it seeing a supersession.
        assert_eq!(RecallQuery::new("x", 5).unwrap().min_relevance(), 0.0);
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
