//! Task 1.3 DoD: drive the real binary's `user`/`key` subcommands against
//! a tmp data dir and assert on what an operator actually sees.
//!
//! These are the commands a self-hoster runs before anything else works,
//! so they are tested the way they are used: as a process, over stdout.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::path::Path;
use tempfile::TempDir;

/// A throwaway instance: its own data dir, its own database.
struct Cli {
    data_dir: TempDir,
}

impl Cli {
    fn new() -> Self {
        Self {
            data_dir: tempfile::tempdir().expect("create tmp data dir"),
        }
    }

    fn run(&self, args: &[&str]) -> Command {
        let mut command = Command::cargo_bin("recordagent").expect("binary builds");
        command
            .args(args)
            .env("RECORDAGENT_STORAGE__PATH", self.data_dir.path())
            // Keep the ambient environment from leaking into the run.
            .env_remove("RECORDAGENT_LOG");
        command
    }

    fn add_user(&self, handle: &str) {
        self.run(&["user", "add", handle]).assert().success();
    }

    /// Issues a key and returns the token printed to stdout.
    fn issue_key(&self, handle: &str, scopes: &str) -> String {
        let output = self
            .run(&["key", "issue", "--user", handle, "--scopes", scopes])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        extract_token(&String::from_utf8(output).expect("utf-8 stdout"))
    }

    fn database_path(&self) -> std::path::PathBuf {
        self.data_dir.path().join("recordagent.db")
    }
}

fn extract_token(stdout: &str) -> String {
    stdout
        .split_whitespace()
        .find(|word| word.starts_with("ra_live_"))
        .unwrap_or_else(|| panic!("no key in output:\n{stdout}"))
        .to_string()
}

#[test]
fn creates_a_user_and_lists_it() {
    let cli = Cli::new();

    cli.run(&["user", "add", "alex", "--email", "alex@example.com"])
        .assert()
        .success()
        .stdout(contains("created user alex"));

    cli.run(&["user", "list"])
        .assert()
        .success()
        .stdout(contains("alex").and(contains("alex@example.com")));
}

#[test]
fn creating_a_database_is_enough_to_start_from_nothing() {
    let cli = Cli::new();
    assert!(!cli.database_path().exists());

    cli.add_user("alex");

    assert!(
        cli.database_path().exists(),
        "the first command should create the database"
    );
}

#[test]
fn rejects_a_duplicate_handle_with_a_readable_error() {
    let cli = Cli::new();
    cli.add_user("alex");

    cli.run(&["user", "add", "alex"])
        .assert()
        .failure()
        .stderr(contains("already exists"));
}

#[test]
fn rejects_an_invalid_handle() {
    let cli = Cli::new();

    cli.run(&["user", "add", "not a handle"])
        .assert()
        .failure()
        .stderr(contains("may only contain"));
}

#[test]
fn issues_a_key_and_shows_it_exactly_once() {
    let cli = Cli::new();
    cli.add_user("alex");

    let assertion = cli
        .run(&["key", "issue", "--user", "alex", "--scopes", "read,write"])
        .assert()
        .success()
        .stdout(contains("API key created for alex"))
        .stdout(contains("scopes: read,write"))
        .stdout(contains("only time this key is shown"));

    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    let token = extract_token(&stdout);
    assert_eq!(token.len(), "ra_live_".len() + 8 + 32, "got {token}");

    // The secret must never be recoverable afterwards: `key list` shows
    // the prefix, and nothing anywhere shows the secret again.
    let listing = cli
        .run(&["key", "list", "--user", "alex"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let listing = String::from_utf8(listing).unwrap();

    assert!(
        !listing.contains(&token),
        "the full key reappeared in `key list`:\n{listing}"
    );
    let prefix = &token["ra_live_".len().."ra_live_".len() + 8];
    assert!(listing.contains(prefix), "prefix missing from:\n{listing}");
}

#[test]
fn the_secret_is_never_written_to_the_database() {
    let cli = Cli::new();
    cli.add_user("alex");
    let token = cli.issue_key("alex", "read");

    // The strongest form of the claim: grep the raw database file. A
    // stolen database must not yield a usable key.
    let raw = std::fs::read(cli.database_path()).expect("read database");
    let secret = &token["ra_live_".len() + 8..];

    assert!(
        !contains_bytes(&raw, secret.as_bytes()),
        "the key secret was found in the database file"
    );
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn rejects_an_unknown_scope() {
    let cli = Cli::new();
    cli.add_user("alex");

    cli.run(&["key", "issue", "--user", "alex", "--scopes", "superuser"])
        .assert()
        .failure()
        .stderr(contains("superuser"));
}

#[test]
fn refuses_to_issue_a_key_for_an_unknown_user() {
    let cli = Cli::new();

    cli.run(&["key", "issue", "--user", "nobody"])
        .assert()
        .failure()
        .stderr(contains("not found"));
}

#[test]
fn revokes_a_key_by_prefix() {
    let cli = Cli::new();
    cli.add_user("alex");
    let token = cli.issue_key("alex", "read");
    let prefix = &token["ra_live_".len().."ra_live_".len() + 8];

    cli.run(&["key", "revoke", prefix])
        .assert()
        .success()
        .stdout(contains("revoked"));

    cli.run(&["key", "list", "--user", "alex"])
        .assert()
        .success()
        .stdout(contains("revoked"));
}

#[test]
fn reports_an_unknown_prefix_on_revoke() {
    let cli = Cli::new();

    cli.run(&["key", "revoke", "deadbeef"])
        .assert()
        .failure()
        .stderr(contains("no API key with prefix"));
}

#[test]
fn one_users_keys_never_appear_under_another() {
    let cli = Cli::new();
    cli.add_user("alex");
    cli.add_user("sam");
    let alex_token = cli.issue_key("alex", "read");
    let sam_token = cli.issue_key("sam", "read");

    let alex_prefix = &alex_token["ra_live_".len().."ra_live_".len() + 8];
    let sam_prefix = &sam_token["ra_live_".len().."ra_live_".len() + 8];

    cli.run(&["key", "list", "--user", "alex"])
        .assert()
        .success()
        .stdout(contains(alex_prefix).and(contains(sam_prefix).not()));

    cli.run(&["key", "list", "--user", "sam"])
        .assert()
        .success()
        .stdout(contains(sam_prefix).and(contains(alex_prefix).not()));
}

#[test]
fn state_survives_across_separate_invocations() {
    let cli = Cli::new();

    // Each command is its own process: anything that persists has to have
    // reached the database, not just some in-process cache.
    cli.add_user("alex");
    cli.issue_key("alex", "read,write");

    cli.run(&["key", "list", "--user", "alex"])
        .assert()
        .success()
        .stdout(contains("read,write").and(contains("active")));
}

#[test]
fn respects_an_explicit_config_file() {
    let config_dir = tempfile::tempdir().unwrap();
    let data_dir = config_dir.path().join("data");
    let config_path = config_dir.path().join("recordagent.toml");
    std::fs::write(
        &config_path,
        format!("[storage]\npath = \"{}\"\n", data_dir.display()),
    )
    .unwrap();

    Command::cargo_bin("recordagent")
        .unwrap()
        .args(["user", "add", "alex", "--config"])
        .arg(&config_path)
        // An env override would win over the file, so it must be absent
        // for this test to prove the file was read.
        .env_remove("RECORDAGENT_STORAGE__PATH")
        .assert()
        .success();

    assert!(
        Path::new(&data_dir).join("recordagent.db").exists(),
        "the config file's storage path was not honoured"
    );
}
