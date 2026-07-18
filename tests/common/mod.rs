//! Black-box test harness: spawns the real `recordagent` binary against a
//! tmp data dir and a free port, and waits for it to become healthy before
//! handing control back to the test. Every scenario test in `tests/`
//! drives the app exactly the way a real client would — over HTTP.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

pub struct TestApp {
    pub base_url: String,
    child: Child,
    // Held for the app's lifetime so the data dir isn't cleaned up out
    // from under a running process; never read directly.
    _data_dir: TempDir,
}

impl TestApp {
    /// Spawns `recordagent serve` on a free port with a fresh tmp data
    /// dir, and blocks (async) until `/healthz` responds or panics after
    /// a 10s timeout.
    pub async fn spawn() -> Self {
        let port = free_port();
        let data_dir = tempfile::tempdir().expect("create tmp data dir");

        let child = Command::new(env!("CARGO_BIN_EXE_recordagent"))
            .arg("serve")
            .env("RECORDAGENT_SERVER__HOST", "127.0.0.1")
            .env("RECORDAGENT_SERVER__PORT", port.to_string())
            .env("RECORDAGENT_STORAGE__PATH", data_dir.path())
            .env_remove("RECORDAGENT_LOG")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn recordagent binary");

        let base_url = format!("http://127.0.0.1:{port}");
        wait_until_healthy(&base_url).await;

        Self {
            base_url,
            child,
            _data_dir: data_dir,
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
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

async fn wait_until_healthy(base_url: &str) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(response) = client.get(format!("{base_url}/healthz")).send().await {
            if response.status().is_success() {
                return;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("recordagent did not become healthy within 10s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
