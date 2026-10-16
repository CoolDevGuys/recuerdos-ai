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
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

        let documents: Vec<&str> = candidates.iter().map(|c| c.text).collect();
        let batch_size = self.batch_size;
        // `fastembed` returns the results *sorted* best-first with an
        // `index` back into `documents`; the domain contract is scores
        // aligned with `candidates`, so undo the sort here.
        let ranked = self.pool.with(|model| {
            model
                .rerank(query, documents.as_slice(), false, Some(batch_size))
                .map_err(|e| RaError::Internal(format!("failed to rerank candidates: {e}")))
        })?;

        align_scores(ranked, candidates.len())
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

/// The loaded sessions, one mutex each.
///
/// A small slice over mutexes with the two properties the adapter needs:
/// never run more than `len` scores at once (that is `len` mutexes by
/// construction), and never busy-wait for one to free — if every session
/// is busy, the caller queues on the first, so the waiter costs a
/// park, not a spin. `max_concurrency = 1` therefore degrades to exactly
/// the single-model-in-front-of-a-mutex behaviour the embedders use, and
/// two concurrent scores serialise (pinned by a test with a counting
/// stand-in, so the guarantee does not depend on an ONNX model).
struct InstancePool<T> {
    instances: Vec<Mutex<T>>,
}

impl<T> InstancePool<T> {
    fn new(instances: Vec<T>) -> Self {
        Self {
            instances: instances.into_iter().map(Mutex::new).collect(),
        }
    }

    /// Runs `f` on one instance, the first one free — waiting on the
    /// first if all are busy.
    fn with<R>(&self, f: impl FnOnce(&mut T) -> Result<R>) -> Result<R> {
        for instance in &self.instances {
            // `WouldBlock` means busy; a poisoned instance is skipped
            // here and surfaces on the waiting path below.
            if let Ok(mut guard) = instance.try_lock() {
                return f(&mut guard);
            }
        }
        let mut guard = self.instances[0]
            .lock()
            .map_err(|_| RaError::Internal("rerank model mutex poisoned".to_string()))?;
        f(&mut guard)
    }
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
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max.fetch_max(now, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(20));
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
