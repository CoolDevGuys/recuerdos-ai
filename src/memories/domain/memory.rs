//! The `Memory` aggregate — one atomic thing worth remembering.
//!
//! "Atomic" is the design rule: a memory holds a single durable fact,
//! preference or decision, not a transcript. Retrieval returns whole
//! memories into an agent's context window, so a memory that bundles five
//! unrelated facts wastes four of them on every recall.

use super::category::Category;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{MemoryId, UserId};
use chrono::{DateTime, Utc};

/// Longer than this and it is a document, not a memory. Generous enough
/// for a paragraph of rationale on a decision.
pub const MAX_CONTENT_LEN: usize = 4_000;
pub const MAX_TAGS: usize = 32;
pub const MAX_TAG_LEN: usize = 64;

/// Where a memory came from — which client, which session.
///
/// Kept as a value object rather than free JSON so "who wrote this?" has
/// one shape across REST, MCP and the CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemorySource {
    /// e.g. `claude-code`, `hermes`, `rest`.
    pub client: Option<String>,
    pub session_id: Option<String>,
}

/// An entity mentioned by a memory (`{name: "Hetzner", kind: "service"}`).
///
/// Extracted from Phase 4 onward and unused by retrieval today. It is
/// carried now because the graph layer (project-plan.md §4, Strategy B)
/// needs it, and back-filling entities across an existing corpus later
/// means re-running an LLM over every memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Memory {
    id: MemoryId,
    user_id: UserId,
    content: String,
    category: Category,
    tags: Vec<String>,
    entities: Vec<Entity>,
    confidence: f32,
    source: MemorySource,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_accessed_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    superseded_by: Option<MemoryId>,
}

/// Everything needed to create a memory. A struct rather than a
/// nine-argument constructor, so callers can't silently transpose two
/// `Option<String>`s.
#[derive(Debug, Clone)]
pub struct NewMemory {
    pub content: String,
    pub category: Category,
    pub tags: Vec<String>,
    pub entities: Vec<Entity>,
    pub confidence: f32,
    pub source: MemorySource,
    pub expires_at: Option<DateTime<Utc>>,
}

impl Memory {
    pub fn create(user_id: UserId, new: NewMemory, now: DateTime<Utc>) -> Result<Self> {
        let content = new.content.trim();
        if content.is_empty() {
            return Err(RaError::Validation("memory content is empty".to_string()));
        }
        if content.chars().count() > MAX_CONTENT_LEN {
            return Err(RaError::Validation(format!(
                "memory content is longer than {MAX_CONTENT_LEN} characters"
            )));
        }

        if let Some(expires_at) = new.expires_at
            && expires_at <= now
        {
            return Err(RaError::Validation(
                "expires_at is already in the past".to_string(),
            ));
        }

        Ok(Self {
            id: MemoryId::new(),
            user_id,
            content: content.to_string(),
            category: new.category,
            tags: normalize_tags(new.tags)?,
            entities: new.entities,
            confidence: clamp_confidence(new.confidence),
            source: new.source,
            created_at: now,
            updated_at: now,
            last_accessed_at: None,
            expires_at: new.expires_at,
            superseded_by: None,
        })
    }

    /// Rebuilds from storage without re-validating: stored rows are
    /// historical fact (same reasoning as `User::from_stored`).
    #[allow(clippy::too_many_arguments)]
    pub fn from_stored(
        id: MemoryId,
        user_id: UserId,
        content: String,
        category: Category,
        tags: Vec<String>,
        entities: Vec<Entity>,
        confidence: f32,
        source: MemorySource,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        last_accessed_at: Option<DateTime<Utc>>,
        expires_at: Option<DateTime<Utc>>,
        superseded_by: Option<MemoryId>,
    ) -> Self {
        Self {
            id,
            user_id,
            content,
            category,
            tags,
            entities,
            confidence,
            source,
            created_at,
            updated_at,
            last_accessed_at,
            expires_at,
            superseded_by,
        }
    }

    pub fn id(&self) -> MemoryId {
        self.id
    }

    /// Whose memory this is. The repositories scope by the *context's*
    /// user rather than trusting this, so it is read by tests and by
    /// Phase 4's reconciliation rather than by the write path.
    #[allow(dead_code)]
    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn category(&self) -> &Category {
        &self.category
    }

    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub fn confidence(&self) -> f32 {
        self.confidence
    }

    pub fn source(&self) -> &MemorySource {
        &self.source
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    pub fn last_accessed_at(&self) -> Option<DateTime<Utc>> {
        self.last_accessed_at
    }

    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_at
    }

    pub fn superseded_by(&self) -> Option<MemoryId> {
        self.superseded_by
    }

    pub fn is_superseded(&self) -> bool {
        self.superseded_by.is_some()
    }

    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|expires| expires <= now)
    }

    /// Whether this memory should appear in ordinary recall.
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        !self.is_superseded() && !self.is_expired_at(now)
    }

    /// Applies an edit, returning the updated memory.
    ///
    /// Takes `self` by value: an edit produces a new state rather than
    /// mutating one in place, so a half-applied update can't be observed.
    pub fn edit(mut self, edit: MemoryEdit, now: DateTime<Utc>) -> Result<Self> {
        if let Some(content) = edit.content {
            let content = content.trim();
            if content.is_empty() {
                return Err(RaError::Validation("memory content is empty".to_string()));
            }
            if content.chars().count() > MAX_CONTENT_LEN {
                return Err(RaError::Validation(format!(
                    "memory content is longer than {MAX_CONTENT_LEN} characters"
                )));
            }
            self.content = content.to_string();
        }
        if let Some(category) = edit.category {
            self.category = category;
        }
        if let Some(tags) = edit.tags {
            self.tags = normalize_tags(tags)?;
        }
        if let Some(expires_at) = edit.expires_at {
            self.expires_at = expires_at;
        }

        self.updated_at = now;
        Ok(self)
    }

    /// Marks this memory as replaced by another. The memory is retained,
    /// only hidden from ordinary recall.
    ///
    /// Phase 4's reconciliation (ADD/UPDATE/DELETE/NOOP) is its first
    /// production caller; today it is exercised by tests, which is how
    /// the storage and recall paths already handle superseded rows.
    #[allow(dead_code)]
    pub fn supersede(mut self, replacement: MemoryId, now: DateTime<Utc>) -> Self {
        self.superseded_by = Some(replacement);
        self.updated_at = now;
        self
    }

    /// Records a recall. The SQLite repository does this in one UPDATE
    /// rather than by rebuilding the aggregate, so this is the in-memory
    /// path — used by the test double and by Phase 5's decay work.
    #[allow(dead_code)]
    pub fn mark_accessed(mut self, now: DateTime<Utc>) -> Self {
        self.last_accessed_at = Some(now);
        self
    }
}

/// A partial update. `None` means "leave alone"; `Some(None)` on
/// `expires_at` means "clear it".
#[derive(Debug, Clone, Default)]
pub struct MemoryEdit {
    pub content: Option<String>,
    pub category: Option<Category>,
    pub tags: Option<Vec<String>>,
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

fn clamp_confidence(confidence: f32) -> f32 {
    if confidence.is_nan() {
        // A NaN would poison every ranking comparison it touches.
        return 0.0;
    }
    confidence.clamp(0.0, 1.0)
}

/// Lowercases, trims, drops blanks and de-duplicates while preserving
/// order. Tags are matched exactly at query time, so `Rust` and `rust`
/// being two different tags would silently split a filter's results.
fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>> {
    let mut normalized: Vec<String> = Vec::with_capacity(tags.len());

    for tag in tags {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.is_empty() {
            continue;
        }
        if tag.chars().count() > MAX_TAG_LEN {
            return Err(RaError::Validation(format!(
                "tag {tag:?} is longer than {MAX_TAG_LEN} characters"
            )));
        }
        if !normalized.contains(&tag) {
            normalized.push(tag);
        }
    }

    if normalized.len() > MAX_TAGS {
        return Err(RaError::Validation(format!("more than {MAX_TAGS} tags")));
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn new_memory(content: &str) -> NewMemory {
        NewMemory {
            content: content.to_string(),
            category: Category::PreferenceCoding,
            tags: vec![],
            entities: vec![],
            confidence: 0.9,
            source: MemorySource::default(),
            expires_at: None,
        }
    }

    fn create(content: &str) -> Memory {
        Memory::create(UserId::new(), new_memory(content), now()).unwrap()
    }

    #[test]
    fn creates_a_memory() {
        let memory = create("  User prefers pnpm  ");

        assert_eq!(memory.content(), "User prefers pnpm", "content is trimmed");
        assert_eq!(memory.category(), &Category::PreferenceCoding);
        assert_eq!(memory.created_at(), now());
        assert_eq!(memory.updated_at(), now());
        assert_eq!(memory.last_accessed_at(), None);
        assert!(!memory.is_superseded());
        assert!(memory.is_active_at(now()));
    }

    #[test]
    fn rejects_empty_or_whitespace_content() {
        for content in ["", "   ", "\n\t "] {
            assert!(
                Memory::create(UserId::new(), new_memory(content), now()).is_err(),
                "{content:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_overlong_content() {
        let content = "a".repeat(MAX_CONTENT_LEN + 1);
        assert!(Memory::create(UserId::new(), new_memory(&content), now()).is_err());
    }

    #[test]
    fn accepts_content_at_the_limit() {
        let content = "a".repeat(MAX_CONTENT_LEN);
        assert!(Memory::create(UserId::new(), new_memory(&content), now()).is_ok());
    }

    #[test]
    fn normalizes_tags() {
        let mut new = new_memory("x");
        new.tags = vec![
            "  TypeScript ".to_string(),
            "typescript".to_string(),
            "".to_string(),
            "Imports".to_string(),
        ];

        let memory = Memory::create(UserId::new(), new, now()).unwrap();

        assert_eq!(
            memory.tags(),
            &["typescript".to_string(), "imports".to_string()],
            "tags should be lowercased, de-duplicated, blanks dropped, order kept"
        );
    }

    #[test]
    fn rejects_an_overlong_tag() {
        let mut new = new_memory("x");
        new.tags = vec!["a".repeat(MAX_TAG_LEN + 1)];
        assert!(Memory::create(UserId::new(), new, now()).is_err());
    }

    #[test]
    fn rejects_too_many_tags() {
        let mut new = new_memory("x");
        new.tags = (0..=MAX_TAGS).map(|i| format!("tag{i}")).collect();
        assert!(Memory::create(UserId::new(), new, now()).is_err());
    }

    #[test]
    fn clamps_confidence_into_range() {
        for (given, expected) in [(1.5, 1.0), (-0.2, 0.0), (0.5, 0.5), (f32::NAN, 0.0)] {
            let mut new = new_memory("x");
            new.confidence = given;
            let memory = Memory::create(UserId::new(), new, now()).unwrap();
            assert_eq!(memory.confidence(), expected, "for input {given}");
        }
    }

    #[test]
    fn rejects_an_expiry_in_the_past() {
        let mut new = new_memory("x");
        new.expires_at = Some(now() - chrono::Duration::seconds(1));
        assert!(Memory::create(UserId::new(), new, now()).is_err());
    }

    #[test]
    fn expiry_hides_a_memory_only_once_it_passes() {
        let mut new = new_memory("x");
        new.expires_at = Some(now() + chrono::Duration::days(1));
        let memory = Memory::create(UserId::new(), new, now()).unwrap();

        assert!(memory.is_active_at(now()));
        assert!(!memory.is_expired_at(now()));
        assert!(memory.is_expired_at(now() + chrono::Duration::days(2)));
        assert!(!memory.is_active_at(now() + chrono::Duration::days(2)));
    }

    #[test]
    fn editing_updates_only_the_given_fields() {
        let memory = create("original");
        let later = now() + chrono::Duration::hours(1);

        let edited = memory
            .clone()
            .edit(
                MemoryEdit {
                    content: Some("revised".to_string()),
                    ..MemoryEdit::default()
                },
                later,
            )
            .unwrap();

        assert_eq!(edited.content(), "revised");
        assert_eq!(edited.category(), memory.category(), "untouched");
        assert_eq!(edited.created_at(), now(), "creation time is immutable");
        assert_eq!(edited.updated_at(), later);
        assert_eq!(edited.id(), memory.id());
    }

    #[test]
    fn editing_can_clear_an_expiry() {
        let mut new = new_memory("x");
        new.expires_at = Some(now() + chrono::Duration::days(1));
        let memory = Memory::create(UserId::new(), new, now()).unwrap();

        let edited = memory
            .edit(
                MemoryEdit {
                    expires_at: Some(None),
                    ..MemoryEdit::default()
                },
                now(),
            )
            .unwrap();

        assert_eq!(edited.expires_at(), None);
    }

    #[test]
    fn editing_rejects_empty_content() {
        let memory = create("original");
        assert!(
            memory
                .edit(
                    MemoryEdit {
                        content: Some("  ".to_string()),
                        ..MemoryEdit::default()
                    },
                    now()
                )
                .is_err()
        );
    }

    #[test]
    fn superseding_retains_the_memory_but_deactivates_it() {
        let memory = create("old fact");
        let replacement = MemoryId::new();

        let superseded = memory.supersede(replacement, now());

        assert!(superseded.is_superseded());
        assert_eq!(superseded.superseded_by(), Some(replacement));
        assert!(!superseded.is_active_at(now()));
        assert_eq!(
            superseded.content(),
            "old fact",
            "content is kept — supersede is not delete"
        );
    }

    #[test]
    fn marking_accessed_records_the_time() {
        let memory = create("x");
        let later = now() + chrono::Duration::days(3);

        let accessed = memory.mark_accessed(later);

        assert_eq!(accessed.last_accessed_at(), Some(later));
    }
}
