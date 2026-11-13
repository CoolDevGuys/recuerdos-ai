//! Black-box test harness: spawns the real `recuerdos-ai` binary against a
//! tmp data dir and a free port, and waits for it to become healthy before
//! handing control back to the test. Every scenario test in `tests/`
//! drives the app exactly the way a real client would — over HTTP.

// Each integration test binary compiles this module separately, so a
// helper used by one suite looks dead to the others. The alternative is
// per-suite duplication of the harness, which is worse.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

pub struct TestApp {
    pub base_url: String,
    child: Child,
    // Held for the app's lifetime so the data dir isn't cleaned up out
    // from under a running process. Also the path the CLI helpers below
    // point at, so they operate on the same database the server serves.
    data_dir: TempDir,
}

impl TestApp {
    /// Spawns `recuerdos-ai serve` on a free port with a fresh tmp data
    /// dir, and blocks (async) until `/healthz` responds or panics after
    /// a 10s timeout.
    pub async fn spawn() -> Self {
        Self::spawn_with(&[]).await
    }

    /// Spawns with extra `RECUERDOS_AI_*` settings.
    ///
    /// Config reaches the daemon only through its environment, so this is
    /// how a test runs the same binary in a different mode — pointing
    /// `[understanding]` at a mock provider, for instance, which is what
    /// makes an end-to-end pipeline test possible without a real model.
    pub async fn spawn_with(env: &[(&str, &str)]) -> Self {
        // `free_port` can only *suggest* a port: it binds 0, reads the number
        // and lets go, and a parallel suite can be handed the same number
        // before this daemon binds it. Polling `/healthz` alone would then be
        // answered by *that* suite's daemon while this one quietly fails to
        // bind and exits — and the test fails later with "connection refused"
        // once the other suite shuts down. So the harness waits for this
        // child's own `listening` line, and a child that exits first lost the
        // race and is retried on a fresh port.
        for attempt in 1..=PORT_ATTEMPTS {
            let port = free_port();
            let data_dir = tempfile::tempdir().expect("create tmp data dir");

            let mut command = Command::new(env!("CARGO_BIN_EXE_recuerdos-ai"));
            command
                .arg("serve")
                .env("RECUERDOS_AI_SERVER__HOST", "127.0.0.1")
                .env("RECUERDOS_AI_SERVER__PORT", port.to_string())
                .env("RECUERDOS_AI_STORAGE__PATH", data_dir.path())
                .env_remove("RECUERDOS_AI_LOG")
                // The `listening` line is an info event; pin the level so a
                // RUST_LOG in the environment cannot hide it.
                .env("RUST_LOG", "info")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            for (key, value) in env {
                command.env(key, value);
            }

            let mut child = command
                .spawn()
                .expect("failed to spawn recuerdos-ai binary");

            // Both streams are drained for the child's whole life: a pipe
            // nobody reads fills up and blocks the daemon mid-write.
            let (bound_tx, bound_rx) = mpsc::channel();
            let marker = format!(":{port}");
            drain(child.stdout.take().expect("piped stdout"), move |line| {
                if line.contains("listening") && line.contains(&marker) {
                    let _ = bound_tx.send(());
                }
            });
            let stderr_log = Arc::new(Mutex::new(String::new()));
            let sink = Arc::clone(&stderr_log);
            drain(child.stderr.take().expect("piped stderr"), move |line| {
                let mut log = sink.lock().unwrap();
                log.push_str(line);
                log.push('\n');
            });

            if !wait_until_bound(&mut child, &bound_rx).await {
                let _ = child.wait();
                if attempt < PORT_ATTEMPTS {
                    continue;
                }
                panic!(
                    "recuerdos-ai exited before listening on {PORT_ATTEMPTS} fresh ports; \
                     its stderr:\n{}",
                    stderr_log.lock().unwrap()
                );
            }

            let base_url = format!("http://127.0.0.1:{port}");
            wait_until_healthy(&base_url).await;

            return Self {
                base_url,
                child,
                data_dir,
            };
        }
        unreachable!("the loop either returns or panics on its last attempt")
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }

    /// Creates a user and issues a key, returning the key.
    ///
    /// Goes through the CLI rather than an HTTP endpoint because that is
    /// the only way to mint a credential — by design, there is no route
    /// that hands out keys. It writes to the same database file the
    /// running server reads.
    pub fn create_user_with_key(&self, handle: &str, scopes: &str) -> String {
        self.cli(&["user", "add", handle]);
        let stdout = self.cli(&["key", "issue", "--user", handle, "--scopes", scopes]);

        stdout
            .split_whitespace()
            .find(|word| word.starts_with("ra_live_"))
            .unwrap_or_else(|| panic!("no key in `key issue` output:\n{stdout}"))
            .to_string()
    }

    pub fn revoke_key(&self, prefix: &str) {
        self.cli(&["key", "revoke", prefix]);
    }

    /// Runs a CLI subcommand against this app's data dir, returning stdout.
    fn cli(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_recuerdos-ai"))
            .args(args)
            .env("RECUERDOS_AI_STORAGE__PATH", self.data_dir.path())
            .env_remove("RECUERDOS_AI_LOG")
            .output()
            .expect("failed to run recuerdos-ai CLI");

        assert!(
            output.status.success(),
            "`recuerdos-ai {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8(output.stdout).expect("utf-8 stdout")
    }

    /// Polls (non-blocking) for process exit, up to `timeout`.
    pub async fn wait_for_exit(&mut self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// How many fresh ports [`TestApp::spawn_with`] tries before giving up. A
/// collision needs two suites to draw the same ephemeral port inside one
/// model load, so a second attempt almost always succeeds.
const PORT_ATTEMPTS: usize = 5;

/// Reads `stream` line by line on its own thread until it closes, handing
/// each line to `on_line`.
fn drain<R: Read + Send + 'static>(stream: R, mut on_line: impl FnMut(&str) + Send + 'static) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            match line {
                Ok(line) => on_line(&line),
                Err(_) => break,
            }
        }
    });
}

/// Waits for the child's `listening` line (`true`) or its exit (`false`):
/// the daemon binds after loading its models, and a lost port race shows
/// up as an exit with the bind error. Panics after [`HEALTH_TIMEOUT`].
async fn wait_until_bound(child: &mut Child, bound: &mpsc::Receiver<()>) -> bool {
    let deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;
    loop {
        if bound.try_recv().is_ok() {
            return true;
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            // The line and the exit can race; a bound child that then
            // crashed is still a bound child — the health wait reports it.
            return bound.try_recv().is_ok();
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "recuerdos-ai did not start listening within {HEALTH_TIMEOUT:?}. \
                 If the embedding model is missing this is where it surfaces: \
                 run `recuerdos-ai warm-models` (or `just warm`) first."
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Generous because each spawned app loads the ONNX embedding model at
/// startup (Phase 2), and the integration suites run in parallel — a
/// dozen processes doing that at once on a CI box takes far longer than
/// any one of them does alone. Too tight a bound here shows up as
/// mystifying flakes rather than as the resource contention it is.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(90);

async fn wait_until_healthy(base_url: &str) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;
    loop {
        if let Ok(response) = client.get(format!("{base_url}/healthz")).send().await {
            if response.status().is_success() {
                return;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "recuerdos-ai did not become healthy within {HEALTH_TIMEOUT:?}. \
                 If the embedding model is missing this is where it surfaces: \
                 run `recuerdos-ai warm-models` (or `just warm`) first."
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A `POST /v1/memories:direct` body in the structured shape
/// (ai-specs/memory-structure-plan.md §3.1), from one written in the old one.
///
/// Suites that test something other than the structure (recall, categories,
/// the map, isolation) save memories in passing; this gives each the required
/// `kind`, `topic`, `applies_to` and `origin` unless the body states them, and
/// renames the fields the old shape called otherwise. A body's `category` is
/// kept: it is still accepted alongside the structure.
pub fn structured(mut body: serde_json::Value) -> serde_json::Value {
    let object = body.as_object_mut().expect("a JSON object body");
    for (old, new) in [("subcategory", "subtopic"), ("expires_at", "valid_until")] {
        if let Some(value) = object.remove(old)
            && !value.is_null()
        {
            object.insert(new.to_string(), value);
        }
    }
    object.remove("confidence");
    let defaults = [
        ("kind", serde_json::json!("fact")),
        ("topic", serde_json::json!("coding")),
        ("applies_to", serde_json::json!({"target": "general"})),
        ("origin", serde_json::json!("user_said")),
    ];
    for (field, value) in defaults {
        object.entry(field).or_insert(value);
    }
    body
}
