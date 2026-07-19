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

    pub fn allows(&self, required: Scope) -> bool {
        self.scopes.contains(&Scope::Admin) || self.scopes.contains(&required)
    }

    /// Gate for a scoped operation: `Ok(())` or a `Forbidden` error
    /// naming the scope that was missing.
    pub fn require(&self, required: Scope) -> Result<()> {
        if self.allows(required) {
            Ok(())
        } else {
            Err(RaError::Forbidden(format!(
                "this API key is missing the {required} scope"
            )))
        }
    }

    /// A context for tests in other contexts, which cannot call the
    /// private constructors. Compiled out of release builds entirely, so
    /// it can never widen the production surface.
    #[cfg(test)]
    pub fn for_test(user_id: UserId) -> Self {
        Self {
            user_id,
            handle: "test-user".to_string(),
            key_id: None,
            scopes: vec![Scope::Admin],
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
}
