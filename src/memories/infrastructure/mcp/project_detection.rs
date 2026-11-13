//! Which project the `recuerdos-ai mcp` shim is running in
//! (ai-specs/memory-structure-plan.md §6).
//!
//! The harness starts the shim inside the folder it works in, so the shim can
//! tell the daemon "this is quill" without the agent typing it. In order:
//!
//! 1. `RECUERDOS_AI_PROJECT`, when set (an override in the harness config);
//! 2. the repository's `origin` remote, normalised (`github.com/acme/quill`),
//!    named after its last segment — the same for every clone and worktree;
//! 3. the repository's top-level folder name, for a repository with no remote.
//!
//! Outside a git repository there is no project: a home directory is not one,
//! and guessing one would file memories under a project that does not exist.

use crate::memories::application::structured_memory_saver::DetectedProject;
use std::path::Path;
use std::process::Command;

pub const PROJECT_ENV: &str = "RECUERDOS_AI_PROJECT";

/// The project for the folder the shim was started in, if any.
pub fn detect_project(folder: &Path) -> Option<DetectedProject> {
    if let Ok(name) = std::env::var(PROJECT_ENV)
        && !name.trim().is_empty()
    {
        return Some(DetectedProject {
            name: name.trim().to_string(),
            alias: None,
        });
    }
    if let Some(remote) =
        git(folder, &["remote", "get-url", "origin"]).and_then(|url| normalize_remote(&url))
    {
        return Some(DetectedProject {
            name: name_of(&remote),
            alias: Some(remote),
        });
    }
    let top = git(folder, &["rev-parse", "--show-toplevel"])?;
    let folder_name = Path::new(&top).file_name()?.to_str()?;
    Some(DetectedProject {
        name: slug(folder_name),
        alias: None,
    })
}

fn git(folder: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(folder)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// `git@github.com:Acme/quill.git`, `https://user@github.com/acme/quill`,
/// `ssh://git@github.com:22/acme/quill.git` → `github.com/acme/quill`.
pub fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest.to_string(),
        // scp-like `git@host:owner/repo`.
        None => url.replacen(':', "/", 1),
    };
    let rest = rest
        .rsplit_once('@')
        .map_or(rest.as_str(), |(_, host)| host);
    let (host, path) = rest.split_once('/')?;
    let host = host.split(':').next()?.to_lowercase();
    let path = path
        .trim_matches('/')
        .trim_end_matches(".git")
        .to_lowercase();
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}"))
}

/// The project name a remote gives: its last path segment.
fn name_of(remote: &str) -> String {
    slug(remote.rsplit('/').next().unwrap_or(remote))
}

/// A folder or repository name as a project name: lowercase, with anything
/// outside `a-z 0-9 . _ -` turned into `-`.
fn slug(raw: &str) -> String {
    raw.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || ".-_".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_normalise_to_host_and_path() {
        for url in [
            "git@github.com:Acme/quill.git",
            "https://github.com/acme/quill.git",
            "https://user@github.com/acme/quill",
            "ssh://git@github.com:22/acme/quill.git",
            "https://github.com/acme/quill/",
        ] {
            assert_eq!(
                normalize_remote(url).as_deref(),
                Some("github.com/acme/quill"),
                "{url}"
            );
        }
        assert_eq!(
            normalize_remote("git@gitlab.example.com:group/sub/quill.git").as_deref(),
            Some("gitlab.example.com/group/sub/quill")
        );
        assert_eq!(normalize_remote("not a url"), None);
    }

    #[test]
    fn a_remote_names_its_project_after_its_last_segment() {
        assert_eq!(name_of("github.com/acme/quill"), "quill");
        assert_eq!(name_of("github.com/acme/My Repo"), "my-repo");
        assert_eq!(slug("Recuerdos AI"), "recuerdos-ai");
    }

    #[test]
    fn outside_a_repository_there_is_no_project() {
        let folder = tempfile::tempdir().unwrap();
        // The env override is process-wide; only assert when it is unset.
        if std::env::var(PROJECT_ENV).is_err() {
            assert_eq!(detect_project(folder.path()), None);
        }
    }
}
