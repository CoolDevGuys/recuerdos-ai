//! In-memory doubles for the consolidation use cases.
//!
//! Mirrors the real adapter's user scoping, so a use case that forgot to
//! scope a lookup fails here rather than only in the slower integration
//! suite — which for the profile digest would mean one user's briefing
//! served to another.

use crate::consolidation::domain::profile_digest::{Domain, ProfileDigestStore, StoredDigest};
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use crate::shared::ids::UserId;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct InMemoryProfileDigestStore {
    digests: Mutex<HashMap<(UserId, String), StoredDigest>>,
}

impl ProfileDigestStore for InMemoryProfileDigestStore {
    fn find(&self, context: &UserContext, domain: Domain) -> Result<Option<StoredDigest>> {
        Ok(self
            .digests
            .lock()
            .expect("digest mutex poisoned")
            .get(&(context.user_id(), domain.as_str().to_string()))
            .cloned())
    }

    fn save(&self, context: &UserContext, domain: Domain, digest: &StoredDigest) -> Result<()> {
        self.digests.lock().expect("digest mutex poisoned").insert(
            (context.user_id(), domain.as_str().to_string()),
            digest.clone(),
        );
        Ok(())
    }
}
