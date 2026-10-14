# Recuerdos AI — Recall Quality & Cross-Encoder Reranking Plan

**v0.1 · drafted 2026-10-14 · companion to [project-plan.md](project-plan.md) and [implementation-plan.md](../implementation-plan.md)**

This plan fixes two measured recall defects (keyword-leg noise, miscalibrated
relevance) and then, only if the measurements still justify it, adds a
cross-encoder reranker. It is written so that an AI agent (or a human) can pick
up **one phase at a time**, work until that phase's **Definition of Done (DoD)**
is met, and stop. Every phase ends in a state that is shippable and testable on
its own.

---

## 0. How an agent must use this document

1. **Work phases in order.** Each phase depends on the numbers recorded by the
   previous one. Do not start phase *N+1* until every DoD box of phase *N* is
   ticked **and** the phase's numbers are written into [§12 Results log](#12-results-log).
2. **One branch per phase**, named as given in the phase header. One logical
   change per commit, conventional-commit style (`fix(...)`, `feat(...)`,
   `chore(...)`), matching `git log`.
3. **The standard check must pass at the end of every phase:**
   `just check-native` (= `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings`
   + `bash scripts/check-boundaries.sh` + `cargo test`). Docker users run `just check`.
4. **The eval must be run at the end of every phase that touches retrieval:**
   `just eval-native` (needs the embedding model reachable; see Phase 0, task 0.1).
   Record the output in §12.
5. **Respect the architecture.** Contracts (traits) live in `domain`, implementations
   in `infrastructure`, and every method that touches user data takes `&UserContext`.
   `scripts/check-boundaries.sh` enforces the boundaries; do not weaken it.
6. **"No X, no change" is a hard rule.** Every new optional component (`Option<Arc<dyn …>>`)
   must leave the pre-existing code path *byte-identical* when absent, pinned by a test
   (the pattern is `MemoryRecaller` with `graph: None`).
7. **Items marked `VERIFY` were not checked while writing this plan.** Confirm them
   against the code or crate docs before relying on them, and note the answer in §12.
8. If a DoD cannot be met, **stop and record why in §12**; do not weaken the DoD to
   make it pass. The gates in Phase 3 and Phase 7 are decisions, not formalities.

---

## 1. Why this plan exists (evidence)

Four natural-language recalls were run against the live `recuerdos-ai` store
(corpus: mostly coding preferences saved 2026-08-11; `gemini-embedding-2`):

| Query | Right memory | Where it ranked |
|---|---|---|
| "what do I use for data storage" | "Only parameterized queries are permitted for database operations" | **#3** (below "Prefers short answers" 0.89, "snake_case filenames" 0.87) |
| "deployment and hosting" | "backend production Dockerfile is managed through production-deployment OpenSpec changes" | **#2** |
| "health, fitness or family" | "lives with a partner and baby in Berlin" | **#5**, relevance 0.33, under four unrelated coding rules at 0.84–0.86 |
| `what and or the for do` (stopwords only) | none exists | **5 results at relevance 0.84–0.91** — should be ~0 |

The same topics phrased without filler words ("database storage", "deployment
hosting", "partner baby Berlin family life") ranked the right memory first or
second at 0.86–0.93.

The committed eval agrees that this is a *ranking* problem, not a *recall*
problem: `eval/baseline.json` has **recall@5 = 91.7 %** but **precision@1 = 62.5 %**
over 24 cases.

### Root causes found in the code

1. **Keyword leg matches function words.** `src/memories/infrastructure/tantivy_text_index.rs`
   declares `content`, `tags`, `category` as plain `TEXT` fields (tantivy's `default`
   tokenizer: simple tokenizer + lowercase, **no stopword removal, no stemming**), and the
   query parser ORs every term. "what", "and", "or", "the", "do", "I" therefore each match
   any memory containing them, and RRF then promotes generic memories that collect such
   matches.
2. **Relevance baseline is calibrated for a different model.** `[retrieval].similarity_baseline`
   defaults to `0.5`, documented in `recall_ranker.rs` as measured against `bge-small-en-v1.5`.
   `recuerdos-ai.toml` runs `gemini-embedding-2` and does not override it. Unrelated text in
   Gemini space sits well above 0.5 cosine (**VERIFY by measurement in Phase 2**), so noise is
   reported as relevance and the 0.25 floor stops filtering. Ranking is unaffected (RRF uses
   ranks), but `relevance` and `min_relevance` are wrong.
3. **There is no second-stage ranker.** Fusion is rank-based (RRF, k = 60) over
   independently-scored candidates. A cross-encoder reads query and memory *together*
   (full cross-attention) and is the standard fix for "right memory is in the top 5 but not
   first". It is only worth building if causes 1–2, which are free at query time, do not
   already close the gap.

### What is deliberately out of scope

- A concept/cluster layer on the entity graph (suggested separately). The evidence
  above shows the right memories are already retrieved; revisit only if Phase 3's eval
  shows failures that survive ranking fixes.
- Stemming/lemmatisation in the keyword leg, query expansion, and per-leg weighting
  other than the rerank weight. Noted as follow-ups in §11.

---

## 2. Corrections to the first draft of the reranking plan

The earlier draft ("Option A") was reviewed against the code; the following are **binding
changes** reflected in the phases below.

| # | Draft said | Reality / decision |
|---|---|---|
| 1 | "fastembed is bi-encoders only → use `ort` + `tokenizers` directly" | `fastembed` 5.17.3 (already a dependency) exposes `TextRerank`, `RerankerModel` (`BGERerankerBase`, `BGERerankerV2M3`, `JINARerankerV1TurboEn`, `JINARerankerV2BaseMultiligual`) and `UserDefinedRerankingModel` for any ONNX cross-encoder (e.g. MS-MARCO MiniLM). **Use it.** No direct `ort`/`tokenizers` dependency, no hand-written session/tokenisation/download code. `VERIFY` whether a Cargo feature flag is required. |
| 2 | Gate on "precision@5" | The harness reports **recall@5 and precision@1**, not precision@5. The gap to close is p@1 (62.5 %) vs recall@5 (91.7 %). Gate on **precision@1 and MRR**; recall@5 must merely not regress. |
| 3 | Replace `ScoredMemory.relevance` with the cross-encoder probability | **No (not in this plan's first iteration).** Cross-encoder sigmoids saturate toward 0/1 and are not calibrated on this data; the models are trained on question→passage pairs while memories are declarative statements, so a correct match can score low and the 0.25 floor would *delete* it. The floor (`DEFAULT_RELEVANCE_FLOOR`), `GRAPH_HOP_RELEVANCE` and the test pinning them are tied to the current scale. Expose the cross-encoder score as a **separate** field; revisit relevance only after measuring its distribution (Phase 7, task 7.4). |
| 4 | Fuse rerank as a 4th RRF leg and claim it "degrades gracefully" | The rerank list is a permutation of the same candidates the other legs ranked, so with k = 60 it gets ~1 vote in 3–4 and can be muted exactly when vector and BM25 agree on a wrong answer. Graceful degradation only holds at low weight, which also mutes a *good* rerank. **Tune weights above 1 as well (0.5–4)** and evaluate a `replace` strategy (order the top-N by rerank score × the bounded recency/quality band) alongside RRF. |
| 5 | Baseline first, then reranker | **Fix the free defects first** (Phases 1–2), *then* baseline, otherwise the reranker is measured against a baseline with a known bug and may be built to compensate for it. Phase 3 is an explicit go/no-go. |
| 6 | "+30–80 ms at depth 50" | Unmeasured, and only plausible for MiniLM-L6. The multilingual Jina model is several times larger. Latency is **measured by the eval (p50/p95) on the deployment target** and budgeted in the gate. |
| 7 | Eager model download at boot | Needs a persistent volume or baking into the release image (`warm-models`); otherwise every deploy re-downloads from HuggingFace. Pin revision/checksum where the API allows. Adds RAM and image size — currently the Gemini setup has **no** local model at all. |
| 8 | (missing) | Deadline + fallback, concurrency cap, shadow mode, `touch_accessed` side-effect handling, `MatchDetail` is `Eq` (an `f32` field breaks it), SDK/API surface change, audit command for existing data. All added below. |
| 9 | (my own earlier suggestion) "use tantivy's English `StopWordFilter`" | **Insufficient.** Tantivy's built-in English list targets articles/prepositions/conjunctions (`a, an, and, are, as, at, be, but, by, for, if, in, into, is, it, no, not, of, on, or, such, that, the, their, then, there, these, they, this, to, was, will, with` — `VERIFY` the exact list). It does **not** contain `what, which, who, how, do, does, I, my, you, can, should…`, which is what question-style agent queries are made of. Phase 1 ships a project-owned list. |

---

## 3. Design invariants (must hold after every phase)

1. **No reranker configured ⇒ identical behaviour to today**, including identical
   ordering, scores and response shape (additive fields excepted, see I-5).
2. **A failing optional leg degrades, never fails the request** (keyword, graph, rerank all
   `warn!` and fall back). A *startup* failure of an *explicitly enabled* component fails loudly.
3. **Ranking calibration is documented in `recall_ranker.rs` style** — any new weight or
   band has a doc comment explaining *why* it has that value and what breaks if changed.
4. **`relevance` keeps its meaning** (absolute, thresholdable, measured from index signals)
   until a phase explicitly and measurably changes it.
5. **API changes are additive only.** New fields are `Option`/defaulted; MCP tool output,
   REST responses and the Python SDK (`sdk/`) must keep working with old clients and old
   `eval/baseline.json` files (`#[serde(default)]`).
6. **Recall is observationally pure for audit/eval paths.** `MemoryRecaller::execute`
   calls `touch_accessed` (feeds importance decay); tools that replay queries over real
   data must not bump access counts (see Phase 6).
7. **User isolation:** nothing added may read across users; new store/CLI code takes
   `&UserContext` or iterates users the same way `GraphBackfiller` does.

---

## 4. What happens to existing memories

**Important design fact:** a cross-encoder scores a *(query, memory)* pair **at query
time**. There is no per-memory reranking state to compute, so there is nothing to
"rerank all" in storage — adding the reranker requires **no migration of existing
memories**. A pre-computed score would have to be per query, which is meaningless
ahead of time. (A query-result cache is a possible later optimisation, §11.)

What existing data **does** need, per phase:

| Change | Existing memories need | How | Model calls | Daemon |
|---|---|---|---|---|
| Phase 1 stopword analyzer | **Keyword (tantivy) index rebuild** from the `memories` table | Automatic on first start after upgrade (versioned index dir) **and** manual `recuerdos-ai reindex --text` | none | auto path: normal start; manual path: stopped (tantivy writer lock) |
| Phase 2 baseline | nothing (config only) | edit `[retrieval].similarity_baseline` | none | restart |
| Phases 4–5 reranker | **nothing** | — | none at ingest | restart to load model |
| Phase 6 audit | nothing; **read-only replay** over real memories | `recuerdos-ai rerank audit …` | local rerank only | stopped or running (read-only) |

Safety rules for any command that touches existing data:
- The `memories` table is the source of truth and is **never modified** by these phases.
  The tantivy index and vector table are *derived*; rebuilding them must be safe to retry.
- Recommend a copy of the SQLite file and the tantivy directory before the Phase 1
  upgrade (documented in the release notes, Phase 8).
- Note the existing `reindex` command only rebuilds the **vector** table
  (`sqlite_reindexer.rs`). There is currently **no** bulk path to rebuild the tantivy
  index (it is only written by `TextIndex::upsert` on save/update). Phase 1 adds it.

---

## 5. Phase map

| Phase | Deliverable | Branch | Ships on its own? |
|---|---|---|---|
| 0 | Eval harness measures MRR + latency; new failing regression cases recorded | `chore/recall-eval-instrumentation` | yes (tooling only) |
| 1 | Keyword leg ignores function words; text index rebuild path | `fix/keyword-stopwords` | yes |
| 2 | Relevance calibrated per embedding model; negative eval cases | `fix/relevance-calibration` | yes |
| 3 | **Go/no-go gate** on measured numbers (decision record) | `docs/rerank-gate` | yes (docs) |
| 4 | `Reranker` contract + fake + recaller integration, modes `off`/`shadow`/`on` | `feat/rerank-contract` | yes (inert: no real model) |
| 5 | fastembed `TextRerank` adapter, config, wiring, model warm-up | `feat/rerank-fastembed` | yes (off by default) |
| 6 | Shadow telemetry + `rerank audit` CLI over existing memories | `feat/rerank-audit` | yes |
| 7 | Tuning against the eval; strategy/weight/depth decision; relevance decision | `feat/rerank-tuning` | yes |
| 8 | Docs, deploy, default decision, release | `docs/rerank-release` | yes |

If the Phase 3 gate says **no-go**, stop after Phase 3, close this plan with the recorded
numbers, and skip Phases 4–8.

---

## 6. Phase 0 — Measure before changing anything

**Goal.** The eval can detect the defects described in §1 and report what the
reranker will cost, *before* any retrieval code changes.

**Branch:** `chore/recall-eval-instrumentation`

### Tasks

- **0.1 Confirm what the eval runs against.** `eval/cases.toml` states it runs "against the
  real embedding model". `VERIFY` which embedder `recuerdos-ai eval --config …` constructs:
  the one in the given config, or a hard-coded local `bge-small`. Production uses
  `gemini-embedding-2`. Tuning on a different embedder than production does not transfer
  (baseline, weights and floors all differ).
  - If it follows `--config`: document the command used for the production-like run in §12
    (e.g. `cargo run -q --bin recuerdos-ai -- eval --config recuerdos-ai.toml …`) and the env var
    holding the provider key.
  - If it does not: make it follow `--config`. Add a test for the wiring where feasible.
- **0.2 Add metrics to `Report`** (`src/bootstrap/eval.rs`): `mrr` (mean reciprocal rank of
  the first expected memory within the top K, 0–100 scale like the others),
  `latency_ms_p50`, `latency_ms_p95` (per recall call, measured around `MemoryRecaller::execute`
  or the use case the eval calls), and `precision_at_1` per `by_kind`.
  - All new fields use `#[serde(default)]` so the committed `eval/baseline.json` still deserialises.
  - Print them in the summary line.
  - `compare()` additionally fails when `precision_at_1` or `mrr` drops by more than
    `--max-drop` points (today it gates on `recall_at_k` only). Keep the old behaviour for
    old baselines that lack the fields (skip the comparison when the baseline value is
    absent/default-zero **and** documented as such).
- **0.3 Add regression cases to `eval/cases.toml`** (new `kind`s, so they appear in `by_kind`):
  - `nl-question` — the natural-language form of at least six existing cases, with filler
    words ("what do I use for…", "which … do I prefer", "how should I …"). The expected
    memory is the same as the topical form.
  - `nl-category` — abstract/category queries in the style of "anything about how I like to
    work", "deployment and hosting", "health, fitness or family" (add the corresponding
    corpus memories if absent: a family/location fact, a hosting/deploy fact, a database rule).
  - `negative` — queries that match nothing in the corpus (`what and or the for do`,
    `how do I do it`, plus two unrelated topics). **Expected: empty result at the default
    `min_relevance`.** Extend the case schema with `expect_empty = true` (or reuse the existing
    mechanism if there is one) and make the report count a negative case as passed only when
    nothing is returned.
  - Keep every existing case unchanged.
- **0.4 Record the "before" numbers** (on this branch, *before* Phases 1–2) in §12:
  full `eval` output, and the per-case ranks for the new kinds. The new `nl-*` and
  `negative` cases are **expected to fail or score poorly now** — that is the point.
- **0.5 Do not commit a new `eval/baseline.json` yet.** The committed baseline stays the
  pre-plan one until Phase 2 re-records it (otherwise CI would pin the defect).
  If CI runs `eval --baseline`, temporarily mark the new kinds as informational (a
  `[[case]] informational = true` flag, or a separate cases file) so the pipeline stays
  green; Phase 2 removes the flag.

### Tests

- Unit tests for MRR, percentile maths (empty set, single sample, ties), backward-compatible
  deserialisation of the old `baseline.json`, `expect_empty` pass/fail logic, and `compare()`
  on the new fields.

### Definition of Done

- [x] `just check-native` passes — **as written** (`fmt --check` + `clippy --all-targets
      -- -D warnings` + boundary script + `cargo test`), exit 0. The 11 `double_must_use`
      errors in `#[async_trait]` code were verified pre-existing on clean `main` under the
      rebuilt `rust:1-slim` (Rust 1.99); fixed in this branch with a documented
      crate-level `[lints.clippy]` allowance — see the §12 toolchain note.
- [x] `cargo run -q --bin recuerdos-ai -- eval` prints recall@5, precision@1, MRR, p50/p95 latency,
      and per-kind scores, including the new kinds.
- [x] Old `eval/baseline.json` still loads; `eval --baseline eval/baseline.json` still runs
      (gated subset unchanged: 91.7% recall@5 / 62.5% precision@1).
- [x] §12 contains the "before" numbers and states which embedder the eval used
      (full per-case tables for both embedders; `compare` now checks the recorded embedder too).
- [x] The new `nl-question`, `nl-category`, `negative` cases exist, and at least the stopword-only
      `negative` case demonstrably **fails** on this branch — it fails under the default
      (bge-small) eval with 5 results at relevance 0.28–0.32. *(Note, recorded as a Phase 2
      input: the same case passes under the production-like gemini config on this corpus;
      §1's live-store figures were 0.84–0.91 — see the negative-case table in §12.)*

### Expected state after this phase

Production behaviour is unchanged. The repo now has a regression suite that reproduces the
defects and a harness that can show a latency cost.

---

## 7. Phase 1 — Keyword leg: ignore function words, rebuild the index

**Goal.** Natural-language queries stop matching memories on "what/and/or/the/do/I", and
existing deployments get a correct keyword index without data loss.

**Branch:** `fix/keyword-stopwords`

### Tasks

- **1.1 Project-owned stopword list.** New `src/memories/infrastructure/stopwords.rs` exporting a
  `&[&str]` (lowercase). Contents = tantivy's English list **plus** the words natural
  questions are made of, minimum: question words (`what which who whom whose when where why how`),
  auxiliaries/modals (`do does did am is are was were be been being have has had can could
  should would may might must shall will`), pronouns/determiners (`i me my mine we our you your
  he she it its they them their this that these those`), and common glue (`about from with
  into over under than then so such any all some`). Keep it short and defensible; each
  addition is a trade-off (e.g. `not`/`no` hurt negation, but the vector leg carries negation
  well, whereas BM25 on `not` is noise). **Do not** include domain words (`use`, `user`, `prefer`
  — they carry signal in this corpus; revisit only if the negative cases demand it).
  Document the rationale in a module comment in the style of `recall_ranker.rs`.
- **1.2 Custom analyzer.** In `TantivyTextIndex::open`, register a tokenizer (name it
  `ra_text_v2`): `SimpleTokenizer` → `RemoveLongFilter` → `LowerCaser` →
  `StopWordFilter::remove(<list>)`, and set it on the `content`, `tags` and `category` fields
  via `TextFieldIndexing::default().set_tokenizer("ra_text_v2")` with positions as before.
  `VERIFY` the exact tantivy 0.25 API names (`StopWordFilter::remove` vs `::new(Language)`).
  Tokenizer registrations are **not persisted** in the index: register on every `Index` open
  (in `user_index`), not just once at creation.
  - Because the query parser uses each field's tokenizer, the same list is applied to queries
    automatically. Add a test that proves it (a query consisting only of stopwords yields no hits
    and no error).
  - A query that reduces to zero terms must return `Ok(vec![])` (the keyword leg goes silent; the
    vector leg still answers).
  - Exact tokens must still work: `useQuery`, `pnpm`, `RA-1234`, `index.ts` (see the existing
    `exact-token` kind — it must stay at its baseline).
- **1.3 Versioned index location.** `Index::open_in_dir` reuses the *stored* schema, so an old
  index would keep referencing the `default` tokenizer and silently keep the old behaviour. Put the
  index under a versioned subdirectory (e.g. `<root>/<user>/v2/` or `<root>/v2/<user>/`), so the old
  directory is simply ignored. Add a constant `TEXT_INDEX_VERSION` next to the analyzer so the
  next schema change has an obvious place to bump.
- **1.4 Rebuild path (existing memories).**
  1. A bulk rebuild use case in `memories/application` (name by convention, e.g.
     `TextIndexRebuilder`): for each user, read all non-deleted memories from the repository and
     `TextIndex::upsert` them. Commit **in batches** (today `upsert` commits per document; add a
     `upsert_many`/batched variant to the trait or an implementation-level bulk method so a
     10 000-memory rebuild is not 10 000 commits). Idempotent; safe to retry; never writes to
     `memories`.
  2. **Automatic**: at daemon start, if a user has memories but no `v2` index directory, rebuild
     before serving, logging counts and duration at `info`. (Personal-scale corpora rebuild in
     seconds; no model calls.) If the rebuild fails, fail startup with a message pointing at the
     manual command — a silently empty keyword index is worse than a refused start.
  3. **Manual**: `recuerdos-ai reindex --text` (extend the existing `Reindex` subcommand with
     `--text` / `--vectors` flags; with neither flag keep today's behaviour = vectors only, so
     existing scripts do not change). Prints per-user counts. Document that the daemon must be
     stopped for the manual path (tantivy writer lock).
- **1.5 Eval cases.** Ensure the Phase 0 `nl-*` and `negative` cases pass or improve; add two
  more exact-token cases proving identifiers survive the analyzer.

### Tests

- Analyzer: stopword-only query → no hits; mixed query ("what do I use for data storage") → only
  content terms participate (assert on the matched ids, not on score values); casing and
  punctuation behave as before; identifiers (`useQuery`, `RA-1234`) still match exactly.
- Index versioning: opening with only a legacy directory present yields an empty v2 index, then
  `TextIndexRebuilder` populates it from a seeded repository; a second run is a no-op/identical.
- Rebuilder: multi-user isolation (user A's memories never land in user B's index), soft-deleted
  memories are skipped, and a failure mid-way leaves a state a retry fixes.
- CLI: `reindex --text` against a temp data dir with the daemon not running.
- Recaller-level: the stopword-only query returns an empty list at the default floor **once Phase 2
  lands**; in this phase assert only that the *keyword leg* contributes nothing.

### Definition of Done

- [ ] `just check-native` passes (including a boundary-check pass for any new module).
- [ ] `eval` has **no regression** in any pre-existing `by_kind` (notably `exact-token` stays 100 %,
      `needle`, `paraphrase`, `relational` within `--max-drop`), and the new `nl-question` /
      `nl-category` kinds improve vs the Phase 0 numbers (record both in §12).
- [ ] Upgrading a copy of a real data directory (pre-change) → start daemon → keyword recall works with
      no manual step; `recuerdos-ai reindex --text` gives the same index. Documented procedure in §12.
- [ ] Re-running the four queries from §1 against the upgraded copy: the right memory moves up
      or stays; record the new ranks.

### Expected state after this phase

The keyword leg no longer rewards function words. Existing deployments rebuild their text index
automatically. `relevance` values may still be inflated (Phase 2). Nothing about reranking exists yet.

---

## 8. Phase 2 — Relevance calibration per embedding model

**Goal.** `relevance` and `min_relevance` mean the same thing regardless of which embedding model
is configured; a swap of `[embeddings].model` can no longer silently break the floor.

**Branch:** `fix/relevance-calibration`

### Tasks

- **2.1 Measure.** Add a small, repeatable measurement (hidden CLI subcommand or an `eval`
  report section) that, for the configured embedder, reports: the maximum raw cosine of the
  `negative` queries against the corpus ("noise ceiling") and the minimum raw cosine of each
  case's expected memory ("signal floor"). Record both for `gemini-embedding-2` and
  `bge-small-en-v1.5` in §12. This reproduces the method documented in `recall_ranker.rs`
  (noise ≤ 0.09 / real ≥ 0.36 after correction) for the current model.
- **2.2 Per-model default baseline.** Replace the single constant default with a lookup keyed
  by `Embedder::model_id()`: `bge-small-en-v1.5 → 0.5`, `gemini-embedding-2 → <measured>`, unknown
  model → `0.5` plus a `warn!` at startup ("similarity_baseline not calibrated for model X; recall
  relevance may be miscalibrated"). An explicit `[retrieval].similarity_baseline` in the config
  still wins. Keep `RelevanceCalibration::new` and the config validation range (`0.0..=0.99`).
  Update the doc comments on `similarity_baseline` and `DEFAULT_SIMILARITY_BASELINE` with the new
  measurements and the reason a map exists.
- **2.3 Gemini normalisation check.** `similarity_from_l2` assumes unit-length vectors
  (cosine = 1 − d²/2). `VERIFY` that `gemini_embedder.rs` returns normalised vectors for the
  configured dimension (truncated Matryoshka outputs generally are **not** unit-length). If not,
  L2-normalise in the embedder (and note that this changes stored-vs-query scales ⇒ a `reindex`
  is needed for already-stored vectors; document it in the release notes).
- **2.4 Floor check.** Confirm `DEFAULT_RELEVANCE_FLOOR` (0.25) sits inside the measured gap for
  the new calibration; adjust only if the measurement says so, and update the test in
  `recall_ranker.rs` that pins "floor vs measured numbers".
- **2.5 Re-record the baseline.** `just eval-record` on the production-like config; remove any
  `informational` flags from Phase 0.5; commit the new `eval/baseline.json`.

### Tests

- Unit: baseline map lookup (known, unknown + warning), config override precedence, validation.
- Property-style test: with the calibrated baseline, a random-text query produces `relevance`
  below the floor for every corpus memory in the eval corpus.
- Recaller: the stopword-only query now returns an **empty** result at the default floor.

### Definition of Done

- [ ] `just check-native` passes.
- [ ] Every `negative` eval case passes (empty result at the default floor) under the
      production-like config.
- [ ] No pre-existing `by_kind` regresses; recorded in §12.
- [ ] The four queries from §1, re-run live: irrelevant memories no longer show relevance ≥ 0.5
      (record the numbers).
- [ ] New `eval/baseline.json` committed with p@1, MRR and latency fields populated.

### Expected state after this phase

The two free defects are fixed. `eval/baseline.json` now reflects "best achievable without a
second-stage ranker". This is the baseline the reranker must beat.

---

## 9. Phase 3 — Go / no-go gate (decision record)

**Goal.** Decide with numbers whether to build the reranker. No application code changes.

**Branch:** `docs/rerank-gate`

### Tasks

- **3.1** Re-run the eval and the four live queries; paste results into §12.
- **3.2** Compute the **headroom**: `recall@5 − precision@1` overall and for `nl-question`,
  `nl-category`, `needle`, `paraphrase`.
- **3.3** Fill in the decision table below and commit it in §12.

### Decision rule (adjust here, in writing, if the team disagrees)

| Condition (after Phase 2) | Decision |
|---|---|
| `recall@5 − precision@1 < 10` points **and** all `nl-*` cases rank the expected memory at #1 | **NO-GO.** Stop. Record the outcome; close the plan. |
| Headroom ≥ 10 points **or** ≥ 2 `nl-*` cases still miss #1 | **GO.** Continue to Phase 4. |
| Headroom ≥ 10 points but corpus < 100 memories | **GO, cautiously.** Prefer shadow mode (Phase 6) on real data before any default change. |

### Definition of Done

- [ ] §12 contains the filled decision table with the measured numbers and the decision.
- [ ] If NO-GO: plan marked closed in the header; Phases 4–8 are not implemented.

### Expected state after this phase

A recorded, defensible decision. Either the work stops here with the cheaper fixes shipped, or
the next phases proceed with a baseline to beat.

---

## 10. Phases 4–8 (only after a GO)

### Phase 4 — Reranker contract and recaller integration (no real model)

**Goal.** Recall can accept an optional second-stage ranker, in `off` / `shadow` / `on` modes,
proven with a deterministic fake. Zero behaviour change when absent.

**Branch:** `feat/rerank-contract`

#### Tasks

- **4.1 Contract** — `src/memories/domain/reranker.rs`:
  ```rust
  pub trait Reranker: Send + Sync {
      /// Scores each (query, text) pair. The returned scores are aligned with `candidates`
      /// (same length, same order). Higher is better; the scale is model-specific.
      fn score(&self, query: &str, candidates: &[RerankCandidate<'_>]) -> Result<Vec<f32>>;
  }
  pub struct RerankCandidate<'a> { pub id: MemoryId, pub text: &'a str }
  ```
  Aligned-scores (not a sorted list) keeps the raw value available and removes an ordering
  contract the caller must trust. The contract is owned by `memories`; export from `domain/mod.rs`.
- **4.2 Fake** — `FakeReranker` in `application/test_doubles.rs` (deterministic, e.g. scores by
  token overlap; plus a variant that always errors and one that sleeps, for degradation/timeout tests).
- **4.3 Ranker support** — `RecallRanker` gains a rerank input without altering existing entry
  points. Keep `rank` and `rank_with_graph` as-is (thin wrappers over a new general entry
  point, e.g. `rank_with_legs`). `ScoredMemory` gains `rerank_score: Option<f32>` (**not** on
  `MatchDetail`, which derives `Eq` and would lose it to the `f32`); `MatchDetail` gains
  `rerank_rank: Option<usize>` (a `usize` keeps `Eq`). Two strategies behind an internal enum
  (`Rrf { weight }` and `Replace`), selected by config in Phase 5; implement both now, default `Rrf`.
  Document the calibration argument in the module docs: why weights above 1 are in range, how
  the bounded recency/quality band (`MULTIPLIER_FLOOR`) interacts with each strategy.
- **4.4 Recaller flow** — `MemoryRecaller` gains `reranker: Option<Arc<dyn Reranker>>`, `mode`,
  `depth`, `timeout`. After the existing fusion and filtering, take the top `depth` candidates
  (clamped to the candidate pool from `RecallQuery::candidate_depth`), call the reranker through a
  `rerank_leg()` that mirrors `graph_leg()` (best-effort, `warn!` + fall back to the pre-rerank order
  on error **or** deadline), then re-rank with the rerank input:
  - `off` (or `reranker: None`): the pre-existing code path, untouched.
  - `shadow`: compute the reranked ordering, `tracing::info!` the comparison (ids and ranks only —
    **never memory content or the raw query**; log a short hash of the query), but return the
    **pre-rerank** ordering.
  - `on`: return the reranked ordering.
  - `relevance` is computed exactly as today in all modes (invariant I-4).
  - Memories beyond `depth` keep their relative order after the reranked head.
  - Skip reranking when there are fewer than 2 candidates.
- **4.5 Surface the new fields additively.** Add `rerank_score` / `rerank_rank` to the REST and MCP
  match-detail output only when present (`skip_serializing_if`); update the OpenAPI/schema docs,
  and run the Python SDK tests (`just sdk-test`) to confirm old clients are unaffected.

#### Tests

- **Identity:** with `reranker: None` (and with `mode = off` and a reranker present), results are
  identical — same ids, order, `score`, `relevance`, `match_detail` — to the pre-change build,
  over the existing recaller test fixtures. This test is the phase's anchor.
- Fake reranker reverses a fixture ordering → `on` returns the reversed head; `shadow` returns the
  original and logs a diff (capture via a tracing test layer).
- Error and timeout: reranker errors / exceeds the deadline → original order, request succeeds,
  `warn!` emitted.
- Depth: `depth` larger than candidates, `depth = 1`, empty candidates, single candidate.
- Strategy tests: `Rrf` weight 0 ≡ off; weight ≫ 1 ≈ rerank order; `Replace` respects the
  recency/quality band (a fresher, equal-score memory wins a tie but cannot leapfrog a clear
  rerank-score gap).
- Filter interaction: category/tag/since filters are applied before reranking (reranker never
  sees filtered-out memories); `min_relevance` still applied with the unchanged `relevance`.

#### Definition of Done

- [ ] `just check-native` passes; `just sdk-test` passes.
- [ ] The identity test exists and passes.
- [ ] `eval` numbers identical to Phase 3 (no reranker is wired yet) — recorded in §12.
- [ ] Module docs in `recall_ranker.rs` describe both strategies and the weight calibration.

#### Expected state after this phase

The seam exists and is fully tested with a fake. Production behaviour is unchanged because no
real reranker is wired.

---

### Phase 5 — fastembed cross-encoder adapter, config and wiring

**Goal.** A real cross-encoder can be enabled by configuration, loads at boot, and is bounded in
time and concurrency. Off by default.

**Branch:** `feat/rerank-fastembed`

#### Tasks

- **5.1 Adapter** — `src/providers/infrastructure/rerank/fastembed_reranker.rs` (next to the
  embedders; `providers` already owns model-backed infrastructure — follow the existing boundary
  rules) implementing `Reranker` over `fastembed::TextRerank`. Handle: model selection from the
  config string (map to `RerankerModel` variants or `UserDefinedRerankingModel`), cache dir shared
  with the embedder (`[embeddings].cache_dir` convention), batch size, `max_length` (short memories ⇒
  prefer 128–256 with dynamic padding rather than padding everything), thread-safety (`VERIFY`
  whether `TextRerank::rerank` needs `&mut self`; wrap in a `Mutex` only if it does and measure
  the contention), and a **semaphore** capping concurrent inferences (`max_concurrency`) so recall
  bursts cannot starve ingest on the shared blocking pool (`shared/blocking.rs`).
- **5.2 Config** — `[rerank]` in `bootstrap/config.rs`, mirroring `GraphConfig`, with doc comments
  in the calibrated style of the existing ones:
  ```toml
  [rerank]
  mode            = "off"      # off | shadow | on
  model           = "jina-reranker-v1-turbo-en"   # placeholder: pick in Phase 7 from the measured grid
  depth           = 30         # candidates reranked per recall
  strategy        = "rrf"      # rrf | replace
  weight          = 1.0        # RRF weight; >1 lets the reranker overrule vector+BM25
  timeout_ms      = 400        # beyond this the request falls back to the pre-rerank order
  max_concurrency = 2
  batch_size      = 16
  ```
  Validation next to the graph validation: `depth ≥ 1`, `weight ≥ 0`, `timeout_ms ≥ 1`,
  `max_concurrency ≥ 1`, known `mode`/`strategy`; unknown `model` with `mode != off` is an error.
  Env override convention (`RECUERDOS_AI_*`) and `recuerdos-ai config` output must show the
  effective values (no secrets involved).
- **5.3 Wiring** — `bootstrap/memories_wiring.rs`: build the reranker **only** when `mode != off`;
  a load/download failure with `mode != off` fails startup with a clear message (like embeddings).
  `mode = off` must not touch the network, load a model or add any startup cost.
- **5.4 Model warm-up** — extend `warm-models` to also download the rerank model when configured, so
  the release image can bake it in; update `docker/Dockerfile` accordingly (`VERIFY` the current
  warm step) and note the extra image size and RAM in the docs. If the API allows pinning a
  revision/checksum, do.
- **5.5 Test-only integration** — an `#[ignore]` (or feature-gated, like the other model-dependent
  tests; follow the existing convention) test that loads the real model, scores a tiny fixed set, and
  asserts the relevant passage outscores an unrelated one. Not run in the default `cargo test`.

#### Tests

- Config: parsing, defaults (`mode = off`), validation errors, env overrides.
- Wiring: `mode = off` constructs no reranker (assert with a spy/counter); `shadow`/`on` construct
  one; a construction error with `mode != off` aborts startup.
- Semaphore: with `max_concurrency = 1`, two concurrent scores serialise (fake adapter with a
  barrier).
- Real-model test (ignored by default): relevant > unrelated; batch of > `batch_size` items returns
  aligned scores.

#### Definition of Done

- [ ] `just check-native` passes (default build downloads nothing; boundary script passes).
- [ ] With the default config the binary behaves exactly like before (identity test from Phase 4 passes
      through the real wiring).
- [ ] With `mode = "on"` and the model warmed, `recuerdos-ai eval` runs end to end and prints the
      reranked metrics + latency (numbers recorded in §12; not yet judged).
- [ ] `recuerdos-ai warm-models` downloads the rerank model when configured.
- [ ] Documented measurements: model size on disk, RAM delta at idle and under load, cold-start time.

#### Expected state after this phase

The feature exists end to end, behind `mode`, defaulting to off. Quality is not yet claimed.

---

### Phase 6 — Shadow telemetry and the `rerank audit` command (existing memories)

**Goal.** Judge the reranker on **real, existing memories and real queries** without changing what
any client sees and without mutating any stored state. This is the operator-facing answer to
"what do we do with the memories we already have".

**Branch:** `feat/rerank-audit`

#### Tasks

- **6.1 Side-effect-free recall.** Add an option to the recall execution (e.g. an
  `ExecuteOptions { track_access: bool }` or a dedicated `execute_untracked`) that skips
  `touch_accessed`. All audit/eval replays use it. Default behaviour for real requests is unchanged.
- **6.2 `rerank audit` subcommand** (hidden, like `eval`; operator/dev tool):
  ```
  recuerdos-ai rerank audit --user <name> --queries <file> [--limit 8] [--depth N] [--config <path>] [--json]
  ```
  - `<file>`: one query per line (or TOML with optional `expect = "substring of the right memory"`).
  - For each query, run recall twice over the user's **real** store — current pipeline vs
    reranked — using the untracked path, and print: query, the top-N of each ordering with ids,
    category, a content snippet (operator terminal only; truncate), `relevance`, `rerank_score`, and
    the rank delta; flag queries where the **top-1 changed**.
  - Summary: % of queries where top-1 changed, mean rank displacement, p50/p95 rerank latency, and,
    when `expect` is given, p@1 / MRR before vs after.
  - **Read-only:** opens the database and indexes read-only (or never writes); verify with a test
    that `memories`, access counters and importance are byte-identical after a run.
  - User selection follows how `graph backfill` / `consolidate` enumerate users; `--user` required
    when several exist.
  - Works with the daemon running (read-only) or stopped; if a tantivy writer lock prevents the
    read-only open, document "stop the daemon" instead of weakening the guarantee.
- **6.3 Shadow-mode logs** (from Phase 4) get a stable event name and fields so they can be grepped:
  `rerank_shadow{ query_hash, top1_before, top1_after, changed, rerank_ms, depth }` — ids only.
  Document how to compute "disagreement rate" from the logs.
- **6.4 No "rerank all" mutation command is provided, by design** (see §4). The audit command is the
  supported way to run the reranker over existing memories.

#### Tests

- Audit over a seeded store with the fake reranker: deterministic output, `--json` shape, top-1-changed
  flag, summary arithmetic.
- Purity: run audit, then assert `touch_accessed`-driven fields (access count / last accessed / importance)
  and every `memories` row are unchanged.
- Multi-user: `--user` isolation; unknown user is a clear error.
- Shadow logs: capture via a tracing test layer; assert no content or raw query appears in any field.

#### Definition of Done

- [ ] `just check-native` passes.
- [ ] `recuerdos-ai rerank audit` runs against a **copy** of the real data directory with the real
      model and a file containing at least the four queries from §1 plus ten of the operator's own
      common queries; output pasted into §12.
- [ ] Purity test passes.
- [ ] Shadow mode run for a representative period (≥ 3 days or ≥ 200 real recalls, whichever first)
      with the disagreement rate and latency recorded in §12 (skip the duration clause only if the
      store has no live traffic, and say so).

#### Expected state after this phase

Evidence from real data exists. Clients still see the unreranked order unless they opted in.

---

### Phase 7 — Tuning and the ship decision

**Goal.** Choose model, strategy, depth and weight from measurements, not taste; decide whether
`relevance` changes.

**Branch:** `feat/rerank-tuning`

#### Tasks

- **7.1 Grow the eval where reranking should matter**: near-miss distractors, memories that read alike
  but answer different questions, declarative-statement vs question phrasing, abstract-category
  queries (≥ 15 new cases; keep the corpus ≥ 100 memories with realistic filler so the numbers are
  not trivially perfect). Re-record nothing yet; just measure.
- **7.2 Offline grid**, run by a script under `scripts/` (checked in, reproducible, prints a table to
  paste in §12):
  `model ∈ {candidates the team can afford}` × `strategy ∈ {rrf, replace}` × `depth ∈ {10, 20, 30, 50}`
  × `weight ∈ {0.5, 1, 2, 4}` (weight only for `rrf`). Record per cell: p@1, MRR, recall@5, per-kind
  p@1, p50/p95 latency.
- **7.3 Pick the winner** by the ship gate below; document *why* the weight is what it is in the config
  and ranker doc comments (invariant I-3).
- **7.4 Relevance decision** (separate, explicit): measure the distribution of cross-encoder scores for
  (a) expected memories and (b) `negative` queries on the production-like corpus. Only if (a) and (b)
  are cleanly separable, define a documented mapping to `relevance` (or a blend with the existing
  signal) and recalibrate `DEFAULT_RELEVANCE_FLOOR`; otherwise keep `relevance` untouched and say so.
  Never ship a relevance change without the `negative` and floor-regression cases passing.
- **7.5 Commit** the chosen defaults, a re-recorded `eval/baseline.json`, and the latency delta in the
  commit message (the honest cost of the feature).

#### Ship gate (all must hold; adjust only in §12 with a reason)

- [ ] precision@1 improves by **≥ 5 points** absolute vs the Phase 2 baseline (≥ 15 points with the
      expanded Phase 7.1 cases is expected if the thesis is right; record actual).
- [ ] MRR does not decrease; recall@5 decreases by **≤ 2 points**; no `by_kind` p@1 regresses by > 5 points.
- [ ] All `negative` cases still pass.
- [ ] p95 added latency at the chosen depth on the **deployment hardware** ≤ the budget written in §12
      (suggested: ≤ 150 ms; the deadline `timeout_ms` is set above p95 but below the client-visible limit).
- [ ] Phase 6 shadow/audit data on real memories shows no systematic regression (inspect every
      top-1-changed case by hand and record the verdicts).

#### Definition of Done

- [ ] `just check-native` passes; `just eval-native` passes against the new baseline.
- [ ] Grid table, winner, and rationale recorded in §12.
- [ ] Either the ship gate is met, **or** the plan records "reranker stays off" with the numbers and stops.

#### Expected state after this phase

A tuned, measured configuration exists. The default is still `mode = off` until Phase 8.

---

### Phase 8 — Docs, deployment and release

**Goal.** Operators can adopt (or consciously skip) the feature; the default is set according to
Phase 7.

**Branch:** `docs/rerank-release`

#### Tasks

- **8.1 Docs**: README recall description (vector + BM25 + graph + optional rerank), `recuerdos-ai.example.toml`
  `[rerank]` block with calibrated comments, `docs/architecture.md`, project-plan.md §5/§7 cross-reference,
  and a short "Recall quality" troubleshooting note ("results look random? check stopwords/baseline/
  `similarity_baseline` for your model"). State the **language limitation** of the chosen model
  plainly and list the multilingual alternatives and their cost.
- **8.2 Upgrade notes**: back up the data directory; first start rebuilds the text index; `reindex --text`;
  Gemini normalisation `reindex` if Phase 2.3 required it; model download volume/baking instructions;
  expected RAM and image size delta; how to run `rerank audit` before enabling.
- **8.3 Default**: if the ship gate passed and the operator accepts the cost, set `mode = "on"` as the
  default in a **separate commit** (revertable in isolation, the way the graph was enabled in Task 7.3.6);
  otherwise leave `mode = "off"`. Update the `recuerdos-ai config` output expectations accordingly.
- **8.4 Release**: version bump and notes following the existing `chore(release)` convention; run the SDK
  suite and the release image build (`just docker-build`), start the image with the default config, and
  with `mode = "on"` and a baked model.
- **8.5 Close out**: add a Task entry for this work to `implementation-plan.md` (as a Phase 7.x record, same
  shape as Task 7.3) pointing back here.

#### Definition of Done

- [ ] `just check-native`, `just sdk-test`, `just docker-build` all succeed.
- [ ] A clean container started from the release image with the documented env/config answers a
      recall in the chosen mode (smoke test recorded in §12).
- [ ] Docs reviewed for accuracy against the final config keys.
- [ ] §12 is complete; the header status line of this file is updated to "shipped" / "closed (no-go)".

#### Expected state after this phase

The work is released and documented, or consciously closed with the evidence recorded.

---

## 11. Risks and follow-ups

| Risk | Mitigation |
|---|---|
| Stopword list removes a word that matters (`not`, `no`, `it`) | Vector leg carries negation; exact-token and negative eval cases guard identifiers; list is one small, reviewed file. |
| Auto text-index rebuild on startup is slow on a large store | Batched commits; log progress; manual command exists; personal-scale corpora take seconds. |
| Cross-encoder under-scores declarative memories (trained on Q→passage) | Don't touch `relevance`; Phase 7.1 adds statement-style cases; shadow/audit on real data before default. |
| Reranker muted by RRF or over-trusted by `replace` | Both strategies measured; weight range includes >1; recency/quality band bounded as documented. |
| Latency / RAM on the deployment box | Deadline + fallback, concurrency cap, `depth` dial, measurements in Phase 5 DoD. |
| English-only model vs multilingual users | Documented; multilingual option costed in the grid; users may stay `off`. |
| Eval corpus too small to separate strategies | Phase 7.1 grows it; ship gate requires real-data evidence from Phase 6. |
| Model download at runtime in production | `warm-models` + baked image + persistent volume; fail loudly at boot if enabled and missing. |

**Follow-ups (not in this plan):** stemming in the keyword leg; query-result cache keyed by
`(query hash, memory id, content hash)` for the reranker; concept/cluster nodes on the entity graph
(only if Phase 3/7 evidence shows failures the ranker cannot fix); pushing filters into the indexes.

---

## 12. Results log

Fill in as phases complete. Paste raw command output, not summaries.

### Environment
- Embedding model used by eval: **two runs recorded below.** `eval` without `--config`
  uses the built-in default (`local`, `bge-small-en-v1.5`, 384 dims — the model the
  committed baseline was recorded against); with `--config recuerdos-ai.toml` it now
  runs against production's `gemini-embedding-2` (3072 dims), as the printed line
  `embedder: gemini-embedding-2 (3072 dimensions)` confirms. See Phase 0.1 below —
  **it did not follow `--config` before Phase 0; it does now.**
- Eval command (production-like):
  `docker compose run --rm dev cargo run -q --bin recuerdos-ai -- eval --config recuerdos-ai.toml`
  (native equivalent: `cargo run -q --bin recuerdos-ai -- eval --config recuerdos-ai.toml`).
  Provider key: the `GEMINI_API_KEY` env var named by `[embeddings].api_key_env`
  (supplied from `.env` via the dev container's `env_file`).
- Hardware used for latency numbers: MacBook Pro **M4 Pro, 48 GB**, Docker Desktop
  (container build 8 vCPU-equivalent Rosetta/native arm64). Native `cargo` is not
  installed on this host, so `just check` / `just eval-native` (Docker) stand in for
  the `-native` recipes.

### Phase 0 — before

**0.1 resolved (was VERIFY): the eval did NOT follow `--config`.** `run_eval` loaded
the config only for `model_cache_dir`; `eval::seed` assembled `Memories` from
`AppConfig::default()` — so before this phase every eval ran `local bge-small-en-v1.5`
regardless of the production `gemini-embedding-2`. Fixed: `eval::run(cases, &AppConfig)`
now builds the whole instance from the loaded config (embedder, floor, ranker tuning);
only `[storage].path` (throwaway dir) and `[graph].enabled = true` are overridden. The
run prints the embedder it used. Coverage, stated precisely: an `#[ignore]d` wiring
test (`the_config_passed_to_seed_reaches_the_wired_instance`, run with
`RECUERDOS_AI_MODEL_CACHE=/models`, passing) asserts the config's `min_relevance` and
embedder reach the wired `Memories`; the printed `embedder:` line is the end-to-end
proof per run.

**Toolchain (resolved in this PR):** the dev image builds from unpinned `rust:1-slim`,
which advanced to Rust/clippy 1.99 while the crate's `rust-version` is 1.85; clippy's
newer `double_must_use` lint fires 11 times on `#[async_trait]`-generated trait code —
**verified pre-existing**: with Phase 0's changes stashed, clippy 1.99 fails on clean
`main` with the same 11 errors. CI hit exactly these on this branch, so the fix landed
here: a crate-level `[lints.clippy] double_must_use = "allow"` in `Cargo.toml`, with the
reason at the site (the double `#[must_use]` is introduced by the macro expansion and
there is no call-site attribute that can satisfy it). `just check-native` now passes
*as written* — `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` +
boundary script + `cargo test` — **all green** (741 unit + all integration suites;
the ignored wiring test passes under `--ignored`).

**Latency reading notes (so these numbers are not over-read):** `latency_ms` wraps the
whole `MemoryRecaller::execute` including the embedder call, so the Gemini p50 is
remote-API latency, not local inference. One untimed warm-up recall now runs before any
timed one (cold connection/index-open otherwise lands in p95 of a ~40-sample run).
Phase 5/7 must measure the reranker's cost **paired, on/off, same machine and
embedder**, not as an absolute out of these tables.

`compare()` now warns when the baseline's recorded `embedder` differs from the run's;
the committed baseline predates the field, so every run against it prints the
re-record note. `--write-baseline` writes the **gated** subset — informational
(deliberately-failing) cases can never be pinned into `baseline.json` (task 0.5).
`expect_empty` cases are excluded from the overall recall/precision/MRR means and
reported as `abstention_rate` plus their `by_kind` line.

#### Run 1 — default config (the committed baseline's embedder)

`docker compose run --rm dev cargo run -q --bin recuerdos-ai -- eval`
(bge-small-en-v1.5; 35 scored + 4 negative cases; the 15 informational cases marked
`*` are excluded from the gate):

```
embedder: bge-small-en-v1.5 (384 dimensions)

case                                                   kind  recall  top
────────────────────────────────────────────────────────────────────────────
paraphrase: package manager                      paraphrase    100%    ·
paraphrase: import structure                     paraphrase    100%    ·
paraphrase: why this database                    paraphrase    100%    ✓
exact token: useQuery                           exact-token    100%    ✓
exact token: maintenance_work_mem               exact-token    100%    ✓
current fact: deployment target                current-fact    100%    ✓
needle: who reviews auth                             needle    100%    ✓
needle: dietary requirement                          needle      0%    ·
    missed: User is vegetarian and does not eat fish
needle: release process                              needle    100%    ✓
category filter: coding preferences only    category-filter    100%    ✓
category filter: decisions only             category-filter    100%    ✓
tag filter: typescript                           tag-filter    100%    ·
multi-hit: what does the user prefer for …       paraphrase    100%    ✓
subcategory filter: tooling only           subcategory-filter    100%    ✓
subcategory filter: structure only         subcategory-filter    100%    ✓
subcategory filter: testing only           subcategory-filter    100%    ✓
subcategory filter: excludes structure wh… subcategory-filter    100%    ✓
relational: who owns a decision about a s…       relational      0%    ·
    missed: Nadia is the tech lead of the Meridian team
relational: where a service's team sits          relational    100%    ·
relational: on-call for an app's dependen…       relational    100%    ·
relational: compliance sign-off for a flow       relational    100%    ·
relational: expert on a retired system           relational    100%    ·
relational: database behind a team's serv…       relational    100%    ✓
relational: what a pipeline ran on before        relational    100%    ✓
nl-question: package manager                    nl-question    100%    · *
nl-question: module exports                     nl-question    100%    ✓ *
nl-question: storage decision                   nl-question    100%    · *
nl-question: auth review                        nl-question    100%    · *
nl-question: shipping a release                 nl-question    100%    ✓ *
nl-question: testing tools                      nl-question    100%    ✓ *
nl-question: the failed migration               nl-question    100%    ✓ *
nl-category: deployment and hosting             nl-category     50%    ✓ *
    missed: The backend deploys on Hetzner, migrated from Fly.io because of cost
nl-category: health, fitness or family          nl-category    100%    ✓ *
nl-category: code organisation                  nl-category      0%    · *
    missed: User forbids barrel files and index.ts re-exports
    missed: User requires one export per file in TypeScript projects
nl-category: database rules                     nl-category    100%    ✓ *
negative: stopword soup                            negative      0%    · *
    missed: (expected no results at the 0.25 floor; got 5)
negative: shape of a question                      negative      0%    · *
    missed: (expected no results at the 0.25 floor; got 1)
negative: hiking trails                            negative      0%    · *
    missed: (expected no results at the 0.25 floor; got 1)
negative: bouillabaisse                            negative    100%    ✓ *
────────────────────────────────────────────────────────────────────────────
recall@5: 90.0%   precision@1: 62.9%   MRR: 74.8%   latency p50: 10.7 ms p95: 13.3 ms   (35 scored cases, 4 negative)
abstention (negatives returning nothing at the floor): 25.0%
  (15 marked * informational — excluded from the baseline gate)
   category-filter: recall 100.0%   p@1 100.0%
      current-fact: recall 100.0%   p@1 100.0%
       exact-token: recall 100.0%   p@1 100.0%
            needle: recall 66.7%   p@1 66.7%
          negative: recall 25.0%   p@1 25.0%
       nl-category: recall 62.5%   p@1 75.0%
       nl-question: recall 100.0%   p@1 57.1%
        paraphrase: recall 100.0%   p@1 50.0%
        relational: recall 85.7%   p@1 28.6%
  subcategory-filter: recall 100.0%   p@1 100.0%
        tag-filter: recall 100.0%   p@1 0.0%
```

Gate against the committed baseline (`just eval-native`):

```
note: the baseline records no embedder (it predates the field); re-record it to enable the embedder-match check

baseline recall@5: 91.7%   now: 91.7%   change: -0.0 points
baseline precision@1: 62.5%   now: 62.5%   change: -0.0 points
baseline MRR: not recorded — gate skipped (re-record the baseline to enable it)
```

Green: the gated 24-case subset is **byte-identical to the committed baseline** —
neither the two corpus memories added by 0.3 nor the harness changes moved any
pre-existing number.

#### Run 2 — production-like config

`docker compose run --rm dev cargo run -q --bin recuerdos-ai -- eval --config recuerdos-ai.toml`
(gemini-embedding-2, key from `GEMINI_API_KEY`; same 39 cases):

```
embedder: gemini-embedding-2 (3072 dimensions)

case                                                   kind  recall  top
────────────────────────────────────────────────────────────────────────────
paraphrase: package manager                      paraphrase    100%    ·
paraphrase: import structure                     paraphrase    100%    ·
paraphrase: why this database                    paraphrase    100%    ✓
exact token: useQuery                           exact-token    100%    ✓
exact token: maintenance_work_mem               exact-token    100%    ✓
current fact: deployment target                current-fact    100%    ✓
needle: who reviews auth                             needle    100%    ✓
needle: dietary requirement                          needle      0%    ·
    missed: User is vegetarian and does not eat fish
needle: release process                              needle    100%    ✓
category filter: coding preferences only    category-filter    100%    ✓
category filter: decisions only             category-filter    100%    ✓
tag filter: typescript                           tag-filter    100%    ✓
multi-hit: what does the user prefer for …       paraphrase    100%    ✓
subcategory filter: tooling only           subcategory-filter    100%    ✓
subcategory filter: structure only         subcategory-filter    100%    ✓
subcategory filter: testing only           subcategory-filter    100%    ✓
subcategory filter: excludes structure wh… subcategory-filter    100%    ✓
relational: who owns a decision about a s…       relational      0%    ·
    missed: Nadia is the tech lead of the Meridian team
relational: where a service's team sits          relational    100%    ·
relational: on-call for an app's dependen…       relational    100%    ·
relational: compliance sign-off for a flow       relational    100%    ·
relational: expert on a retired system           relational    100%    ·
relational: database behind a team's serv…       relational    100%    ✓
relational: what a pipeline ran on before        relational    100%    ✓
nl-question: package manager                    nl-question    100%    · *
nl-question: module exports                     nl-question    100%    ✓ *
nl-question: storage decision                   nl-question    100%    ✓ *
nl-question: auth review                        nl-question    100%    ✓ *
nl-question: shipping a release                 nl-question    100%    ✓ *
nl-question: testing tools                      nl-question     50%    ✓ *
    missed: User requires table-driven tests in Go, one table per behaviour
nl-question: the failed migration               nl-question    100%    ✓ *
nl-category: deployment and hosting             nl-category     50%    ✓ *
    missed: The backend deploys on Hetzner, migrated from Fly.io because of cost
nl-category: health, fitness or family          nl-category    100%    ✓ *
nl-category: code organisation                  nl-category      0%    · *
    missed: User forbids barrel files and index.ts re-exports
    missed: User requires one export per file in TypeScript projects
nl-category: database rules                     nl-category    100%    ✓ *
negative: stopword soup                            negative    100%    ✓ *
negative: shape of a question                      negative      0%    · *
    missed: (expected no results at the 0.25 floor; got 1)
negative: hiking trails                            negative      0%    · *
    missed: (expected no results at the 0.25 floor; got 1)
negative: bouillabaisse                            negative    100%    ✓ *
────────────────────────────────────────────────────────────────────────────
recall@5: 88.6%   precision@1: 71.4%   MRR: 78.9%   latency p50: 332.6 ms p95: 509.3 ms   (35 scored cases, 4 negative)
abstention (negatives returning nothing at the floor): 50.0%
  (15 marked * informational — excluded from the baseline gate)
   category-filter: recall 100.0%   p@1 100.0%
      current-fact: recall 100.0%   p@1 100.0%
       exact-token: recall 100.0%   p@1 100.0%
            needle: recall 66.7%   p@1 66.7%
          negative: recall 50.0%   p@1 50.0%
       nl-category: recall 62.5%   p@1 75.0%
       nl-question: recall 92.9%   p@1 85.7%
        paraphrase: recall 100.0%   p@1 50.0%
        relational: recall 85.7%   p@1 28.6%
  subcategory-filter: recall 100.0%   p@1 100.0%
        tag-filter: recall 100.0%   p@1 100.0%
```

#### Reconciled reading of the new kinds

- `nl-question` (7): bge-small recall 100.0 / **p@1 57.1** — finds the memory but not
  first on *package manager*, *storage decision* ("what do I use for data storage",
  the §1 query), *auth review*. Gemini: recall 92.9 / p@1 85.7 — better ranking, but
  the Go table-driven memory drops out of the top 5 under "testing tools".
- `nl-category` (4): recall 62.5 / p@1 75.0 on both. **On both embedders the exact §1
  query "deployment and hosting" misses the Hetzner memory** (the runbook answers it
  first), and "anything about how I like my code organised" misses both structure
  preferences. (Gold labels are complete per the review: the deployment case expects
  both deploy memories; the organisation query is scoped to structure.)
- `negative` (4, behind the 0.25 floor), per case on each embedder — this table is the
  reconciliation of the two runs above:

  | negative case | bge-small | gemini |
  |---|---|---|
  | `what and or the for do` | **FAIL — 5 results** (relevance 0.283–0.312, top: the pnpm preference) | pass — 0 results |
  | `how do I do it` | FAIL — 1 result (0.283) | FAIL — 1 result (0.321) |
  | `best hiking trails near the coast` | FAIL — 1 result (0.323) | FAIL — 1 result (0.323) |
  | `french recipe for bouillabaisse` | pass — 0 results | pass — 0 results |
  | abstention | **25.0%** | **50.0%** |

  **Finding that Phase 2 must not lose:** the stopword-only query is the *only*
  negative that splits — it fails on bge-small but passes on gemini at the current
  floor. That means **the production-like eval on this corpus does not reproduce the
  live-store §1 result (5 hits at relevance 0.84–0.91) for `gemini-embedding-2`.**
  The live store's numbers came from its own corpus and the 0.5 baseline; the eval
  corpus plus the 0.25 floor already suppresses that one query. Two negatives
  (`how do I do it`, `hiking trails`) *do* fail on both embedders at 0.28–0.32 —
  above the floor as noise — confirming cause 2 (miscalibration) generally, while
  task 2.1's noise-ceiling measurement should be read against *this* table, not the
  §1 anecdote.

### Phase 0 follow-ups recorded (not blocking any DoD)

- **Toolchain:** resolved in this branch via a crate-level `[lints.clippy]` allowance
  (see the §12 toolchain note). Still worth a separate follow-up: pin the toolchain
  (`rust-toolchain.toml`) so `check-native` is reproducible instead of tracking whatever
  `rust:1-slim` resolves to — the allowance means a future toolchain bump may enable new
  lints silently.
- **`compare()` skip rule:** a baseline legitimately recording `0.0` for
  `precision@1`/`MRR` is indistinguishable from "never recorded" and skips its gate.
  Accepted for now (documented at the code site); the clean fix is `Option<f64>` for
  the additive fields — cheap to do alongside Phase 2.5's re-record.
- **Gate noise:** with 24 gated cases, one flipped case moves `precision_at_1` by
  4.2 points and two exceed `--max-drop 5`. The plan's threshold stands; expect a
  one-case flake to be visible and re-run before believing a red.

### Phase 1 — keyword stopwords
- `eval` output: _TBD_ · four-query re-run: _TBD_ · upgrade-copy procedure result: _TBD_

### Phase 2 — calibration
- Noise ceiling / signal floor per model: _TBD_ · chosen `gemini-embedding-2` baseline: _TBD_
- `eval` output + new `baseline.json`: _TBD_ · four-query re-run: _TBD_

### Phase 3 — gate
| Metric | Value |
|---|---|
| recall@5 | _TBD_ |
| precision@1 | _TBD_ |
| MRR | _TBD_ |
| headroom (recall@5 − p@1) | _TBD_ |
| `nl-*` cases not at #1 | _TBD_ |
| **Decision** | _TBD (GO / NO-GO / GO-cautiously)_ |

### Phase 4 — contract
- Identity test: _TBD_ · `eval` unchanged: _TBD_

### Phase 5 — adapter
- Model size / idle RAM / loaded RAM / cold start: _TBD_ · `eval` with `mode=on`: _TBD_

### Phase 6 — audit and shadow
- `rerank audit` output: _TBD_ · shadow disagreement rate + latency: _TBD_

### Phase 7 — tuning
- Grid table: _TBD_ · winner + rationale: _TBD_ · relevance decision: _TBD_ · ship-gate checklist: _TBD_

### Phase 8 — release
- Smoke test: _TBD_ · final default (`off`/`on`): _TBD_

### `VERIFY` items resolved
- [x] eval embedder follows `--config` (0.1) — **it did not** (defaults were always
      used); fixed in Phase 0 so it does. Details in the Phase 0 log above.
- [ ] tantivy 0.25 `StopWordFilter` API and the built-in English list (1.2, §2 row 9)
- [ ] Gemini embeddings unit-normalised at configured dimension (2.3)
- [ ] fastembed rerank feature flag; `TextRerank::rerank` receiver (`&self`/`&mut self`) (5.1)
- [ ] Dockerfile warm-models step (5.4)
- [ ] Read-only open of SQLite/tantivy while the daemon runs (6.2)
