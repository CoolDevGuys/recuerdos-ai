//! A `reqwest::blocking::Client` confined to a dedicated OS thread.
//!
//! # The hazard
//!
//! reqwest's blocking client owns a Tokio runtime. Building, using, *or
//! dropping* that client while on one of the daemon's async runtime
//! threads panics:
//!
//! ```text
//! Cannot drop a runtime in a context where blocking is not allowed.
//! This happens when a runtime is dropped from within an asynchronous context.
//! ```
//!
//! The synchronous [`Embedder`](crate::memories::domain::embedder::Embedder)
//! trait is called from exactly such places: the startup dimension-probe
//! runs on the main thread inside `#[tokio::main]`, the nightly
//! consolidation job runs on a Tokio task, and even `recordagent reindex`
//! is a sync command executing under the async runtime. A bare blocking
//! client in a remote embedder is therefore a latent panic — not a
//! theoretical one; it aborts the daemon at startup the moment a Gemini or
//! OpenAI-compatible provider is configured.
//!
//! # The fix
//!
//! Keep the client on its own thread — a plain `std::thread`, never a
//! Tokio worker — and hand work to it over a channel. Every reqwest
//! operation, and the client's eventual `Drop`, happen on that thread,
//! where blocking is always allowed. The caller blocks on an ordinary
//! channel receive, which merely parks the thread; it never enters or
//! drops a runtime, so it is safe from an async task, a blocking-pool
//! thread, or plain synchronous code alike. A synchronous embedder built
//! on this is genuinely callable from anywhere.

use crate::shared::error::{RaError, Result};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

/// A unit of work to run against the confined client. Boxed so jobs with
/// different return types share one channel; each closure carries its own
/// reply sender.
type Job = Box<dyn FnOnce(&reqwest::blocking::Client) + Send>;

pub struct BlockingHttpWorker {
    // `Option` so `Drop` can drop the sender *before* joining, closing the
    // channel so the worker's receive loop ends instead of blocking forever.
    jobs: Option<mpsc::Sender<Job>>,
    handle: Option<JoinHandle<()>>,
}

impl BlockingHttpWorker {
    /// Spawns the worker thread, which builds the client with `build` and
    /// then serves jobs until this handle is dropped.
    ///
    /// Blocks until the build finishes, so a client-construction failure
    /// surfaces synchronously here rather than on the first request — and,
    /// crucially, `build` runs on the worker thread, never on the caller's
    /// (possibly async) one.
    pub fn spawn<F>(build: F) -> Result<Self>
    where
        F: FnOnce() -> reqwest::Result<reqwest::blocking::Client> + Send + 'static,
    {
        let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<std::result::Result<(), String>>();

        let handle = thread::Builder::new()
            .name("embeddings-http".to_string())
            .spawn(move || {
                let client = match build() {
                    Ok(client) => {
                        // If the caller has already given up, there is
                        // nothing to serve — let the client drop here.
                        if ready_tx.send(Ok(())).is_err() {
                            return;
                        }
                        client
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };

                // Serve until the sender side is dropped. When the loop
                // ends, `client` drops on *this* thread — never in an
                // async context.
                while let Ok(job) = jobs_rx.recv() {
                    job(&client);
                }
            })
            .map_err(|e| {
                RaError::Internal(format!("failed to start the embeddings HTTP thread: {e}"))
            })?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                jobs: Some(jobs_tx),
                handle: Some(handle),
            }),
            Ok(Err(message)) => Err(RaError::Internal(format!(
                "failed to build the HTTP client: {message}"
            ))),
            Err(_) => Err(RaError::Internal(
                "the embeddings HTTP thread exited before signalling readiness".to_string(),
            )),
        }
    }

    /// Runs `job` against the confined client and returns its result,
    /// blocking the caller until the worker replies.
    ///
    /// Safe to call from an async task: it parks on a channel, it does not
    /// enter or drop a runtime. The `Ok`/`Err` here is the transport
    /// outcome (did the worker run the job at all); a job that itself
    /// returns a `Result` nests inside it.
    pub fn run<T, F>(&self, job: F) -> Result<T>
    where
        F: FnOnce(&reqwest::blocking::Client) -> T + Send + 'static,
        T: Send + 'static,
    {
        let (reply_tx, reply_rx) = mpsc::channel();
        let jobs = self.jobs.as_ref().ok_or_else(|| {
            RaError::Internal("the embeddings HTTP worker has stopped".to_string())
        })?;
        jobs.send(Box::new(move |client| {
            let _ = reply_tx.send(job(client));
        }))
        .map_err(|_| RaError::Internal("the embeddings HTTP worker has stopped".to_string()))?;

        reply_rx.recv().map_err(|_| {
            RaError::Internal("the embeddings HTTP worker dropped the job".to_string())
        })
    }
}

impl Drop for BlockingHttpWorker {
    fn drop(&mut self) {
        // Drop the sender first so the worker's `recv` returns `Err` and
        // the loop exits (dropping the client on its own thread); only
        // then can the join complete instead of deadlocking.
        self.jobs.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_jobs_and_returns_their_values() {
        let worker = BlockingHttpWorker::spawn(|| Ok(reqwest::blocking::Client::new()))
            .expect("worker should start");

        // The job never makes a request; it just proves the round trip.
        let doubled = worker.run(|_client| 21 * 2).expect("job should run");
        assert_eq!(doubled, 42);
    }

    #[test]
    fn a_build_failure_surfaces_synchronously() {
        let error = match BlockingHttpWorker::spawn(|| {
            // An impossible client build: a bogus proxy URL is rejected by
            // the builder, standing in for any construction error.
            reqwest::blocking::Client::builder()
                .proxy(reqwest::Proxy::all("http://[::1]:not-a-port")?)
                .build()
        }) {
            Ok(_) => panic!("a failed build must not yield a worker"),
            Err(error) => error,
        };
        assert!(matches!(error, RaError::Internal(_)), "{error:?}");
    }

    // The real point of the module: this must not panic even though it
    // builds, uses and drops the blocking client from inside a Tokio
    // async context — the exact situation that aborts a bare client.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn is_safe_to_build_use_and_drop_inside_an_async_context() {
        let worker = BlockingHttpWorker::spawn(|| Ok(reqwest::blocking::Client::new()))
            .expect("worker should start from within a runtime");
        let value = worker.run(|_client| "ok").expect("job should run");
        assert_eq!(value, "ok");
        // `worker` drops here, on the async thread — the confined client
        // drops on its own thread instead, so no runtime-drop panic.
    }
}
