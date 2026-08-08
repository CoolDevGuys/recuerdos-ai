//! The canonical form of an entity's name — the graph's node identity.
//!
//! # Why this is the load-bearing piece of the graph
//!
//! A hop is only as good as the join it hops over. If the writer files an
//! edge under `Fly.io` and the reader seeds a hop from `fly.io`, the two
//! never meet and the graph is silent — worse than absent, because it
//! *looks* like it is working. So both sides pass every name through this
//! one function, and the graph's identity is whatever it returns.
//!
//! # What it does, and deliberately does not, collapse
//!
//! It normalises the differences that are pure spelling — case,
//! surrounding and repeated whitespace, a trailing full stop or comma, a
//! possessive `'s`:
//!
//! ```text
//! "Fly.io"  "fly.io"  " Fly.io "  "FLY.IO"  "Fly.io."  "Fly.io's"  →  "fly.io"
//! ```
//!
//! It does **not** unify names that differ in their letters — `Fly` and
//! `Fly.io` and `flyio` stay three keys. Deciding those are the same
//! entity needs a curated alias table, which is out of scope here (Task
//! 7.3.1) and recorded as the known limitation: if a relational eval
//! later blames entity resolution, the alias table is the escape hatch.
//! Keeping this function pure spelling-normalisation means it can never
//! silently merge two genuinely different entities.

// Built and tested under Task 7.3.1, but not consumed by the crate until
// its callers land — the write path in 7.3.2, recall seeding in 7.3.4 —
// so a non-test build sees these as unused until then. Removed once wired.
#![allow(dead_code)]

/// A canonicalised entity name. Two names that should be one graph node
/// produce equal keys; the constructor is the only way to make one, so a
/// raw string can never be mistaken for a key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntityKey(String);

impl EntityKey {
    /// Canonicalises a raw entity name. See the module docs for exactly
    /// what is and is not collapsed.
    pub fn new(raw: &str) -> Self {
        Self(canonicalize(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A name that canonicalises to nothing (empty, whitespace, or only
    /// punctuation) is not a usable node — callers skip these rather than
    /// filing an edge under the empty key.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn canonicalize(raw: &str) -> String {
    // Lowercase first so the whitespace and suffix steps see one case.
    let lowered = raw.to_lowercase();

    // Collapse every run of whitespace to a single space and trim the
    // ends: "Meridian\t team " and "Meridian team" are one node.
    let collapsed = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s = collapsed.as_str();

    // Strip a trailing possessive before punctuation, so "Sam's" → "sam"
    // and "Fly.io's" → "fly.io". Both the straight and curly apostrophe,
    // because text arrives from both keyboards and editors.
    for possessive in ["'s", "\u{2019}s"] {
        if let Some(stripped) = s.strip_suffix(possessive) {
            s = stripped;
            break;
        }
    }

    // Strip trailing sentence punctuation only — an internal dot is part
    // of the name ("fly.io"), a trailing one is where a sentence happened
    // to end ("we moved to fly.io.").
    let trimmed = s.trim_end_matches(|c: char| {
        matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '\u{2019}')
    });

    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(raw: &str) -> String {
        EntityKey::new(raw).as_str().to_string()
    }

    #[test]
    fn spelling_variants_of_one_name_collapse_to_one_key() {
        // The table that matters: every one of these is the same node.
        let cases = [
            ("Fly.io", "fly.io"),
            ("fly.io", "fly.io"),
            ("  Fly.io  ", "fly.io"),
            ("FLY.IO", "fly.io"),
            ("Fly.io.", "fly.io"),               // trailing full stop
            ("Fly.io's", "fly.io"),              // possessive, straight quote
            ("Fly.io\u{2019}s", "fly.io"),       // possessive, curly quote
            ("Meridian  team", "meridian team"), // collapsed whitespace
            ("Meridian team!", "meridian team"), // trailing bang
            ("notifications service,", "notifications service"), // trailing comma
            ("\tHetzner\n", "hetzner"),          // surrounding whitespace
            ("Sam's", "sam"),                    // possessive on a person
        ];

        for (raw, expected) in cases {
            assert_eq!(key(raw), expected, "canonicalising {raw:?}");
        }
    }

    #[test]
    fn an_internal_dot_is_kept_but_a_trailing_one_is_not() {
        // The distinction the trailing-only strip exists to make: the dot
        // in "fly.io" is part of the name; the one after it is a sentence.
        assert_eq!(key("fly.io"), "fly.io");
        assert_eq!(key("fly.io..."), "fly.io");
    }

    #[test]
    fn names_that_differ_in_letters_stay_separate_the_known_limitation() {
        // This function normalises spelling, not identity. Unifying these
        // needs an alias table (out of scope for Task 7.3.1); pinning the
        // behaviour here means a future alias step is a deliberate change,
        // not an accident.
        assert_ne!(key("Fly"), key("Fly.io"));
        assert_ne!(key("flyio"), key("fly.io"));
    }

    #[test]
    fn a_name_that_is_only_punctuation_or_whitespace_is_empty() {
        assert!(EntityKey::new("   ").is_empty());
        assert!(EntityKey::new("...").is_empty());
        assert!(!EntityKey::new("Hetzner").is_empty());
    }
}
