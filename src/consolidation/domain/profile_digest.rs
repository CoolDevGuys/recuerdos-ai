//! The cached, LLM-written profile — what it covers, and how it knows
//! when it is out of date.
//!
//! # Why one digest per scope rather than one document
//!
//! The digest is regenerated when the memories under it change. With one
//! document, saving a single project convention reprints the user's whole
//! profile — a model call and a page of rewriting for sections nothing
//! touched. So there is one digest per scope (ai-specs/memory-structure-plan.md
//! §9): the user's own memories, their general ones, and one per project.
//! A profile read from inside a project is three of them side by side — yours,
//! this project's, general — and a save regenerates only the one it changed.
//! Another project's digest is never in it.
//!
//! # Why staleness is computed, not flagged
//!
//! The obvious design is a `dirty` boolean set by whatever writes a
//! memory. It is also the fragile one: the flag has to be set at every
//! write site — save, update, forget, reconcile, merge, expire — and the
//! failure mode of missing one is a profile that is quietly wrong
//! forever, with nothing to notice it.
//!
//! So staleness is derived instead. A [`Fingerprint`] over the memories
//! a digest was built from is stored beside it; if the fingerprint no
//! longer matches, the digest is stale. A new write site cannot forget
//! to participate, because it does not have to.

use crate::memories::domain::applies_to::AppliesTo;
use crate::memories::domain::memory::Memory;
use crate::shared::ids::ProjectId;
use chrono::{DateTime, Utc};

/// One cached part of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Domain {
    /// Who the user is and how they like things done, everywhere.
    Me,
    /// One project's conventions, rules and facts.
    Project(ProjectId),
    /// The user's world outside any project.
    General,
}

impl Domain {
    /// The stored spelling, and the primary key alongside the user id.
    pub fn key(&self) -> String {
        match self {
            Domain::Me => "me".to_string(),
            Domain::General => "general".to_string(),
            Domain::Project(id) => format!("project:{id}"),
        }
    }

    /// Which digest a memory belongs to.
    pub fn of(memory: &Memory) -> Domain {
        match memory.structure().applies_to() {
            AppliesTo::Me => Domain::Me,
            AppliesTo::Project(project) => Domain::Project(project.id),
            AppliesTo::General => Domain::General,
        }
    }
}

/// A summary of the memories a digest was built from.
///
/// Count plus the latest `updated_at`: between them they move on every
/// mutation that can change a profile. An added or removed memory
/// changes the count; an edited, superseded, merged or expired one
/// changes the timestamp — because all of those write `updated_at`, and
/// the ones that do not write it (expiry soft-deletes) change the count
/// instead.
///
/// It is not a hash of the contents, deliberately. A hash would also
/// change when a memory's decay score is rescored nightly, which would
/// make the profile regenerate every single night for no reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    count: usize,
    latest: Option<DateTime<Utc>>,
}

impl Fingerprint {
    pub fn of(memories: &[&Memory]) -> Self {
        Self {
            count: memories.len(),
            latest: memories.iter().map(|memory| memory.updated_at()).max(),
        }
    }

    /// The stored form. A string because it is opaque to storage — the
    /// only operation is equality.
    pub fn render(&self) -> String {
        match self.latest {
            Some(latest) => format!("{}:{}", self.count, latest.to_rfc3339()),
            None => format!("{}:-", self.count),
        }
    }
}

/// A digest as it was stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDigest {
    pub content: String,
    pub fingerprint: String,
    pub generated_at: DateTime<Utc>,
}

impl StoredDigest {
    /// Whether this digest still describes `current`.
    pub fn covers(&self, current: &Fingerprint) -> bool {
        self.fingerprint == current.render()
    }
}

/// Where digests are cached between generations.
pub trait ProfileDigestStore: Send + Sync {
    fn find(
        &self,
        context: &crate::identity::domain::user_context::UserContext,
        domain: Domain,
    ) -> crate::shared::error::Result<Option<StoredDigest>>;

    /// Writes or replaces one domain's digest.
    fn save(
        &self,
        context: &crate::identity::domain::user_context::UserContext,
        domain: Domain,
        digest: &StoredDigest,
    ) -> crate::shared::error::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::domain::category::Category;
    use crate::memories::domain::memory::{MemorySource, NewMemory};
    use crate::shared::ids::UserId;
    use chrono::Duration;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
    }

    fn memory(category: Category, at: DateTime<Utc>) -> Memory {
        Memory::create(
            UserId::new(),
            NewMemory {
                content: "a memory".to_string(),
                category,
                subcategory: None,
                tags: vec![],
                entities: vec![],
                confidence: 1.0,
                source: MemorySource::default(),
                expires_at: None,
            },
            at,
        )
        .unwrap()
    }

    #[test]
    fn a_memory_goes_to_the_digest_of_its_scope() {
        // Legacy categories map to a scope (plan §8): a coding preference
        // is the user's, a reference is general.
        assert_eq!(
            Domain::of(&memory(Category::PreferenceCoding, now())),
            Domain::Me
        );
        assert_eq!(
            Domain::of(&memory(Category::Reference, now())),
            Domain::General
        );
    }

    #[test]
    fn every_domain_has_a_distinct_stored_name() {
        // They are half of a primary key.
        let project = ProjectId::new();
        let keys = [
            Domain::Me.key(),
            Domain::General.key(),
            Domain::Project(project).key(),
            Domain::Project(ProjectId::new()).key(),
        ];
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(Domain::Project(project).key(), format!("project:{project}"));
    }

    #[test]
    fn an_unchanged_memory_set_keeps_its_digest() {
        let memories = [memory(Category::PreferenceCoding, now())];
        let borrowed: Vec<&Memory> = memories.iter().collect();

        let digest = StoredDigest {
            content: "a digest".to_string(),
            fingerprint: Fingerprint::of(&borrowed).render(),
            generated_at: now(),
        };

        assert!(digest.covers(&Fingerprint::of(&borrowed)));
    }

    #[test]
    fn adding_or_removing_a_memory_invalidates_the_digest() {
        let one = [memory(Category::PreferenceCoding, now())];
        let two = [
            memory(Category::PreferenceCoding, now()),
            memory(Category::Decision, now()),
        ];

        let digest = StoredDigest {
            content: "a digest".to_string(),
            fingerprint: Fingerprint::of(&one.iter().collect::<Vec<_>>()).render(),
            generated_at: now(),
        };

        assert!(!digest.covers(&Fingerprint::of(&two.iter().collect::<Vec<_>>())));
    }

    #[test]
    fn editing_a_memory_invalidates_the_digest_without_changing_the_count() {
        // The case a count alone would miss: same memories, one rewritten.
        let before = [memory(Category::PreferenceCoding, now())];
        let after = [memory(
            Category::PreferenceCoding,
            now() + Duration::days(1),
        )];

        let digest = StoredDigest {
            content: "a digest".to_string(),
            fingerprint: Fingerprint::of(&before.iter().collect::<Vec<_>>()).render(),
            generated_at: now(),
        };

        assert!(!digest.covers(&Fingerprint::of(&after.iter().collect::<Vec<_>>())));
    }

    #[test]
    fn an_empty_set_fingerprints_without_panicking() {
        let empty: Vec<&Memory> = Vec::new();
        let fingerprint = Fingerprint::of(&empty);

        assert_eq!(fingerprint.render(), "0:-");
        assert_eq!(fingerprint, Fingerprint::of(&empty));
    }

    #[test]
    fn a_fingerprint_ignores_the_order_memories_arrive_in() {
        // They come from a SQL query whose row order is not promised.
        let first = memory(Category::PreferenceCoding, now());
        let second = memory(Category::Decision, now() + Duration::days(1));

        let forwards: Vec<&Memory> = vec![&first, &second];
        let backwards: Vec<&Memory> = vec![&second, &first];

        assert_eq!(Fingerprint::of(&forwards), Fingerprint::of(&backwards));
    }
}
