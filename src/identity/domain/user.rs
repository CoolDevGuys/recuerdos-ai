//! A user: the owner of memories, and the unit of isolation.
//!
//! `handle` is the human-facing identifier used everywhere on the CLI
//! (`--user alex`) and is unique. `email` is optional metadata — a
//! self-hoster running this for themselves shouldn't be forced to invent
//! one. (project-plan.md §8 lists `email` as the identifier; a separate
//! handle is a deliberate refinement, since the CLI needs a short unique
//! name and emails change.)

use crate::shared::error::{RaError, Result};
use crate::shared::ids::UserId;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    id: UserId,
    handle: String,
    email: Option<String>,
    created_at: DateTime<Utc>,
}

impl User {
    /// Creates a new user, validating the handle. Handles are lowercased
    /// so `Alex` and `alex` can't become two users that look identical in
    /// a terminal.
    pub fn create(handle: &str, email: Option<&str>, now: DateTime<Utc>) -> Result<Self> {
        let handle = normalize_handle(handle)?;
        let email = match email {
            Some(email) => Some(validate_email(email)?),
            None => None,
        };

        Ok(Self {
            id: UserId::new(),
            handle,
            email,
            created_at: now,
        })
    }

    /// Rebuilds a user from storage. Bypasses validation on purpose:
    /// rows already in the database are historical fact, and refusing to
    /// load them because today's rules are stricter would lock a user out
    /// of their own data.
    pub fn from_stored(
        id: UserId,
        handle: String,
        email: Option<String>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            handle,
            email,
            created_at,
        }
    }

    pub fn id(&self) -> UserId {
        self.id
    }

    pub fn handle(&self) -> &str {
        &self.handle
    }

    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
}

const MAX_HANDLE_LEN: usize = 64;

fn normalize_handle(handle: &str) -> Result<String> {
    let handle = handle.trim().to_ascii_lowercase();

    if handle.is_empty() {
        return Err(RaError::Validation("handle is empty".to_string()));
    }
    if handle.len() > MAX_HANDLE_LEN {
        return Err(RaError::Validation(format!(
            "handle is longer than {MAX_HANDLE_LEN} characters"
        )));
    }
    if !handle
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(RaError::Validation(format!(
            "handle {handle:?} may only contain letters, digits, '-', '_' and '.'"
        )));
    }

    Ok(handle)
}

fn validate_email(email: &str) -> Result<String> {
    let email = email.trim();

    // Deliberately minimal: full RFC 5322 validation is a famous rabbit
    // hole, and this field is optional metadata we never send mail to.
    if email.is_empty() {
        return Err(RaError::Validation("email is empty".to_string()));
    }
    if !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(RaError::Validation(format!(
            "email {email:?} is not a valid address"
        )));
    }

    Ok(email.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    #[test]
    fn creates_a_user_with_a_normalized_handle() {
        let user = User::create("  Alex  ", None, now()).unwrap();
        assert_eq!(user.handle(), "alex");
        assert_eq!(user.email(), None);
        assert_eq!(user.created_at(), now());
    }

    #[test]
    fn accepts_an_optional_email() {
        let user = User::create("alex", Some("alex@example.com"), now()).unwrap();
        assert_eq!(user.email(), Some("alex@example.com"));
    }

    #[test]
    fn rejects_an_empty_handle() {
        assert!(User::create("   ", None, now()).is_err());
    }

    #[test]
    fn rejects_handles_with_unsafe_characters() {
        for handle in ["alex smith", "alex/../root", "alex@host", "a:b"] {
            assert!(
                User::create(handle, None, now()).is_err(),
                "{handle:?} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_handles_with_safe_punctuation() {
        for handle in ["alex", "alex-1", "alex_1", "alex.dev", "a1"] {
            assert!(
                User::create(handle, None, now()).is_ok(),
                "{handle:?} should be accepted"
            );
        }
    }

    #[test]
    fn rejects_an_overlong_handle() {
        let handle = "a".repeat(MAX_HANDLE_LEN + 1);
        assert!(User::create(&handle, None, now()).is_err());
    }

    #[test]
    fn rejects_a_malformed_email() {
        for email in ["", "alex", "@example.com", "alex@"] {
            assert!(
                User::create("alex", Some(email), now()).is_err(),
                "{email:?} should be rejected"
            );
        }
    }

    #[test]
    fn distinct_users_get_distinct_ids() {
        let a = User::create("alex", None, now()).unwrap();
        let b = User::create("sam", None, now()).unwrap();
        assert_ne!(a.id(), b.id());
    }
}
