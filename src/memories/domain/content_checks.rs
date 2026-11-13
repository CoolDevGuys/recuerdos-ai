//! What the text of a memory can give away, checked without a model
//! (ai-specs/memory-structure-plan.md §4, rules R7 and R9).
//!
//! Both checks are tuned for **precision**: a refused save the user meant is
//! worse than a missed case, which the model layer (R7, from S2) or the user
//! catches later. So each fires only on a clear signal, and the labelled sets
//! in the tests hold them to at least 95% precision.

/// Phrases that bind a sentence to one codebase or project (R7). Matched on
/// word boundaries, case-insensitively. Deliberately narrow: "the code"
/// alone is a personal preference as often as a project's rule ("prefers the
/// code to have tests"), so it is not here.
const PROJECT_PHRASES: &[&str] = &[
    // English
    "the codebase",
    "this codebase",
    "our codebase",
    "this repo",
    "this repository",
    "our repo",
    "our repository",
    "this project",
    "our project",
    "in this project",
    "in this repo",
    "we use",
    "we're using",
    "we are using",
    "here we use",
    // Spanish
    "el código base",
    "la base de código",
    "este código",
    "nuestro código",
    "este repo",
    "este repositorio",
    "nuestro repo",
    "nuestro repositorio",
    "este proyecto",
    "nuestro proyecto",
    "en este proyecto",
    "en este repo",
    "aquí usamos",
    "usamos",
];

/// The phrase that ties this text to one project, if there is one.
pub fn project_phrase(content: &str) -> Option<&'static str> {
    let text = content.to_lowercase();
    PROJECT_PHRASES
        .iter()
        .copied()
        .find(|phrase| contains_words(&text, phrase))
}

/// Whether `phrase` occurs in `text` with a word boundary on both sides.
fn contains_words(text: &str, phrase: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(phrase).any(|(start, matched)| {
        let before = text[..start].chars().next_back();
        let after = text[start + matched.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// Which kind of credential the text seems to contain (R9), for the reply.
/// `None` when it looks clean.
pub fn secret_in(content: &str) -> Option<&'static str> {
    if content.contains("-----BEGIN") && content.contains("PRIVATE KEY") {
        return Some("a private key");
    }
    let tokens: Vec<&str> = content
        .split(|c: char| c.is_whitespace() || "\"'`,;()[]{}<>".contains(c))
        .filter(|token| !token.is_empty())
        .collect();

    for token in &tokens {
        if let Some(found) = known_token(token) {
            return Some(found);
        }
        if url_with_password(token) {
            return Some("a password inside a URL");
        }
    }
    if let Some(found) = assigned_credential(&tokens) {
        return Some(found);
    }
    tokens
        .iter()
        .flat_map(|token| token.split(['/', '?', '&', '=', '#', ':', '@']))
        .any(looks_random)
        .then_some("a long random-looking string, like a key or token")
}

/// Tokens with a provider's prefix.
fn known_token(token: &str) -> Option<&'static str> {
    let rest_len = |prefix: &str| token.len().saturating_sub(prefix.len());
    let body_is = |prefix: &str, min: usize| {
        token.starts_with(prefix)
            && rest_len(prefix) >= min
            && token[prefix.len()..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    if token.len() == 20
        && token.starts_with("AKIA")
        && token[4..]
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return Some("an AWS access key");
    }
    for prefix in ["ghp_", "gho_", "ghs_", "ghu_", "ghr_", "github_pat_"] {
        if body_is(prefix, 20) {
            return Some("a GitHub token");
        }
    }
    if body_is("glpat-", 20) {
        return Some("a GitLab token");
    }
    if body_is("sk-", 20) {
        return Some("an API key");
    }
    for prefix in ["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-"] {
        if body_is(prefix, 10) {
            return Some("a Slack token");
        }
    }
    if body_is("AIza", 30) {
        return Some("a Google API key");
    }
    if token.starts_with("eyJ") && token.matches('.').count() == 2 && token.len() >= 30 {
        return Some("a JWT");
    }
    None
}

/// Words that name a credential, in English and Spanish.
const CREDENTIAL_WORDS: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "pwd",
    "token",
    "secret",
    "apikey",
    "api_key",
    "api-key",
    "contraseña",
    "clave",
];

/// `password: hunter2`, `token=abc123`, `password is hunter2`: a credential
/// word followed by something shaped like its value. "The password is in
/// 1Password" and "the token is stored in the vault" are not: the word after
/// "is" carries no digit or symbol.
fn assigned_credential(tokens: &[&str]) -> Option<&'static str> {
    for (i, token) in tokens.iter().enumerate() {
        let lower = token.to_lowercase();
        // `password=hunter2` / `password:hunter2` in one token.
        if let Some((name, value)) = lower.split_once(['=', ':'])
            && is_credential_word(name)
            && value_like(value)
        {
            return Some("a password or token");
        }
        let name = lower.trim_end_matches([':', '=']);
        if !is_credential_word(name) {
            continue;
        }
        let separated = lower.ends_with(':') || lower.ends_with('=');
        if separated && tokens.get(i + 1).copied().is_some_and(value_like) {
            return Some("a password or token");
        }
        // "password is X", and "contraseña del router es X": a few words may
        // sit between the credential word and its "is" / "es" / ":".
        let window = tokens.iter().skip(i + 1).take(4).enumerate();
        for (offset, word) in window {
            let word = word.to_lowercase();
            if matches!(word.as_str(), ":" | "=" | "is" | "es" | "was" | "era") {
                let value = tokens.get(i + 2 + offset).copied();
                if value.is_some_and(value_like) {
                    return Some("a password or token");
                }
                break;
            }
        }
    }
    None
}

/// `https://user:pass@host`: a URL carrying its own password.
fn url_with_password(token: &str) -> bool {
    let Some((_, rest)) = token.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    authority
        .rsplit_once('@')
        .and_then(|(userinfo, _)| userinfo.split_once(':'))
        .is_some_and(|(user, password)| !user.is_empty() && !password.is_empty())
}

fn is_credential_word(word: &str) -> bool {
    CREDENTIAL_WORDS.contains(&word)
}

/// A value a credential might have: no spaces (it is one token), at least 6
/// characters, and a digit or a symbol in it, so ordinary words ("stored",
/// "rotated") are not taken for one.
fn value_like(value: &str) -> bool {
    let value = value.trim_end_matches(['.', ',', ';']);
    value.chars().count() >= 6
        && value
            .chars()
            .any(|c| c.is_ascii_digit() || (!c.is_alphanumeric() && c != '-'))
}

/// At least 32 characters of letters and digits mixing upper case, lower
/// case and digits: a key or token, not a word, a UUID (no upper case) or a
/// commit hash (no upper case).
fn looks_random(token: &str) -> bool {
    token.len() >= 32
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '+')
        && token.chars().any(|c| c.is_ascii_uppercase())
        && token.chars().any(|c| c.is_ascii_lowercase())
        && token.chars().any(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Precision and recall of a check over a labelled set.
    fn measure(check: impl Fn(&str) -> bool, positives: &[&str], negatives: &[&str]) -> (f32, f32) {
        let true_positive = positives.iter().filter(|text| check(text)).count();
        let false_positive = negatives.iter().filter(|text| check(text)).count();
        for text in negatives.iter().filter(|text| check(text)) {
            eprintln!("false positive: {text}");
        }
        for text in positives.iter().filter(|text| !check(text)) {
            eprintln!("missed: {text}");
        }
        let precision = true_positive as f32 / (true_positive + false_positive).max(1) as f32;
        let recall = true_positive as f32 / positives.len() as f32;
        (precision, recall)
    }

    /// Texts bound to one project (R7 should fire), English and Spanish.
    const PROJECT_BOUND: &[&str] = &[
        "The codebase uses Python 3.12+ and is fully type-hinted",
        "This repo forbids default exports",
        "In this project we never mock the database",
        "We use pnpm workspaces for the frontend packages",
        "Our codebase keeps one public class per file",
        "This repository deploys from the main branch only",
        "We're using Vitest instead of Jest",
        "Releases of this project are tagged by CI",
        "Here we use feature folders: domain, application, ui",
        "Our project targets Node 22",
        "In this repo, migrations are forward-only",
        "This codebase has no untyped public functions",
        "We are using Tailwind v4 with the Vite plugin",
        "Este proyecto usa Python 3.12",
        "En este proyecto no se mockea la base de datos",
        "Usamos pnpm en todos los paquetes",
        "Aquí usamos Vitest para los tests unitarios",
        "Este repositorio publica desde main",
        "Nuestro código no tiene exports por defecto",
        "En este repo las migraciones solo van hacia delante",
        "El código base usa arquitectura hexagonal",
        "Nuestro proyecto despliega en un VPS",
    ];

    /// Texts true of the user or their world (R7 must stay quiet).
    const NOT_PROJECT_BOUND: &[&str] = &[
        "The user prefers mocks over real dependencies in unit tests",
        "Prefers pnpm for every JavaScript project",
        "Always writes the test first",
        "Never encodes meaning by colour alone",
        "Prefers small pull requests that each ship something",
        "Prefers the code to be formatted by the tool, never by hand",
        "The NAS backups are in /mnt/tank/backups",
        "The homelab is reached through the bastion host as user pi",
        "Dentist appointment on 14 December at 10:00",
        "Drinks green tea, not coffee",
        "Comfortable with Rust, TypeScript and SQL",
        "Wants every project to have a README",
        "Prefers projects that use strict TypeScript",
        "Learned that flaky tests usually hide a shared clock",
        "Reads science fiction in the evenings",
        "Runs on weekends, usually trail running",
        "Usually reviews code in the morning",
        "Prefiere los mocks en los tests unitarios",
        "Siempre escribe primero el test",
        "Prefiere que el código tenga tests",
        "Las copias de seguridad del NAS están en /mnt/tank/backups",
        "Cita con el dentista el 14 de diciembre",
        "Le gusta el té verde",
        "Prefiere pull requests pequeñas",
        "Uses a split keyboard",
        "Thinks most reuse is premature",
        "The weekly sync with Maya is on Mondays",
        "Wesley uses vim",
        "Prefers reproducible builds in every repository",
    ];

    #[test]
    fn the_scope_check_is_precise_in_english_and_spanish() {
        let (precision, recall) = measure(
            |text| project_phrase(text).is_some(),
            PROJECT_BOUND,
            NOT_PROJECT_BOUND,
        );
        assert!(precision >= 0.95, "precision {precision}");
        assert!(recall >= 0.9, "recall {recall}");
    }

    #[test]
    fn phrases_match_on_word_boundaries_only() {
        assert_eq!(project_phrase("This repo is small"), Some("this repo"));
        assert_eq!(project_phrase("this reporter"), None);
        assert_eq!(project_phrase("Wesley uses vim"), None, "not \"we use\"");
        assert_eq!(project_phrase("¿Usamos Rust?"), Some("usamos"));
    }

    /// Texts carrying a credential (R9 should fire).
    ///
    /// Token-shaped fixtures are split with `concat!`: the compiled strings are
    /// the same, but the source holds no contiguous token for a secret scanner
    /// to stop a push over.
    const SECRETS: &[&str] = &[
        "The server password is hunter2!",
        "password: Tr0ub4dor&3",
        "Root password=correct-horse-9",
        "token: 8f3kd02mf93kd02",
        concat!(
            "The deploy key is gh",
            "p_aB3dE6fGh9iJk2LmN5oPq8RsT1uVw4XyZ7"
        ),
        concat!("API key sk", "-ant-api03-abcdEFGHijklMNOPqrstUVWX"),
        concat!("Use AK", "IAIOSFODNN7EXAMPLE for the bucket"),
        concat!("Slack bot token xo", "xb-123456789012-abcdefghij"),
        concat!("GitLab: gl", "pat-aB3dE6fGh9iJk2LmN5oP"),
        concat!("--", "---BEGIN OPENSSH PRIVATE KEY----- b3BlbnNzaC1rZXk"),
        concat!(
            "Auth header: ey",
            "JhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
        ),
        "The webhook secret is Zr8tK2mQ9vX4nB7wL1pS6dF3hJ0yC5gA",
        "La contraseña del router es Gato#2024",
        "clave: Lu1sR0dr1guez!",
        concat!("Maps key AI", "zaSyD4kE9fG2hJ5lM8nP1qR3sT6uV9wX0yZ2a"),
        "Connect with https://user:N7kQ2xP9mL4vB8rT1wZ6cY3hJ5gD0fS@db.example.com",
    ];

    /// Texts about credentials without one in them (R9 must stay quiet).
    const NOT_SECRETS: &[&str] = &[
        "The password is in 1Password, under Homelab",
        "The token is stored in the vault, never in the repo",
        "Secrets live in environment variables or a secret manager",
        "Authentication tokens must be short-lived and support rotation",
        "SSH into the NAS with the key in ~/.ssh/id_ed25519",
        "The homelab is reached through the bastion host as user pi",
        "The API key for staging is in the team's Bitwarden",
        "Rotate the database password every 90 days",
        "Commit 3f9a2c1d8e7b6a5f4c3d2e1f0a9b8c7d6e5f4a3b introduced the bug",
        "The memory id is 019f7c5a-3b2e-7c1d-9a8b-1c2d3e4f5a6b",
        "The design doc is at https://docs.example.com/design/memory-structure-plan",
        "La contraseña está en el gestor de contraseñas",
        "Prefers passphrases over passwords",
        "The password policy requires twelve characters",
        "Uses pnpm, Vitest and Playwright",
        "The bucket is s3://quill-attachments-production",
        "Restart the daemon with systemctl restart recuerdos-ai",
        "The token expires after 15 minutes",
        "Use the sk- prefix to spot an API key in logs",
        "Our CI image is ghcr.io/cooldevguys/recuerdos-ai:0.5.0-rc5",
    ];

    #[test]
    fn the_secret_check_is_precise() {
        let (precision, recall) = measure(|text| secret_in(text).is_some(), SECRETS, NOT_SECRETS);
        assert!(precision >= 0.95, "precision {precision}");
        assert!(recall >= 0.9, "recall {recall}");
    }

    #[test]
    fn the_secret_check_says_what_it_saw() {
        assert_eq!(
            secret_in("password: Tr0ub4dor&3"),
            Some("a password or token")
        );
        assert_eq!(
            secret_in("ghp_aB3dE6fGh9iJk2LmN5oPq8RsT1uVw4XyZ7"),
            Some("a GitHub token")
        );
        assert_eq!(secret_in("Prefers pnpm"), None);
    }
}
