//! What an API key is allowed to do.
//!
//! Scopes are deliberately coarse. `Read` and `Write` are independent —
//! holding `Write` does *not* imply `Read`, because a write-only ingestion
//! key (an agent that saves memories but must never read them back) is a
//! real and useful thing to hand out. `Admin` implies everything.

use crate::shared::error::{RaError, Result};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    Read,
    Write,
    Admin,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::Admin => "admin",
        }
    }

    /// Parses a comma-separated scope list (`"read,write"`), rejecting
    /// unknown names and de-duplicating. An empty list is an error: a key
    /// that can do nothing is always a mistake, never an intent.
    pub fn parse_list(raw: &str) -> Result<Vec<Scope>> {
        let mut scopes: Vec<Scope> = Vec::new();

        for part in raw.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let scope: Scope = part.parse()?;
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
        }

        if scopes.is_empty() {
            return Err(RaError::Validation(
                "at least one scope is required (read, write, admin)".to_string(),
            ));
        }

        scopes.sort();
        Ok(scopes)
    }

    /// Renders a scope list back into its canonical `"read,write"` form.
    pub fn join(scopes: &[Scope]) -> String {
        scopes
            .iter()
            .map(Scope::as_str)
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Scope {
    type Err = RaError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "read" => Ok(Scope::Read),
            "write" => Ok(Scope::Write),
            "admin" => Ok(Scope::Admin),
            other => Err(RaError::Validation(format!(
                "unknown scope {other:?} (expected read, write or admin)"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_comma_separated_list() {
        assert_eq!(
            Scope::parse_list("read,write").unwrap(),
            vec![Scope::Read, Scope::Write]
        );
    }

    #[test]
    fn tolerates_whitespace_and_case_and_duplicates() {
        assert_eq!(
            Scope::parse_list(" READ , read,  Write ").unwrap(),
            vec![Scope::Read, Scope::Write]
        );
    }

    #[test]
    fn rejects_unknown_scopes() {
        let err = Scope::parse_list("read,superuser").unwrap_err();
        assert!(matches!(err, RaError::Validation(_)), "got {err:?}");
        assert!(err.to_string().contains("superuser"));
    }

    #[test]
    fn rejects_an_empty_list() {
        assert!(Scope::parse_list("").is_err());
        assert!(Scope::parse_list("  , ,").is_err());
    }

    #[test]
    fn join_round_trips_through_parse_list() {
        let scopes = Scope::parse_list("admin,read").unwrap();
        assert_eq!(Scope::parse_list(&Scope::join(&scopes)).unwrap(), scopes);
    }
}
