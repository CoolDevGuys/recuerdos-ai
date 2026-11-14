//! Who "the user" is.
//!
//! A store has one owner, so in it "the user" always means that person —
//! whichever harness wrote the memory. Agents write it that way ("The user
//! prefers…"), but a memory can also name them ("Alejandro Romero is a
//! backend engineer…"), and without knowing the name nothing tells that
//! memory from one about somebody else. The owner's names say it.
//!
//! [`self_subject`] is the one check: does this memory *begin* with the user
//! as its subject? Deliberately narrow — the subject must be followed by a
//! word that makes it a statement about them ("is", "works", "prefers"…) —
//! because its callers act on a yes: a save is refused, a pipeline candidate
//! is re-filed, a stored memory goes to review. "The user's sister Ana…" is
//! about Ana, and "The user interface is React" is about a codebase; neither
//! matches.

use crate::identity::domain::user_context::UserContext;
use crate::shared::error::{RaError, Result};

/// The most names one user may give.
pub const MAX_SELF_NAMES: usize = 8;

/// The longest single name.
pub const MAX_SELF_NAME_LEN: usize = 64;

/// The user's names, first the one they go by in full. Trimmed, with
/// case-insensitive repeats removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelfNames(Vec<String>);

impl SelfNames {
    pub fn parse(raw: &[String]) -> Result<Self> {
        let mut names: Vec<String> = Vec::new();
        for name in raw {
            let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                continue;
            }
            if name.chars().count() > MAX_SELF_NAME_LEN {
                return Err(RaError::Validation(format!(
                    "a name is at most {MAX_SELF_NAME_LEN} characters"
                )));
            }
            if name.chars().any(char::is_control) {
                return Err(RaError::Validation(
                    "a name may not contain control characters".to_string(),
                ));
            }
            if !names.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
                names.push(name);
            }
        }
        if names.len() > MAX_SELF_NAMES {
            return Err(RaError::Validation(format!(
                "at most {MAX_SELF_NAMES} names"
            )));
        }
        Ok(Self(names))
    }

    /// Rebuilds stored names without re-validating.
    pub fn from_stored(names: Vec<String>) -> Self {
        Self(names)
    }

    pub fn names(&self) -> &[String] {
        &self.0
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// One line for a prompt or the profile: `The user is Alejandro Romero
    /// (also called Alejandro, Alex).` `None` with no names.
    pub fn sentence(&self) -> Option<String> {
        let (first, rest) = self.0.split_first()?;
        Some(if rest.is_empty() {
            format!("The user is {first}.")
        } else {
            format!("The user is {first} (also called {}).", rest.join(", "))
        })
    }
}

/// Where a user's names are kept.
pub trait SelfNameStore: Send + Sync {
    /// Their names; none until they give some.
    fn get(&self, context: &UserContext) -> Result<SelfNames>;

    /// Replaces them; empty clears them.
    fn set(&self, context: &UserContext, names: &SelfNames) -> Result<()>;
}

/// How a memory names the user as its subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfSubject {
    /// "The user is…", "User prefers…".
    TheUser,
    /// One of their names, as written in the memory.
    Named(String),
}

/// Words that, right after the subject, make the sentence a statement about
/// that person. An allowlist on purpose: a word missing here costs a memory
/// that is not caught, a wrong one costs a refused save.
const PREDICATES: &[&str] = &[
    // to be, to have, modals and negations
    "is",
    "was",
    "has",
    "had",
    "isn't",
    "wasn't",
    "hasn't",
    "can",
    "cannot",
    "can't",
    "could",
    "will",
    "would",
    "should",
    "does",
    "doesn't",
    "did",
    "didn't",
    "used",
    "never",
    "always",
    "usually",
    "often",
    "also",
    "currently",
    "recently",
    "still",
    "now",
    "mostly",
    // what people do and are
    "works",
    "worked",
    "lives",
    "lived",
    "prefers",
    "preferred",
    "likes",
    "liked",
    "loves",
    "dislikes",
    "hates",
    "enjoys",
    "wants",
    "needs",
    "knows",
    "speaks",
    "uses",
    "owns",
    "drives",
    "plays",
    "runs",
    "reads",
    "writes",
    "cooks",
    "travels",
    "studies",
    "studied",
    "graduated",
    "moved",
    "grew",
    "joined",
    "left",
    "leads",
    "led",
    "manages",
    "managed",
    "specializes",
    "specialises",
    "specialized",
    "specialised",
    "became",
    "holds",
    "earned",
    "built",
    "builds",
    "started",
    "founded",
    "teaches",
    "taught",
    "practices",
    "practises",
    "believes",
    "follows",
    "avoids",
    "eats",
    "drinks",
    "sleeps",
    "wakes",
    "trains",
    "codes",
    "programs",
    "develops",
];

/// Whether `content` begins with the user as its subject.
pub fn self_subject(content: &str, names: &SelfNames) -> Option<SelfSubject> {
    let text = content.trim_start();
    let lower = text.to_lowercase();

    for lead in ["the user", "user"] {
        if lower
            .strip_prefix(lead)
            .and_then(after_subject)
            .is_some_and(starts_with_predicate)
        {
            return Some(SelfSubject::TheUser);
        }
    }

    // Longest first, so "Alejandro Romero" is matched before "Alejandro".
    let mut names: Vec<&String> = names.names().iter().collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.chars().count()));
    for name in names {
        let lowered = name.to_lowercase();
        // "Alejandro, a backend engineer, …" or "Alejandro is…"; never
        // "Alejandro's sister" or "Alejandrina".
        let Some(rest) = lower.strip_prefix(&lowered).and_then(after_subject) else {
            continue;
        };
        if starts_with_predicate(rest) || rest.starts_with("a ") || rest.starts_with("an ") {
            let original = text.get(..lowered.len()).unwrap_or(name.as_str());
            return Some(SelfSubject::Named(original.to_string()));
        }
    }
    None
}

/// What follows a subject that ends a word: an optional comma, then a space.
fn after_subject(rest: &str) -> Option<&str> {
    rest.strip_prefix(',').unwrap_or(rest).strip_prefix(' ')
}

fn starts_with_predicate(rest: &str) -> bool {
    let word = rest
        .split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap_or("");
    PREDICATES.contains(&word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alejandro() -> SelfNames {
        SelfNames::parse(&[
            "Alejandro Romero".to_string(),
            "Alejandro".to_string(),
            "Alex".to_string(),
        ])
        .unwrap()
    }

    #[test]
    fn the_user_as_subject_is_the_owner() {
        let none = SelfNames::default();
        for content in [
            "The user is an expert in PHP and also proficient in Python.",
            "The user works at Kau in Berlin.",
            "the user prefers dark roast coffee",
            "User lives in Berlin",
            "The user, currently, works remotely",
        ] {
            assert_eq!(
                self_subject(content, &none),
                Some(SelfSubject::TheUser),
                "{content}"
            );
        }
    }

    #[test]
    fn someone_or_something_of_the_users_is_not_the_user() {
        let names = alejandro();
        for content in [
            "The user's sister Ana is allergic to nuts",
            "The user interface is built with React",
            "The user table has a unique email column",
            "Alejandro's sister lives in Madrid",
            "Alex Turner is the singer of the Arctic Monkeys",
            "Alexandra is the user's manager",
            "Ana told the user about the meeting",
        ] {
            assert_eq!(self_subject(content, &names), None, "{content}");
        }
    }

    #[test]
    fn a_name_of_the_users_as_subject_is_the_owner() {
        let names = alejandro();
        assert_eq!(
            self_subject(
                "Alejandro Romero is a Senior Backend Software Engineer based in Berlin.",
                &names
            ),
            Some(SelfSubject::Named("Alejandro Romero".to_string()))
        );
        assert_eq!(
            self_subject("alex prefers tabs", &names),
            Some(SelfSubject::Named("alex".to_string()))
        );
        assert_eq!(
            self_subject("Alejandro, a backend engineer, lives in Berlin", &names),
            Some(SelfSubject::Named("Alejandro".to_string()))
        );
        assert_eq!(
            self_subject(
                "Alejandro Romero is a backend engineer",
                &SelfNames::default()
            ),
            None,
            "without the names a name is just a name"
        );
    }

    #[test]
    fn names_are_trimmed_deduplicated_and_bounded() {
        let names = SelfNames::parse(&[
            "  Alejandro   Romero ".to_string(),
            "alejandro romero".to_string(),
            "".to_string(),
            "Alex".to_string(),
        ])
        .unwrap();
        assert_eq!(names.names(), ["Alejandro Romero", "Alex"]);
        assert_eq!(
            names.sentence().as_deref(),
            Some("The user is Alejandro Romero (also called Alex).")
        );
        assert!(SelfNames::parse(&vec!["x".to_string(); 1]).is_ok());
        let many: Vec<String> = (0..9).map(|i| format!("name{i}")).collect();
        assert!(SelfNames::parse(&many).is_err());
        assert!(SelfNames::parse(&["a\u{7}b".to_string()]).is_err());
        assert_eq!(SelfNames::default().sentence(), None);
    }
}
