//! The cross-encoder adapter over `fastembed::TextRerank`
//! (reranking-plan.md Phase 5).
//!
//! # Why a pool instead of one model
//!
//! `TextRerank::rerank` takes `&mut self`: one ONNX session cannot run
//! two batches at once, so a lone session behind a `Mutex` caps inference
//! at one regardless of how many recalls are in flight. That cap *is* the
//! concurrency bound the plan asks for — but `[rerank].max_concurrency`
//! promises to configure that bound, not to discover it from ONNX. So the
//! adapter loads `max_concurrency` sessions and each scores on its own.
//! The honest cost is RAM: every session holds its own copy of the model
//! weights, which is why the number defaults to 1 for a model whose
//! sessions measure ~1.5 GiB each, and the plan's Phase 5 DoD makes the
//! memory delta a documented measurement rather than a surprise. The
//! benefit is the one the semaphore exists for: a burst of recalls cannot
//! starve the ingest embedder on the shared blocking pool beyond that
//! fixed number of concurrent inferences.
//!
//! # Why the scores are squashed
//!
//! `fastembed` returns the model's raw output — unbounded logits for the
//! BGE rerankers (negative for a bad pair), sigmoids for the Jina ones.
//! Two consumers need something narrower than "model-specific":
//! [`RerankStrategy::Replace`](crate::memories::domain::recall_ranker::RerankStrategy)
//! multiplies the raw score by the bounded recency/quality band, whose
//! Phase 4 contract is non-negative mutually-comparable scores; and the
//! audit/telemetry of Phases 6–7 want one readable scale across models.
//! A sigmoid maps every model's logits onto `(0, 1)` *strictly
//! monotonically*, so the RRF leg sees exactly the ranks it would have
//! seen anyway (it only ranks; see the `reranker` domain docs), `Replace`
//! gets its contract, and nothing else about the model changes.
//!
//! # Model selection
//!
//! `[rerank].model` names either one of the four models `fastembed`
//! ships (short name or HuggingFace repo id) or, for a "bring your own"
//! cross-encoder, a path to an ONNX file ending in `.onnx`. The file
//! must sit beside the four tokenizer files HuggingFace layouts carry
//! (`tokenizer.json`, `config.json`, `special_tokens_map.json`,
//! `tokenizer_config.json`); that is exactly what a
//! `fastembed::UserDefinedRerankingModel` needs and nothing more.
//! Unknown names are a config *error* (when rerank is on), never a
//! silent fallback — "you asked for this model and we could not find it"
//! must stop startup, like embeddings.

use crate::memories::domain::reranker::{RerankCandidate, Reranker};
use crate::shared::error::{RaError, Result};
use fastembed::{
    RerankInitOptions, RerankInitOptionsUserDefined, RerankResult, RerankerModel, TextRerank,
    TokenizerFiles, UserDefinedRerankingModel,
};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// The `(query, memory)` sequence cap handed to the tokenizer.
///
/// Memories are single sentences or short paragraphs — the whole eval
/// corpus fits inside 128 tokens — so the model's 512-token default only
/// buys padding: `fastembed` pads each batch to its longest pair, and a
/// longer cap widens that pad for every score without ever being used.
/// 256 leaves generous room for the query plus a long memory; if a
/// deployment starts saving paragraphs that need more, this is the one
/// constant to raise (and the Phase 7 latency grid is where to see what
/// it cost).
pub const DEFAULT_MAX_LENGTH: usize = 256;

/// A local ONNX cross-encoder over `fastembed::TextRerank`.
///
/// Constructed only by `bootstrap::memories_wiring::build_reranker`,
/// only when `[rerank].mode != "off"` — like the graph and the same
/// guarantee: with `mode = off` this type is never built, no model loads,
/// no network is touched, and startup costs exactly what it did before.
pub struct FastembedReranker {
    /// `max_concurrency` sessions; see the module docs for why the pool
    /// is the concurrency bound and what it costs.
    pool: InstancePool<TextRerank>,
    /// Documents per model forward pass. Memories are short, so batches
    /// of `depth` (≤ ~50) are dominated by a couple of passes; the knob
    /// exists for the Phase 7 grid, not for taste.
    batch_size: usize,
    /// The configured name, echoed as `model_id` for logs.
    model_id: String,
}

impl FastembedReranker {
    /// Loads `max_concurrency` sessions of `model_name`, downloading into
    /// `cache_dir` if absent.
    ///
    /// Slow (seconds per session, plus one download) and possibly
    /// network-bound, so — like [`FastembedEmbedder`](super::super::embeddings::fastembed_embedder::FastembedEmbedder)
    /// — it happens once at startup: an operator who enabled reranking
    /// must find out the model is unreachable when the daemon starts, not
    /// when the first recall silently serves its pre-rerank order.
    pub fn load(
        model_name: &str,
        cache_dir: PathBuf,
        max_concurrency: usize,
        batch_size: usize,
    ) -> Result<Self> {
        let model_id = model_name.trim().to_string();
        let batch_size = batch_size.max(1);
        // `AppConfig::validate` refuses 0, but `load` is also reachable
        // from tests and tools with hand-built config, and a pool of
        // nothing would deadlock every score rather than say so.
        let instances = (0..max_concurrency.max(1))
            .map(|_| load_instance(&model_id, &cache_dir))
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            pool: InstancePool::new(instances),
            batch_size,
            model_id,
        })
    }

    /// The configured model name, for logs and the startup echo.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }
}

impl Reranker for FastembedReranker {
    fn score(&self, query: &str, candidates: &[RerankCandidate<'_>]) -> Result<Vec<f32>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let ranked = self
            .pool
            .with(|model| self.judge(model, query, candidates))?;
        align_scores(ranked, candidates.len())
    }

    fn score_before(
        &self,
        query: &str,
        candidates: &[RerankCandidate<'_>],
        deadline: Instant,
    ) -> Result<Vec<f32>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // The wait is bounded; the inference once a session is had is not —
        // ONNX cannot be pre-empted mid-batch, and cutting a batch loose
        // would leave the session in an unknown state. What bounds *that* is
        // the head being scored in chunks (`preferred_chunk`), so the work
        // abandoned after a deadline is one chunk rather than the whole head.
        let ranked = self
            .pool
            .until(deadline, |model| self.judge(model, query, candidates))
            .map_err(|waited| match waited {
                Waited::Busy(budget) => RaError::RerankBusy(budget),
                Waited::Failed(error) => error,
            })?;
        align_scores(ranked, candidates.len())
    }
}

impl FastembedReranker {
    /// One forward pass over the candidates on a session this caller holds.
    ///
    /// Split out so `score` and `score_before` cannot drift apart: the two
    /// differ only in how the session was waited for.
    fn judge(
        &self,
        model: &mut TextRerank,
        query: &str,
        candidates: &[RerankCandidate<'_>],
    ) -> Result<Vec<RerankResult>> {
        let documents: Vec<&str> = candidates.iter().map(|c| c.text).collect();
        let batch_size = self.batch_size;
        // `fastembed` returns the results *sorted* best-first with an
        // `index` back into `documents`; the domain contract is scores
        // aligned with `candidates`, so undo the sort here.
        model
            .rerank(query, documents.as_slice(), false, Some(batch_size))
            .map_err(|e| RaError::Internal(format!("failed to rerank candidates: {e}")))
    }
}

/// Undo `fastembed`'s sort: place each result's squashed score back at the
/// candidate its `index` refers to, so the returned scores are aligned
/// with the candidates in the order they were given (the domain contract).
/// A result count that does not match the candidate count, or an index
/// outside it, is the model breaking its contract — an error degrades the
/// leg, never a panic.
fn align_scores(ranked: Vec<RerankResult>, candidates: usize) -> Result<Vec<f32>> {
    let mut aligned = vec![0.0f32; candidates];
    if ranked.len() != candidates {
        return Err(RaError::Internal(format!(
            "reranker returned {} scores for {} candidates",
            ranked.len(),
            candidates
        )));
    }
    for result in ranked {
        match aligned.get_mut(result.index) {
            Some(slot) => *slot = squash(result.score),
            None => {
                return Err(RaError::Internal(format!(
                    "reranker reported index {} for {} candidates",
                    result.index, candidates
                )));
            }
        }
    }
    Ok(aligned)
}

/// Maps a model's raw logit onto `(0, 1)` strictly monotonically.
///
/// See the module docs for why every score passes through this; ranks
/// cannot change because `sigmoid` is strictly increasing, which is the
/// property the whole adapter rests on and is pinned by a test below.
fn squash(logit: f32) -> f32 {
    1.0 / (1.0 + (-logit).exp())
}

/// The loaded sessions, handed out one at a time by a token.
///
/// A pool with the two properties the adapter needs: never run more than
/// `len` scores at once, and never busy-wait for one to free — a waiter
/// parks on the condition variable, it does not spin. `max_concurrency = 1`
/// therefore degrades to exactly the single-model-in-front-of-a-mutex
/// behaviour the embedders use, and two concurrent scores serialise (pinned
/// by a test with a counting stand-in, so the guarantee does not depend on
/// an ONNX model).
///
/// # Why a token, and not a mutex per session
///
/// A `Mutex<T>` per session can say "wait until this session is free" but
/// not "wait until *some* session is free, and stop waiting at my deadline":
/// the old pool tried each session with `try_lock` and then blocked on
/// `instances[0]`, which is an unbounded wait — so a saturated host piled up
/// detached scoring threads, each of which went on judging a head whose
/// request had already been answered, and made the *next* recall queue behind
/// that abandoned work. Holding a token is what makes the wait finite and the
/// session identity irrelevant: `acquire_until` hands back whichever session
/// frees first, and giving it up is a `Drop`.
///
/// The plan sketched this as a token *channel* with `recv_timeout`. Same
/// shape, different primitive: `mpsc::Receiver` is not `Sync`, so the only
/// way to share one is behind a mutex — and a thread parked in
/// `recv_timeout` holds that mutex, so the second waiter's deadline would be
/// gated by the first one's, which is the bug being fixed. A `VecDeque` of
/// free indices under a mutex with a `Condvar` waits per caller instead, and
/// still costs no new dependency.
struct InstancePool<T> {
    /// Sessions, each touched only by whoever holds its index.
    sessions: Vec<Mutex<T>>,
    /// The indices nobody holds. Empty means every session is judging.
    free: Mutex<VecDeque<usize>>,
    /// Notified when an index comes back.
    ready: Condvar,
}

/// Holds one session until it is dropped, which returns its token.
///
/// The `Drop` is what makes a panic in scoring, or a caller that walks away
/// at its deadline, unable to leak a session: the token goes back either
/// way, so the pool cannot drain to zero and stop serving.
struct Session<'a, T> {
    pool: &'a InstancePool<T>,
    index: Option<usize>,
}

impl<'a, T> Session<'a, T> {
    /// Claims the next free index. The pool lock goes down with this frame;
    /// the session stays held until `Drop`, which is the whole point of the
    /// token.
    fn taken(
        pool: &'a InstancePool<T>,
        mut free: std::sync::MutexGuard<'_, VecDeque<usize>>,
    ) -> Self {
        let index = free
            .pop_front()
            .expect("taken is only called with a non-empty free list");
        Self {
            pool,
            index: Some(index),
        }
    }

    /// Runs `f` on the session this token stands for. A session whose
    /// inference panicked once stays poisoned and keeps reporting that: the
    /// leg degrades on every recall, which the `rerank` trace shows the
    /// operator, rather than the pool quietly shrinking to zero concurrency.
    fn with<R>(&mut self, f: impl FnOnce(&mut T) -> Result<R>) -> Result<R> {
        let index = self
            .index
            .expect("a live session always holds its index until drop");
        let mut guard = self.pool.sessions[index]
            .lock()
            .map_err(|_| RaError::Internal("rerank model session mutex poisoned".to_string()))?;
        f(&mut guard)
    }
}

impl<T> Drop for Session<'_, T> {
    fn drop(&mut self) {
        let Some(index) = self.index.take() else {
            return;
        };
        match self.pool.free.lock() {
            Ok(mut free) => {
                free.push_back(index);
                self.pool.ready.notify_one();
            }
            // Poisoned means a thread died holding the free list, which
            // cannot have left it corrupt (it is a `VecDeque` of integers
            // pushed under this same lock), so the token goes back and the
            // queue is read again. Losing the token instead would be a
            // permanent capacity cut for a dead thread's panic.
            Err(poisoned) => {
                poisoned.into_inner().push_back(index);
                self.pool.ready.notify_one();
            }
        }
    }
}

impl<T> InstancePool<T> {
    fn new(instances: Vec<T>) -> Self {
        let sessions: Vec<Mutex<T>> = instances.into_iter().map(Mutex::new).collect();
        let free = Mutex::new((0..sessions.len()).collect::<VecDeque<usize>>());
        Self {
            sessions,
            free,
            ready: Condvar::new(),
        }
    }

    /// Runs `f` on one session, waiting as long as it takes for one to free.
    fn with<R>(&self, f: impl FnOnce(&mut T) -> Result<R>) -> Result<R> {
        self.acquire_blocking().with(f)
    }

    /// Runs `f` on one session, or returns busy when none frees by
    /// `deadline`. `Waited::Busy` says nothing was judged: the caller's
    /// budget went entirely on queueing.
    fn until<R>(
        &self,
        deadline: Instant,
        f: impl FnOnce(&mut T) -> Result<R>,
    ) -> std::result::Result<R, Waited> {
        let mut session = match self.acquire_until(deadline) {
            Ok(session) => session,
            Err(budget) => return Err(Waited::Busy(budget)),
        };
        // Whatever `f` does — score, error, or panic — the token is returned
        // by `Session::drop` before this frame is left.
        session.with(f).map_err(Waited::Failed)
    }

    fn acquire_blocking(&self) -> Session<'_, T> {
        let mut free = self.locked_free();
        while free.is_empty() {
            free = self
                .ready
                .wait(free)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        Session::taken(self, free)
    }

    fn acquire_until(&self, deadline: Instant) -> std::result::Result<Session<'_, T>, Duration> {
        // The budget is read once, before any waiting, so the busy error can
        // say what the caller asked for rather than what it happened to get.
        let budget = deadline.saturating_duration_since(Instant::now());
        let mut free = self.locked_free();
        loop {
            if !free.is_empty() {
                return Ok(Session::taken(self, free));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(budget);
            }
            let (guard, timed_out) = self
                .ready
                .wait_timeout_while(free, remaining, |free| free.is_empty())
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            free = guard;
            if timed_out.timed_out() && free.is_empty() {
                return Err(budget);
            }
        }
    }

    /// The free list, ignoring a poisoning that cannot have corrupted it.
    fn locked_free(&self) -> std::sync::MutexGuard<'_, VecDeque<usize>> {
        self.free
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Why `InstancePool::until` did not run `f`.
#[derive(Debug)]
enum Waited {
    /// No session freed in time; the duration is how long the caller waited.
    Busy(Duration),
    /// A session was had, and `f` failed on it.
    Failed(RaError),
}

/// What `[rerank].model` can name: one of `fastembed`'s built-in
/// cross-encoders, or a local ONNX file for a "bring your own" model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RerankModelChoice {
    BuiltIn(RerankerModel),
    UserDefined(PathBuf),
}

/// Resolve the configured model name.
///
/// Case-insensitive on the names, and accepting both the short form
/// (`jina-reranker-v1-turbo-en`) and the HuggingFace repo id it maps to,
/// because both appear in `fastembed`'s own model tables and operators
/// copy from both. A name ending in `.onnx` is taken as a file path —
/// existence is checked at load, not here, so this stays a pure function
/// config validation can call.
pub fn resolve_model(name: &str) -> Result<RerankModelChoice> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RaError::Validation("[rerank].model is empty".to_string()));
    }
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".onnx") {
        return Ok(RerankModelChoice::UserDefined(PathBuf::from(name)));
    }
    let model = match lower.as_str() {
        "bge-reranker-base" | "baai/bge-reranker-base" => RerankerModel::BGERerankerBase,
        "bge-reranker-v2-m3" | "rozgo/bge-reranker-v2-m3" => RerankerModel::BGERerankerV2M3,
        "jina-reranker-v1-turbo-en" | "jinaai/jina-reranker-v1-turbo-en" => {
            RerankerModel::JINARerankerV1TurboEn
        }
        "jina-reranker-v2-base-multilingual" | "jinaai/jina-reranker-v2-base-multilingual" => {
            RerankerModel::JINARerankerV2BaseMultiligual
        }
        other => {
            return Err(RaError::Validation(format!(
                "unknown rerank model {other:?} (expected one of \
                 bge-reranker-base, bge-reranker-v2-m3, jina-reranker-v1-turbo-en, \
                 jina-reranker-v2-base-multilingual, or a path to an .onnx file)"
            )));
        }
    };
    Ok(RerankModelChoice::BuiltIn(model))
}

/// One session's worth of loading, per the [`RerankModelChoice`] branch.
fn load_instance(model_name: &str, cache_dir: &Path) -> Result<TextRerank> {
    std::fs::create_dir_all(cache_dir).map_err(|e| {
        RaError::Internal(format!(
            "failed to create the model cache directory {}: {e}",
            cache_dir.display()
        ))
    })?;

    match resolve_model(model_name)? {
        RerankModelChoice::BuiltIn(model) => {
            // `Display` for `RerankerModel` prints the HuggingFace repo id —
            // what to point at when the download fails.
            let repo = model.to_string();
            TextRerank::try_new(
                RerankInitOptions::new(model)
                    .with_cache_dir(cache_dir.to_path_buf())
                    .with_show_download_progress(false)
                    .with_max_length(DEFAULT_MAX_LENGTH),
            )
            .map_err(|e| {
                RaError::Internal(format!(
                    "failed to load rerank model {model_name:?} (from {repo}) into {}: \
                     {e}. If this host has no internet access, pre-populate the cache \
                     with `recuerdos-ai warm-models` or bake the model into the image.",
                    cache_dir.display()
                ))
            })
        }
        RerankModelChoice::UserDefined(path) => load_user_defined(&path),
    }
}

/// The tokenizer files `fastembed` needs beside a bring-your-own ONNX
/// model — the same HuggingFace layout its downloader produces — read by
/// [`load_user_defined`], whose error message names the one that failed.
fn load_user_defined(onnx_path: &Path) -> Result<TextRerank> {
    let dir = onnx_path.parent().ok_or_else(|| {
        RaError::Validation(format!(
            "[rerank].model {:?} has no parent directory to read the tokenizer files from",
            onnx_path.display()
        ))
    })?;

    let read = |file: &str| {
        let path = dir.join(file);
        std::fs::read(&path).map_err(|e| {
            RaError::Internal(format!(
                "failed to read {file} for the rerank model {} (a user-defined \
                 model needs {file} beside its .onnx file): {e}",
                onnx_path.display()
            ))
        })
    };

    let tokenizer_files = TokenizerFiles {
        tokenizer_file: read("tokenizer.json")?,
        config_file: read("config.json")?,
        special_tokens_map_file: read("special_tokens_map.json")?,
        tokenizer_config_file: read("tokenizer_config.json")?,
    };

    TextRerank::try_new_from_user_defined(
        UserDefinedRerankingModel::new(onnx_path.to_path_buf(), tokenizer_files),
        RerankInitOptionsUserDefined::new().with_max_length(DEFAULT_MAX_LENGTH),
    )
    .map_err(|e| {
        RaError::Internal(format!(
            "failed to load the rerank model from {}: {e}",
            onnx_path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::ids::MemoryId;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[test]
    fn resolves_the_supported_models_by_short_name_and_repo_id() {
        for name in [
            "bge-reranker-base",
            "baai/bge-reranker-base",
            "bge-reranker-v2-m3",
            "rozgo/bge-reranker-v2-m3",
            "jina-reranker-v1-turbo-en",
            "jinaai/jina-reranker-v1-turbo-en",
            "jina-reranker-v2-base-multilingual",
            "jinaai/jina-reranker-v2-base-multilingual",
            // trimmed and case-insensitive, like the embedder's resolver
            "  JINAAI/JINA-RERANKER-V1-TURBO-EN ",
        ] {
            assert!(resolve_model(name).is_ok(), "{name} should resolve");
        }
    }

    #[test]
    fn takes_an_onnx_path_as_a_user_defined_model() {
        let choice = resolve_model("/opt/models/model.onnx").unwrap();
        assert_eq!(
            choice,
            RerankModelChoice::UserDefined(PathBuf::from("/opt/models/model.onnx"))
        );
    }

    #[test]
    fn rejects_an_unknown_model_by_name() {
        let error = resolve_model("bge-reranker-v3").unwrap_err();
        assert!(matches!(error, RaError::Validation(_)), "got {error:?}");
        assert!(error.to_string().contains("jina-reranker-v1-turbo-en"));

        let error = resolve_model("   ").unwrap_err();
        assert!(matches!(error, RaError::Validation(_)));
    }

    #[test]
    fn a_missing_tokenizer_file_names_itself_not_a_load_backtrace() {
        // A user-defined path whose directory lacks the tokenizer files:
        // the error must say which file is missing, since that is the
        // misconfiguration the operator actually made.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.onnx");

        let error = load_user_defined(&path).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("tokenizer.json"), "{message}");
    }

    #[test]
    fn squashing_is_monotonic_and_strictly_inside_the_unit_interval() {
        // The adapter's whole claim: scores change, ranks never do. The
        // sigmoid saturates to its bounds at extremes f32 cannot express
        // as interior values, so strictness is asserted where the curve
        // is representable and containment holds everywhere.
        assert!((0.0..1.0).contains(&squash(-15.0)));
        assert!((0.0..1.0).contains(&squash(15.0)));
        assert!(squash(-1000.0) >= 0.0);
        assert!(squash(1000.0) <= 1.0);
        let mut last = -1.0;
        for step in 0..31 {
            let logit = step as f32 - 15.0;
            let value = squash(logit);
            assert!(value > last, "sigmoid must strictly increase at {logit}");
            last = value;
        }
    }

    /// A stand-in instance that counts how many are inside `with` at the
    /// same time, so the pool's concurrency claim is a measurement rather
    /// than an inspection of ONNX.
    struct Counting {
        active: &'static AtomicUsize,
        max: &'static AtomicUsize,
    }

    impl Counting {
        fn enter(&mut self) {
            self.hold(Duration::from_millis(20));
        }

        /// Inside a session for `how_long`, recording the concurrency peak.
        fn hold(&mut self, how_long: Duration) {
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max.fetch_max(now, Ordering::SeqCst);
            thread::sleep(how_long);
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn one_instance_serialises_concurrent_scores() {
        static ACTIVE: AtomicUsize = AtomicUsize::new(0);
        static MAX: AtomicUsize = AtomicUsize::new(0);

        let pool = InstancePool::new(vec![Counting {
            active: &ACTIVE,
            max: &MAX,
        }]);

        thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    pool.with(|instance| {
                        instance.enter();
                        Ok(())
                    })
                    .unwrap()
                });
            }
        });

        assert_eq!(
            MAX.load(Ordering::SeqCst),
            1,
            "one instance ran two scores at once"
        );
    }

    #[test]
    fn two_instances_allow_two_scores_at_once() {
        static ACTIVE: AtomicUsize = AtomicUsize::new(0);
        static MAX: AtomicUsize = AtomicUsize::new(0);

        let pool = InstancePool::new(vec![
            Counting {
                active: &ACTIVE,
                max: &MAX,
            },
            Counting {
                active: &ACTIVE,
                max: &MAX,
            },
        ]);

        // A barrier of 2 *inside* would deadlock a 1-instance pool and prove
        // the bound from the other side; here two 20ms sleeps are enough to
        // overlap, and the max is the assertion.
        thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    pool.with(|instance| {
                        instance.enter();
                        Ok(())
                    })
                    .unwrap()
                });
            }
        });

        assert_eq!(
            MAX.load(Ordering::SeqCst),
            2,
            "two instances never scored at once"
        );
    }

    #[test]
    fn an_error_from_the_instance_propagates() {
        let pool = InstancePool::new(vec![()]);
        let error = pool
            .with(|_| Err::<(), _>(RaError::Internal("boom".to_string())))
            .unwrap_err();
        assert!(error.to_string().contains("boom"));
    }

    #[test]
    fn a_saturated_pool_refuses_a_caller_at_its_deadline() {
        static ACTIVE: AtomicUsize = AtomicUsize::new(0);
        static MAX: AtomicUsize = AtomicUsize::new(0);

        let pool = InstancePool::new(vec![Counting {
            active: &ACTIVE,
            max: &MAX,
        }]);

        thread::scope(|scope| {
            // One caller holds the only session for 150 ms — longer than the
            // second caller's whole budget, which is the point: the old pool
            // would have parked that caller on the session and scored a head
            // nobody was waiting for.
            let holder = scope.spawn(|| {
                pool.with(|instance| {
                    instance.hold(Duration::from_millis(150));
                    Ok(())
                })
            });
            thread::sleep(Duration::from_millis(20));

            let budget = Duration::from_millis(30);
            let started = Instant::now();
            let outcome = pool.until(started + budget, |instance| {
                instance.enter();
                Ok(())
            });
            let waited = started.elapsed();

            // The budget it reports is what was left when the caller walked
            // in, so a hair under the requested 30 ms — the promise is "your
            // budget", not "the instant you asked for".
            assert!(
                matches!(outcome, Err(Waited::Busy(asked))
                    if asked <= budget && budget - asked < Duration::from_millis(5)),
                "a caller with no session should be refused with its own budget, got {outcome:?}"
            );
            // The deadline is the promise: not the holder's inference, and
            // not a spin that returns instantly having done nothing.
            assert!(
                waited < Duration::from_millis(120),
                "the refused caller waited {waited:?} — past its {budget:?} budget, \
                 which is the abandoned-work bug this pool exists to remove"
            );
            assert_eq!(
                MAX.load(Ordering::SeqCst),
                1,
                "a refused caller still ran work inside a session"
            );
            holder.join().unwrap().unwrap();
        });
    }

    #[test]
    fn a_session_that_frees_in_time_is_used_rather_than_refused() {
        static ACTIVE: AtomicUsize = AtomicUsize::new(0);
        static MAX: AtomicUsize = AtomicUsize::new(0);

        let pool = InstancePool::new(vec![Counting {
            active: &ACTIVE,
            max: &MAX,
        }]);

        thread::scope(|scope| {
            let holder = scope.spawn(|| {
                pool.with(|instance| {
                    instance.hold(Duration::from_millis(30));
                    Ok(())
                })
            });
            thread::sleep(Duration::from_millis(10));

            // Generous budget: this caller is meant to wait, and to get the
            // session the moment it frees. Busy is not the default answer.
            let outcome = pool.until(Instant::now() + Duration::from_secs(5), |instance| {
                instance.hold(Duration::from_millis(10));
                Ok("judged")
            });

            assert!(
                matches!(outcome, Ok("judged")),
                "a waiting caller gave up early: {outcome:?}"
            );
            assert_eq!(
                MAX.load(Ordering::SeqCst),
                1,
                "a timed caller ran two scores at once on one session"
            );
            holder.join().unwrap().unwrap();
        });
    }

    #[test]
    fn a_panicking_score_gives_its_session_back() {
        static ACTIVE: AtomicUsize = AtomicUsize::new(0);
        static MAX: AtomicUsize = AtomicUsize::new(0);

        let pool = InstancePool::new(vec![Counting {
            active: &ACTIVE,
            max: &MAX,
        }]);

        let panicked: std::result::Result<std::result::Result<(), RaError>, _> =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pool.with(|_| panic!("the model died mid-batch"))
            }));
        assert!(panicked.is_err(), "the panic must escape the pool");

        // Capacity is what a leak would cost: the token came back, so the
        // pool serves again instead of draining to zero. The session itself
        // is poisoned by the panic and says so — the leg degrades on every
        // recall, visible in the `rerank` trace, rather than the pool
        // silently losing concurrency.
        let mut session = pool
            .acquire_until(Instant::now() + Duration::from_secs(1))
            .expect("a panicked score must not swallow its token");
        let error = session
            .with::<()>(|_| unreachable!("a poisoned session never runs its work"))
            .unwrap_err();
        assert!(
            error.to_string().contains("poisoned"),
            "the poisoned session should name itself: {error}"
        );
    }

    #[test]
    fn scores_are_aligned_back_from_the_sorted_results() {
        // The reverse mapping from `fastembed`'s sorted `RerankResult`s
        // (its `index` points back into the candidates) is adapter logic,
        // so it gets its own pin.
        //
        // Scores: candidate 0 gets 3.0, candidate 1 gets 1.0, candidate 2
        // gets 2.0 — delivered sorted, that is indices [0, 2, 1].
        let aligned = align_scores(
            vec![
                RerankResult {
                    document: None,
                    score: 3.0,
                    index: 0,
                },
                RerankResult {
                    document: None,
                    score: 2.0,
                    index: 2,
                },
                RerankResult {
                    document: None,
                    score: 1.0,
                    index: 1,
                },
            ],
            3,
        )
        .unwrap();

        let expected = [squash(3.0), squash(1.0), squash(2.0)];
        for (got, want) in aligned.iter().zip(expected) {
            assert!((got - want).abs() < f32::EPSILON, "{got} != {want}");
        }

        // A result outside the candidate range, or a count that disagrees
        // with the candidate count, is a broken model: the leg must report
        // it (and degrade), not index out of bounds.
        assert!(
            align_scores(
                vec![RerankResult {
                    document: None,
                    score: 1.0,
                    index: 7,
                }],
                3,
            )
            .is_err()
        );
        assert!(
            align_scores(
                vec![RerankResult {
                    document: None,
                    score: 1.0,
                    index: 0,
                }],
                2,
            )
            .is_err()
        );
    }

    /// Exercises the real model: loads it, scores a tiny fixed set, and
    /// asserts the relevant passage outscores an unrelated one. Ignored
    /// by default because it needs the model on disk (and a download on a
    /// cold cache); the nightly CI job runs the ignored tests, and the
    /// `warm-models` command pre-populates the cache like the embedders'.
    #[test]
    #[ignore = "requires the rerank ONNX model; run with --ignored"]
    fn the_relevant_passage_outscores_the_unrelated_one() {
        let reranker = FastembedReranker::load(
            "jina-reranker-v1-turbo-en",
            model_cache_dir(),
            1,
            16, // one batch for a handful of documents
        )
        .unwrap();

        let candidates = [
            "The user prefers pnpm over npm and yarn for package management",
            "The deployment target for the backend is Hetzner, migrated from Fly.io",
        ];
        let candidates: Vec<RerankCandidate> = candidates
            .iter()
            .map(|text| RerankCandidate {
                id: MemoryId::new(),
                text,
            })
            .collect();

        let scores = reranker
            .score("which package manager does the user prefer", &candidates)
            .unwrap();
        assert_eq!(scores.len(), 2);
        assert!(
            scores[0] > scores[1],
            "the package-manager passage should outscore the hosting one: {scores:?}"
        );
    }

    #[test]
    #[ignore = "requires the rerank ONNX model; run with --ignored"]
    fn a_batch_larger_than_batch_size_returns_aligned_scores() {
        // `fastembed` splits internally and concatenates; the adapter must
        // keep the alignment across the split (the sort-back is per whole
        // call, not per chunk).
        let reranker = FastembedReranker::load(
            "jina-reranker-v1-turbo-en",
            model_cache_dir(),
            1,
            4, // deliberately smaller than the 5 candidates below
        )
        .unwrap();

        let texts = [
            "pnpm is the preferred package manager",
            "one",
            "two",
            "three",
            "which package manager does the user prefer, exactly: pnpm",
        ];
        let candidates: Vec<RerankCandidate> = texts
            .iter()
            .map(|text| RerankCandidate {
                id: MemoryId::new(),
                text,
            })
            .collect();

        let scores = reranker
            .score("which package manager", &candidates)
            .unwrap();
        assert_eq!(scores.len(), texts.len());
        assert!(
            scores[0] > scores[1],
            "scores must not swap across the batch split: {scores:?}"
        );
    }

    fn model_cache_dir() -> PathBuf {
        std::env::var("RECUERDOS_AI_MODEL_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/models"))
    }
}
