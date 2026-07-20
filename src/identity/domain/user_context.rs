//! `UserContext`: proof that a request was authenticated, and the only
//! way to name whose data is being touched.
//!
//! # The isolation guarantee
//!
//! Every repository method in every context takes `&UserContext` rather
//! than a bare `UserId`. Combined with the constructor below being
//! `pub(in crate::identity)`, that means **no code outside this context
//! can invent a context for a user it did not authenticate** — it can
//! only pass along one the auth middleware produced. Forging access to
//! another user's memories isn't a bug you have to remember not to write;
//! it fails to compile.
//!
//! This is the type-level half of project-plan.md §11. The runtime half —
//! that every query actually filters by `user_id` — is covered by
//! `tests/identity_isolation.rs`.

use super::scope::Scope;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{ApiKeyId, UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserContext {
    user_id: UserId,
    handle: String,
    key_id: Option<ApiKeyId>,
    scopes: Vec<Scope>,
}

impl UserContext {
    /// Mints a context for a successfully authenticated key.
    ///
    /// Deliberately `pub(in crate::identity)`: see the module docs.
    pub(in crate::identity) fn authenticated(
        user_id: UserId,
        handle: String,
        key_id: ApiKeyId,
        scopes: Vec<Scope>,
    ) -> Self {
        Self {
            user_id,
            handle,
            key_id: Some(key_id),
            scopes,
        }
    }

    /// Mints the context used when `[auth].mode = "none"` — a single-user
    /// deployment that opted out of authentication entirely. Carries no
    /// key id, so audit records can tell "the unauthenticated local user"
    /// apart from a real key.
    pub(in crate::identity) fn unauthenticated(user_id: UserId, handle: String) -> Self {
        Self {
            user_id,
            handle,
            key_id: None,
            scopes: vec![Scope::Admin],
        }
    }

    /// Mints the context a background worker acts under.
    ///
    /// Async ingestion is the reason this exists: a job outlives the
    /// request that enqueued it, so by the time it runs there is no key
    /// to authenticate. The guarantee is preserved by *where the user id
    /// comes from* — the job row, written by a handler that had already
    /// authenticated as that user, and never from anything a caller
    /// supplies at claim time.
    ///
    /// Narrower than [`Self::unauthenticated`] on purpose: read and write
    /// only. A background job has no business revoking keys, and carrying
    /// Admin here would make the worker the widest-privileged code in the
    /// process. It also carries no key id, so the audit trail can tell
    /// pipeline writes from a client's.
    pub(in crate::identity) fn background(user_id: UserId, handle: String) -> Self {
        Self {
            user_id,
            handle,
            key_id: None,
            scopes: vec![Scope::Read, Scope::Write],
        }
    }

    /// Whose data this request may touch. Phase 1 has no data to scope
    /// yet; from Phase 2 every repository call takes this.
    #[allow(dead_code)]
    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn handle(&self) -> &str {
        &self.handle
    }

    pub fn key_id(&self) -> Option<ApiKeyId> {
        self.key_id
    }

    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    #[allow(dead_code)] // used by `require` below and by tests
    pub fn allows(&self, required: Scope) -> bool {
        self.scopes.contains(&Scope::Admin) || self.scopes.contains(&required)
    }

    /// Gate for a scoped operation: `Ok(())` or a `Forbidden` error
    /// naming the scope that was missing.
    #[allow(dead_code)] // called by the ReadAccess/WriteAccess extractors
    pub fn require(&self, required: Scope) -> Result<()> {
        if self.allows(required) {
            Ok(())
        } else {
            Err(RaError::Forbidden(format!(
                "this API key is missing the {required} scope"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(scopes: Vec<Scope>) -> UserContext {
        UserContext::authenticated(UserId::new(), "alex".into(), ApiKeyId::new(), scopes)
    }

    #[test]
    fn carries_the_authenticated_identity() {
        let user_id = UserId::new();
        let key_id = ApiKeyId::new();
        let ctx = UserContext::authenticated(user_id, "alex".into(), key_id, vec![Scope::Read]);

        assert_eq!(ctx.user_id(), user_id);
        assert_eq!(ctx.handle(), "alex");
        assert_eq!(ctx.key_id(), Some(key_id));
    }

    #[test]
    fn require_passes_for_a_held_scope() {
        assert!(context(vec![Scope::Read]).require(Scope::Read).is_ok());
    }

    #[test]
    fn require_rejects_a_missing_scope_by_name() {
        let err = context(vec![Scope::Read])
            .require(Scope::Write)
            .unwrap_err();
        assert!(matches!(err, RaError::Forbidden(_)), "got {err:?}");
        assert!(err.to_string().contains("write"), "got {err}");
    }

    #[test]
    fn admin_satisfies_every_requirement() {
        let ctx = context(vec![Scope::Admin]);
        assert!(ctx.require(Scope::Read).is_ok());
        assert!(ctx.require(Scope::Write).is_ok());
    }

    #[test]
    fn the_unauthenticated_context_has_no_key_but_full_access() {
        let ctx = UserContext::unauthenticated(UserId::new(), "default".into());
        assert_eq!(ctx.key_id(), None);
        assert!(ctx.require(Scope::Write).is_ok());
    }

    #[test]
    fn a_background_context_can_read_and_write_but_is_not_admin() {
        // The ingestion worker runs under this. Granting it Admin would
        // make the widest-privileged code in the process the part that
        // runs unattended on model output.
        let ctx = UserContext::background(UserId::new(), "alex".into());

        assert!(ctx.require(Scope::Read).is_ok());
        assert!(ctx.require(Scope::Write).is_ok());
        assert!(
            ctx.require(Scope::Admin).is_err(),
            "a background job has no business administering keys"
        );
        assert_eq!(ctx.key_id(), None, "audit must distinguish pipeline writes");
    }
}
