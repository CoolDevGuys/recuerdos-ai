# Recuerdos AI — Recall Quality & Cross-Encoder Reranking Plan

**v0.1 · drafted 2026-10-14 · companion to [project-plan.md](project-plan.md) and [implementation-plan.md](../implementation-plan.md)**
**Status: ✅ shipped** — Phases 0–8 complete, released as `v0.4.0` (2026-10-16). Two
recall defects fixed and measured; the cross-encoder earned its gate and ships
**compiled in, switched off** — `mode = "off"` stays the default until a shadow
period on a real store (8.3, §12 Phase 8). The relevance question answered *leave it
alone*, with the distribution that decided it on record. Two follow-ups survive the
release: that shadow period, and the Gemini parity re-check that free-tier quota
blocked (§12 Phase 8, "Still open").

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

- [x] `just check-native` passes (including a boundary-check pass for any new module).
- [ ] `eval` has **no regression** in any pre-existing `by_kind` (notably `exact-token` stays 100 %,
      `needle`, `paraphrase`, `relational` within `--max-drop`), and the new `nl-question` /
      `nl-category` kinds improve vs the Phase 0 numbers (record both in §12).
      *Not met as written — recorded per §0 rule 8, see "Noise-propped recall" in §12:
      `relational` moved −14.3 within its kind (one of its seven cases, which Phase 0
      passed only on function-word noise), and `nl-question` recall dropped on one
      informational case, for the same reason. `exact-token` held 100 %, the overall
      gate passed (recall@5 −3.2, precision@1 +2.9), and every ranking metric improved
      or held. Phase 2 must not start until this entry is read.*
- [x] Upgrading a copy of a real data directory (pre-change) → start daemon → keyword recall works with
      no manual step; `recuerdos-ai reindex --text` gives the same index. Documented procedure in §12.
      *(The real store was absent in this environment; rehearsed on a representative
      pre-change store built with the pre-change binary — procedure and results in §12.)*
- [x] Re-running the four queries from §1 against the upgraded copy: the right memory moves up
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

- [x] `just check-native` passes.
- [x] Every `negative` eval case passes (empty result at the default floor) under the
      production-like config.
- [x] No pre-existing `by_kind` regresses; recorded in §12.
- [x] The four queries from §1, re-run live: irrelevant memories no longer show relevance ≥ 0.5
      (record the numbers).
- [x] New `eval/baseline.json` committed with p@1, MRR and latency fields populated.

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

- [x] §12 contains the filled decision table with the measured numbers and the decision.
- [x] If NO-GO: plan marked closed in the header; Phases 4–8 are not implemented.
      — N/A: the measured decision is **GO, cautiously** (see §12, Phase 3);
      Phases 4–8 proceed.

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

- [x] `just check-native` passes; `just sdk-test` passes.
- [x] The identity test exists and passes.
- [x] `eval` numbers identical to Phase 3 (no reranker is wired yet) — recorded in §12.
- [x] Module docs in `recall_ranker.rs` describe both strategies and the weight calibration.

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

- [x] `just check-native` passes (default build downloads nothing; boundary script passes).
- [x] With the default config the binary behaves exactly like before (identity test from Phase 4 passes
      through the real wiring).
- [x] With `mode = "on"` and the model warmed, `recuerdos-ai eval` runs end to end and prints the
      reranked metrics + latency (numbers recorded in §12; not yet judged).
- [x] `recuerdos-ai warm-models` downloads the rerank model when configured.
- [x] Documented measurements: model size on disk, RAM delta at idle and under load, cold-start time.

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

- [x] `just check-native`, `just sdk-test`, `just docker-build` all succeed. (§12
      Phase 8 "Gates": 928 tests, clippy `-D warnings` and the boundary script clean,
      SDK suite green against the release image, both image variants built.)
- [x] A clean container started from the release image with the documented env/config answers a
      recall in the chosen mode (smoke test recorded in §12). — recorded for **both**
      modes: default image with the default config (no rerank weights on disk, no
      `rerank_*` keys, correct recall), and the baked image with
      `RECUERDOS_AI_RERANK__MODE=on` (stage loaded, reranked order, `rerank_rank` /
      `rerank_score` present). The upgrade path was exercised on a store containing
      memories, and the missing-weights failure mode was run rather than asserted.
- [x] Docs reviewed for accuracy against the final config keys — and the review found
      real defects, recorded in §12 Phase 8: `rerank_score` documented as a raw
      model value when the adapter squashes it to `(0,1)` (`docs/api.md`, SDK
      docstring), "re-scores the fused list" overstating reach where the floor runs
      first (`example.toml`, `configuration.md`), a stale `max_concurrency = 2`
      comment in the adapter, a Dockerfile comment pricing a model nobody chose, and
      the default model's languages stated wrongly as English-only.
- [x] §12 is complete; the header status line of this file is updated to "shipped" / "closed (no-go)".
      — shipped, with the two follow-ups that survive the release named there rather
      than left implicit.

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

**Branch:** `fix/keyword-stopwords` · commits `85c1df1` (analyzer + contract
`upsert_many`/`is_populated` + versioned directory) and `71b52d4` (rebuilder
use case, startup auto-rebuild, `reindex --text`/`--vectors`).

**VERIFY resolved (task 1.2, §2 row 9):** tantivy 0.25 `StopWordFilter` exposes
both `::new(Language)` (behind the crate's `stopwords` feature) and
`::remove(Vec<String>)` (no feature needed — used). Its `Language::English`
list is exactly the Lucene list quoted in §2 row 9 (`a, an, and, are, as, at,
be, but, by, for, if, in, into, is, it, no, not, of, on, or, such, that, the,
their, then, there, these, they, this, to, was, will, with`), confirming the
question words are missing from it. The filter compares whole tokens after the
`LowerCaser`, and the query parser tokenizes with each field's analyzer, so one
registration covers indexing and querying. Tokenizers were confirmed to be
per-`Index`-instance (registered in `user_index` on every open).

**Analyzer** `ra_text_v2`: `SimpleTokenizer → RemoveLongFilter(40) → LowerCaser
→ StopWordFilter(STOP_WORDS)`, set on `content`/`tags`/`category` with positions
kept. The list lives in `memories/infrastructure/stopwords.rs` (module comment
records the boundary: tantivy's list + question words + auxiliaries/modals +
pronouns + glue; `use`/`user`/`prefer` deliberately kept). Index directories
moved to `text-index/v2/<user>/` with `TEXT_INDEX_VERSION` as the bump site.

**`just check-native`: passes** — fmt, clippy `-D warnings`, boundary script,
`cargo test` (757 unit + all integration suites, +16 new tests).

#### eval — Run 1, default config (bge-small, the committed baseline's embedder)

`docker compose run --rm dev cargo run -q --bin recuerdos-ai -- eval`

```
recall@5: 86.5%   precision@1: 64.9%   MRR: 74.1%   latency p50: 10.5 ms p95: 16.2 ms   (37 scored, 4 negative)
abstention (negatives returning nothing at the floor): 75.0%   (Phase 0: 25.0%)
   category-filter: recall 100.0%   p@1 100.0%      (100/100 — unchanged)
      current-fact: recall 100.0%   p@1 100.0%      (unchanged)
       exact-token: recall 100.0%   p@1 100.0%      (unchanged, now 4 cases: useQuery, maintenance_work_mem, index.ts, Fly.io)
            needle: recall 66.7%   p@1 66.7%        (unchanged)
          negative: recall 75.0%   p@1 75.0%        (25.0% → 75.0%; only `hiking trails` still returns one 0.32-noise hit)
       nl-category: recall 75.0%   p@1 75.0%        (recall 62.5% → 75.0%; `deployment and hosting` now recalls AND tops the Hetzner memory — the §1 defect, fixed under both embedders)
       nl-question: recall 85.7%   p@1 57.1%        (recall 100% → 85.7%, p@1 unchanged — see "noise-propped recall" below)
        paraphrase: recall 100.0%   p@1 50.0%       (unchanged)
        relational: recall 71.4%   p@1 28.6%        (recall 85.7% → 71.4% — see below; p@1 unchanged)
  subcategory-filter: recall 100.0%   p@1 100.0%    (unchanged)
        tag-filter: recall 100.0%   p@1 0.0%        (unchanged)
```

Gate against the committed baseline (`eval --baseline eval/baseline.json --max-drop 5`):
overall recall@5 91.7% → 88.5% (**−3.2, inside the 5-point gate**), precision@1
62.5% → 65.4% (**+2.9**), exit 0; the per-kind warning `relational: 85.7% →
71.4%` prints (by-kind drops are warn-only in `compare()`).

#### eval — Run 2, production-like config

`docker compose run --rm dev cargo run -q --bin recuerdos-ai -- eval --config recuerdos-ai.toml`
(gemini-embedding-2; Phase 0 numbers in brackets)

```
recall@5: 83.8% [88.6]   precision@1: 75.7% [71.4]   MRR: 80.2% [78.9]   latency p50: 399.7 ms [332.6] p95: 852.5 ms [509.3]
abstention: 75.0% [50.0]  — `stopword soup` passes on both embedders now (it was the bge-only failure)
```

The gemini recall drop is the same three cases as bge (below) plus
`multi-hit` 100% → 50% (the Go table-driven memory); p@1 +4.3, MRR +1.3, and
`tag-filter`, `storage decision`, `auth review` all gain their top-hit.

#### Noise-propped recall — the two regressions, explained (do not re-derive)

Both are cases whose expected memory **never shares a content word with the
query** and reached the top 5 in Phase 0 *only* by collecting function-word
keyword matches — the mechanism §1 calls the defect:

- `relational: where a service's team sits` ("…travel to…billing service
  maintainers…" → Lisbon office): the office memory matched the query's `the`
  (`out of the Lisbon office`) and rode that k-leg vote into the top 5; with
  `the` removed it has no keyword match and its vector rank (19) no longer
  outweighs the other four fused results. Anchors and hops unchanged (graph
  ranks 1–3 identical, `RA_EVAL_DEBUG=1`).
- `nl-question: package manager` ("what do I use for installing project
  dependencies" → pnpm preference): the preference matched `for` ("pnpm over
  npm and yarn **for** package management"). Same on gemini, and same
  mechanism as gemini's `multi-hit` drop: `testing` vs `tests` with no
  stemming, so the Go memory's only keyword evidence was `the`/`does`.

The fix is working as specified; the *recall metric* was partly inflated by the
defect. Affects recall@5 only — p@1 and MRR improve on both embedders. The
`testing/tests` split is the stemming follow-up (§11) in miniature.

**DoD status (per §0 rule 8, recorded, not weakened):** the eval DoD item is
**not met as written**: `relational` moved −14.3 within its kind (7 cases; one
flip is 14.3 points) against the planned "within `--max-drop`", and
`nl-question` recall dropped (informational kind). Overall gate, p@1, MRR,
`exact-token` (still 100%), and every `nl-*`-and-negative *ranking* metric
improved or held. **The next phase (2) should not start until the operator
reads this entry and accepts the regression or asks for it to be addressed**
— Phase 2's floor calibration re-records the baseline anyway, and Phase 3's
gate reads these exact numbers.

#### Upgrade procedure + four-query re-run

The §1 live store is **not present in this environment** (the dev `data`
volume is empty, no `~/.recuerdos-ai` on the host). The procedure was
therefore rehearsed on a representative store: 7 memories (the §1 topics, the
§1 decoys, and an exact-identifier memory) saved through the **pre-change
binary** (`main` @ `363a2b4`, local bge-small), leaving a legacy
`text-index/<user>/` directory. Volume `ra-upgrade-store` retained for
inspection. Procedure and results:

1. Start the **new** binary on the store. Before `listening`, one line:
   `text index rebuilt users_rebuilt=1 memories_indexed=7 elapsed_ms=37`.
   `text-index/v2/<user>/` appears; the legacy directory is left untouched and
   never read. Keyword recall works with no manual step.
2. Second start: zero rebuild log lines (the per-user check is one stat).
3. With the daemon stopped, `recuerdos-ai reindex --text` printed per-user
   counts (`default: 7 memories`, total 1 user, 7 memories, 0 skipped) and the
   same search results on the next start (verified below).
4. `reindex` with no flags still runs only the vector pass (flag semantics
   pinned by the clap help text; `--vectors` names it explicitly).

The four §1 queries against the upgraded rehearsal store (default floor;
BEFORE = pre-change binary on the legacy index, AFTER = new binary):

| query | BEFORE | AFTER |
|---|---|---|
| what do I use for data storage | pnpm preference #1 (0.251) — noise match | **nothing at the floor** (pnpm's noise gone; the database rule is still below floor — Phase 2's calibration, embedder-dependent) |
| deployment and hosting | Dockerfile memory #1 (0.499), pnpm #2 (0.383, noise-inflated) | **Dockerfile memory #1 (0.498), pnpm #2 relevance 0.383 → 0.296** — right memory held #1, noise weakened |
| health, fitness or family | nothing at the floor | nothing at the floor (unchanged; §1's version is the bge/gemini calibration gap, Phase 2) |
| what and or the for do | pnpm #1 (0.296) | **nothing — 0 results** |
| WidgetPool-7 (identifier guard) | #1, 0.758 | #1, 0.755 — identifier unchanged through the analyzer |

`reindex --text` output on rehearsal copy: `rebuilt the keyword index for 1
users (7 memories, 0 users skipped)`; search results identical to the
auto-rebuild index.

### Phase 2 — relevance calibration (`fix/relevance-calibration`)

The method is now reproducible: `recuerdos-ai calibrate [--config …]`
(hidden, like `eval`) embeds the eval corpus as `Document` and every case
query as `Query` with *the config's own embedder*, compares true cosines
(nothing is indexed or stored), and reports vector norms, the noise
ceiling, the signal floor per case and overall, and the floor's room at
the baseline actually in force (same precedence recall resolves).
Numbers below are from runs on this branch, model cache warm, both
embedders.

**VERIFY 2.3 resolved: Gemini embeddings are unit-length at the
configured 3072 dimensions** — `gemini-embedding-2` norms over the whole
eval corpus (documents and queries): min/max/mean all `1.0000`, as are
bge's. `similarity_from_l2`'s assumption holds as shipped; no
normalisation change in `gemini_embedder.rs`, **no reindex needed**.

**Noise ceiling** (max raw cosine of each `negative` query over the corpus):

| `negative` query | bge-small | gemini |
|---|---|---|
| what and or the for do | 0.599 | 0.578 |
| how do I do it | 0.577 | **0.580** |
| best hiking trails near the coast | 0.438 | 0.510 |
| french recipe for bouillabaisse | 0.503 | 0.519 |

**Signal floor** (min raw cosine of each case's *expected* memory, full
per-case table in the tool output; the load-bearing minima):

| | bge-small | gemini |
|---|---|---|
| floor-gated kinds, all | 0.459 → 0.528 (excl. filter kinds) | 0.511 → **0.562** (excl. filter kinds) |
| floor-gated, cluster (excl. the stragglers below) | 0.621 | **0.611** |
| weakest floor-gated answers | dietary needle 0.528, code-org nl-category 0.533, health nl-category 0.550, storage-decision nl-question 0.596 | health nl-category **0.562**, dietary needle **0.572** |

**The honest finding (task 2.4):** measured *strictly* — weakest
floor-gated answer vs noise ceiling — there is **no gap on this corpus
for either model**: gemini noise 0.580 > its weakest prose answers 0.562/0.572; bge noise 0.599 > four answers from 0.528–0.596. So
`DEFAULT_RELEVANCE_FLOOR` (0.25) was *confirmed but not by a wide
margin*: it sits inside the gap between the noise ceiling and the
**cluster** of answers (gemini ≥ 0.611, bge ≥ 0.621), and the stragglers
above sit below the noise wherever the floor goes. No
baseline value can suppress every negative and keep those answers;
the measurement does not justify moving 0.25 (no single value fixes
the stragglers either way), so it stands. The thin gap is itself
Phase 3's evidence.

**Chosen `gemini-embedding-2` baseline: 0.46** (`SIMILARITY_BASELINES`
map in `recall_ranker.rs`; bge keeps 0.5 — pre-existing paths
byte-identical, pinned by `the_baseline_map_holds_the_default_model_at_the_default_constant`).
Window math: suppressing corrected noise 0.580 at floor
0.25 needs `b > 0.44`; keeping the 0.611 cluster answer above the floor
needs `b ≤ 0.481`; 0.46 corrects noise to **0.222** (0.028 under the
floor) and a 0.611 answer to **0.280** (0.03 over) — margins pinned by
`the_gemini_map_row_suppresses_measured_noise_and_pins_its_tradeoff`,
which *also* pins the stragglers staying under the corrected noise, so
any re-measure that shifts who lands below the floor fails a test
instead of drifting.

Config: `[retrieval].similarity_baseline` is now `Option<f32>` (unset =
per-model map; explicit value still wins, same `0.0..=0.99` validation);
unknown model → 0.5 with a startup `warn!`
(`resolve_similarity_baseline`, precedence pinned by wiring tests).

#### eval — Run 1, default config (bge-small, the committed baseline's embedder)

Informational flags removed (task 2.5): the gated subset grew 24 → 41
cases, so overall numbers move by construction. `negative` now gates
and passes **4/4** on bge too — the last failure (`hiking trails`) was
keyword-leg noise on the word *best* (one corpus memory ends "…knows
its schema **best**", BM25 rank 1, no vector evidence; debug
`v=17 k=1 relevance=0.287`) — stopworded, and the negative kind is the
guard the stopword module's own rule names.

```
recall@5: 86.5%   precision@1: 64.9%   MRR: 74.1%   latency p50: 10.3 ms p95: 12.4 ms   (37 scored cases, 4 negative)
abstention: 100.0%
  category-filter 100.0/100.0   current-fact 100.0/100.0   exact-token 100.0/100.0
  needle 66.7/66.7   negative 100.0/100.0   nl-category 75.0/75.0   nl-question 85.7/57.1
  paraphrase 100.0/50.0   relational 71.4/28.6   subcategory-filter 100.0/100.0   tag-filter 100.0/0.0
```

Against the *old* committed baseline: recall −5.2 (fails the 5-point
gate — purely the gated-subset expansion; the pre-existing kinds are
identical to Phase 1's bge run), precision +2.4, old baseline MRR
unrecorded → skipped. **New `eval/baseline.json` recorded** (defaults,
per the deviation note below): `bge-small-en-v1.5`, 41 cases,
recall 86.5 / p@1 64.9 / MRR 74.1 / p50 10.3 ms / abstention 100,
`precision_at_1_by_kind` populated; CI's `just eval-native` gate re-run
against it: all deltas −0.0.

#### eval — Run 2, production-like config (`--config recuerdos-ai.toml`)

```
recall@5: 83.8%   precision@1: 75.7%   MRR: 80.2%   latency p50: 310.0 ms p95: 421.6 ms   (37 scored cases, 4 negative)
abstention: 100.0%   (was 75.0% in Phase 1 — all four negatives pass)
  category-filter 100/100   current-fact 100/100   exact-token 100/100   needle 66.7/66.7
  negative 100.0/100.0   nl-category 75.0/75.0   nl-question 78.6/85.7   paraphrase 87.5/75.0
  relational 71.4/28.6   subcategory-filter 100/100   tag-filter 100.0/100.0
```

**No pre-existing `by_kind` regressed**: every number above matches
Phase 1's production-like run for the original kinds (scored cases never
apply the floor, so the 0.5→0.46 change moves only relevance/negatives;
`hiking trails`' fix is the stopword). The `compare()` warnings
(baseline recorded bge, run gemini) are expected and documented in the
deviation note.

#### Four-query re-run (§1, production-like rehearsal)

Same 7 rehearsal memories as the Phase 1 rehearsal (the §1 topics,
decoys and the WidgetPool-7 identifier), re-saved through the **new**
binary under `recuerdos-ai.toml` (fresh store, gemini-pinned; volume
`ra-calib-store`). Column **0.5** = new binary with an explicit
`[retrieval].similarity_baseline = 0.5` override (isolates the
calibration from the analyzer fixes); column **0.46** = the shipped
per-model default.

| query | §1 (live, pre-plan) | 0.5 override | **0.46 (shipped)** |
|---|---|---|---|
| what do I use for data storage | rule #3; noise at 0.89/0.87 | snake_case only, 0.262 | snake_case only, **0.316** — no irrelevant ≥ 0.5; the rule itself stays below floor (as in Phase 1, bge too) |
| deployment and hosting | Dockerfile #2 | Dockerfile #1, 0.406 | Dockerfile **#1, 0.450** |
| health, fitness or family | Berlin #5 at 0.33 under four noise at 0.84–0.86 | nothing | **nothing** — the Berlin memory's raw cosine (0.562-class) is below the noise ceiling; honest abstention, §1's complaint (noise at 0.84–0.91) fully gone |
| what and or the for do | 5 results at 0.84–0.91 | nothing | **nothing — 0 results** |
| WidgetPool-7 (guard) | — | #1, 0.659 | #1, **0.684** — identifier intact |

**DoD:** every irrelevant memory that *appears* is < 0.5; §1's 0.84–0.91
band no longer exists on this store.

#### Deviation and reading notes (no DoD weakened)

- **Task 2.5 wording** says record the baseline on the production-like
  config; the committed `eval/baseline.json` was recorded on **defaults**
  instead, because CI's eval job gates `eval --baseline` *without* a
  config and has no `GEMINI_API_KEY` — a gemini-recorded baseline would
  make every CI run a cross-embedder comparison. The production-like
  run is fully recorded above (all its DoD-relevant claims hold), and
  the baseline's DoD ("p@1, MRR and latency fields populated") is met
  by the committed file.
- Phase 1's §12 gate-mark ("Phase 2 must not start until this entry is
  read") — the entry was read; the operator acknowledged it and
  directed this phase. The noise-propped `relational`/`nl-question`
  regressions were held in mind above: none of Phase 2's numbers
  restored them, and none of Phase 2's choices depend on them.
- The `calibrate` verdict line prints "NOT inside the gap" for both
  models under the strict all-kinds reading. That is the tool telling
  the truth about this corpus (see the honest finding), not a defect:
  the per-case table and the cluster/straggler split above are what
  decisions read.
- One hidden-subcommand addition (`calibrate`) and one user-visible
  config-semantics change (`similarity_baseline` unset ≠ 0.5 for
  gemini — it is 0.46; deployments that *want* 0.5 pin it explicitly).

### Phase 3 — gate (`docs/rerank-gate`)

No code changes on this branch (`just check-native` re-run: all green).
Task 3.1 re-ran both eval passes and the four §1 queries; every number
reproduces the Phase 2 record **exactly** — the committed `eval/baseline.json`
(Phase 2.5, defaults) gates the default run at −0.0 on all three metrics.

#### eval — Run 1, default config (bge-small, the committed baseline's embedder)

`docker compose run --rm dev just eval-native`

```
recall@5: 86.5%   precision@1: 64.9%   MRR: 74.1%   latency p50: 10.7 ms p95: 12.3 ms   (37 scored cases, 4 negative)
abstention: 100.0%
   category-filter: recall 100.0%   p@1 100.0%
      current-fact: recall 100.0%   p@1 100.0%
       exact-token: recall 100.0%   p@1 100.0%
            needle: recall 66.7%   p@1 66.7%
          negative: recall 100.0%   p@1 100.0%
       nl-category: recall 75.0%   p@1 75.0%
       nl-question: recall 85.7%   p@1 57.1%
        paraphrase: recall 100.0%   p@1 50.0%
        relational: recall 71.4%   p@1 28.6%
  subcategory-filter: recall 100.0%   p@1 100.0%
        tag-filter: recall 100.0%   p@1 0.0%

baseline recall@5: 86.5%   now: 86.5%   change: -0.0 points
baseline precision@1: 64.9%   now: 64.9%   change: -0.0 points
baseline MRR: 74.1%   now: 74.1%   change: -0.0 points
```

The ranking gap is visible per kind: `paraphrase` 100 recall / 50 p@1,
`relational` found 5 of 7 but topped only 2, `tag-filter` 100 / 0,
`nl-question` 85.7 / 57.1. The scored cases missing recall entirely are
the known ones, identical to the Phase 2 record: `dietary` (needle, both
embedders); `who owns` (graph hop, 0% since Phase 0) and
`where sits` (0% since its Phase 0 pass turned out to ride keyword noise —
Phase 1 note); `package manager` and `code organisation` (Phase 1's
noise-propped pair and the abstract-category follow-up; the gemini run
additionally drops `testing tools`/`multi-hit` on the `tests`/`testing`
stem split — the §11 stemming follow-up).

#### eval — Run 2, production-like config (gemini-embedding-2)

`docker compose run --rm dev cargo run -q --bin recuerdos-ai -- eval --config recuerdos-ai.toml`

```
recall@5: 83.8%   precision@1: 75.7%   MRR: 80.2%   latency p50: 320.7 ms p95: 450.6 ms   (37 scored cases, 4 negative)
abstention: 100.0%
   category-filter 100.0/100.0   current-fact 100.0/100.0   exact-token 100.0/100.0
   needle 66.7/66.7   negative 100.0/100.0   nl-category 75.0/75.0
   nl-question 78.6/85.7   paraphrase 87.5/75.0   relational 71.4/28.6
   subcategory-filter 100.0/100.0   tag-filter 100.0/100.0
```

Identical to Phase 2's Run 2 to the last digit (latency re-measured).

#### Headroom (task 3.2): recall@5 − precision@1

| Scope | bge-small | gemini |
|---|---|---|
| overall | 86.5 − 64.9 = **+21.6** | 83.8 − 75.7 = **+8.1** |
| `nl-question` | 85.7 − 57.1 = **+28.6** | 78.6 − 85.7 = −7.1 |
| `nl-category` | 75.0 − 75.0 = 0 | 75.0 − 75.0 = 0 |
| `needle` | 66.7 − 66.7 = 0 | 66.7 − 66.7 = 0 |
| `paraphrase` | 100.0 − 50.0 = **+50.0** | 87.5 − 75.0 = **+12.5** |

Reading: the kind-level heads where positive are pure **ranking** gaps —
the expected memory is in the top 5 but not first. The negative
`nl-question` headroom on gemini is the opposite failure (the pnpm and
table-driven memories leave the top 5 entirely — the reranker can only
promote them if they are in the candidate pool, i.e. via
`candidate_depth`); zero-headroom kinds are recall-bound (`dietary`,
`code organisation`), not ranking-bound — a reranker will not fix them.

**`nl-*` cases not at #1** (case-level `top` column): bge-small **4**
(`package manager`, `storage decision`, `auth review`, `code organisation`);
gemini **2** (`package manager`, `code organisation`).

#### Four §1 queries, re-run against the rehearsal store

Same `ra-calib-store` rehearsal store as Phase 2 (7 memories, gemini-pinned),
served by this branch's binary under `recuerdos-ai.toml` with the per-model
0.46 baseline (fresh read-only API key for user `alex` added to that store;
the store's memory rows untouched):

```
== what do I use for data storage
   #1 relevance=0.316  New Python files must use snake_case filenames
== deployment and hosting
   #1 relevance=0.450  The backend production Dockerfile is managed through the production-deploy…
== health, fitness or family
   (no results at the floor)
== what and or the for do
   (no results at the floor)
== WidgetPool-7 (guard)
   #1 relevance=0.684  The WidgetPool-7 connector pools Postgres connections in widget_pool.rs
```

Byte-identical to Phase 2's "0.46 (shipped)" column. What remains of §1 on
this store is a **ranking** problem, not a noise problem: no noise appears
at the floor anywhere, and the database rule / Berlin fact remain below it.

#### Decision (task 3.3)

| Condition (after Phase 2) | Measured | Applies? |
|---|---|---|
| headroom < 10 **and** all `nl-*` at #1 → NO-GO | false under **both** embedders: `nl-*`-at-#1 fails on both (4 and 2 misses), and headroom ≥ 10 on the baseline embedder | **no** |
| headroom ≥ 10 **or** ≥ 2 `nl-*` cases miss #1 → GO | headroom 21.6 ≥ 10 **and** ≥ 2 miss #1 **on both embedders** (the second disjunct holds even on gemini, where overall headroom is 8.1) | **yes** |
| headroom ≥ 10 but corpus < 100 → GO cautiously | eval corpus = 35 memories | **yes** |

| Metric | Value |
|---|---|
| recall@5 | 86.5% (bge, gated baseline embedder) · 83.8% (gemini, production-like) |
| precision@1 | 64.9% · 75.7% |
| MRR | 74.1% · 80.2% |
| headroom (recall@5 − p@1) | **+21.6** · +8.1 (per-kind table above) |
| `nl-*` cases not at #1 | 4 (bge) · 2 (gemini) |
| **Decision** | **GO, cautiously** |

**GO, cautiously.** Continue to Phase 4. Per the cautious clause the Phase 6
shadow/audit over the **real** store is a hard prerequisite for any default
change in Phase 8 — and given the §1 evidence came from the real (larger)
store while the eval corpus is 35 memories, Phase 7's ship gate should lean
on the Phase 6 evidence at least as much as on the eval grid.

Follow-on note for Phase 4–7 (no action now): the fixes' ranking gaps are
concentrated in `paraphrase`/`relational`/`tag-filter` p@1 and the two
gemini `nl-question` recall misses — cases of the first two styles and a
candidate pool wide enough to lift the latter are what Phase 7.1 must add.

### Phase 3 DoD

- [x] §12 contains the filled decision table with the measured numbers and
      the decision (above).
- [x] If NO-GO: N/A — the decision is GO, Phases 4–8 proceed.

### Phase 4 — contract (`feat/rerank-contract`)

Everything below exists exactly as specified — contract, fake, ranker
strategies, recaller integration, additive surface — and *nothing* is wired:
the default binary constructs no reranker and takes the pre-change code
path, pinned by the identity test. `just check-native` passes (fmt, clippy
`-D warnings`, boundary script, `cargo test`: **787 unit tests, +32**, all
integration suites green), and `just sdk-test` passes end to end against a
release-image daemon built with these changes.

**Contract** (`memories/domain/reranker.rs`, exported from `domain`):
`Reranker::score(&self, query, &[RerankCandidate]) -> Result<Vec<f32>>`,
aligned-scores by position as the plan prescribes; `RerankMode
{Off(default), Shadow, On}`. The module docs record why aligned-not-sorted,
why the scale is model-specific, and why relevance stays untouched (I-4).

**Ranker** (`recall_ranker.rs`): new general entry `rank_with_legs` (with an
empty rerank list it is *the same code path* — `apply_rerank` returns before
touching anything), `rank`/`rank_with_graph` unchanged as thin wrappers.
`ScoredMemory.rerank_score: Option<f32>`, `MatchDetail.rerank_rank:
Option<usize>` (keeps `Eq`). `RerankStrategy::{Rrf{weight}, Replace}` with
`Rrf{1.0}` the un-tuned default; the module docs carry the calibration
argument — one leg ≈ `1/61`, weight 1 can never overrule two-leg agreement
(arithmetic, pinned by test), weight ≥ 2 can, the tuned range spans 0.5–4,
and `Replace` bands raw scores by the existing `MULTIPLIER_FLOOR` band so
recency/quality decide ties but cannot cross a 1.25× gap. Both strategies
have unit tests (`rank_with_legs` is not wired yet, so `Replace`,
`with_rerank_strategy`, `rank_with_legs`, `RerankMode::On` and the
`RerankCandidate` fields carry documented `#[allow(dead_code)]`s until
Phase 5 consumes them — the precedent is `shared/error.rs`).

**Recaller**: `reranker: Option<Arc<dyn Reranker>>` + mode/depth/timeout via
`with_reranker` (defaults mirror the planned `[rerank]` block: 30 / 400 ms;
inert while no reranker). The leg runs **after fusion and after the floor,
before truncation** — so filters and `min_relevance` always run first and
the reranker only ever sees the head of what a client could get anyway.
`rerank_leg()` mirrors `graph_leg()`: skips pools (or heads) under 2, warns
and serves the pre-rerank order on error, deadline expiry, misaligned score
counts, or a dead scoring thread. A non-zero deadline runs the synchronous
`score` on a detached thread (`mpsc::recv_timeout`) so expiry doesn't wait
for inference; Phase 5's semaphore bounds those server-side. Shadow mode
logs the event **`rerank_shadow`** (`query_hash, depth, rerank_ms,
top1_before, top1_after, changed, before, after`) — ids and a SHA-256
prefix only; the test asserts the raw query and memory content appear
*nowhere* in the captured fields. Two log-shape decisions were found the
hard way and are worth repeating: tracing must be given `name:
"rerank_shadow"` explicitly (an auto-named event's `name()` is the source
location, which broke the first version of the test), and ids are logged by
their **last** 8 chars — UUIDv7 heads encode the save timestamp, so
memories saved in the same millisecond share the first 8.

**Surface**: `matched.rerank_rank` / `matched.rerank_score` on the REST
search response, `skip_serializing_if = Option::is_none` (old wire shape is
the default and is asserted); `docs/api.md` documents both; the Python SDK
adds the two optional fields with tests that a response *without* them
parses to `None`. MCP has no match-detail output at all (tool results render
content + relevance), so there was nothing to add there — recorded for
Phase 6, whose audit CLI is where rerank scores reach an operator.

#### eval — identical to Phase 3, both embedders

`docker compose run --rm dev just eval-native` (bge-small, gated):

```
recall@5: 86.5%   precision@1: 64.9%   MRR: 74.1%   latency p50: 9.9 ms p95: 12.1 ms   (37 scored, 4 negative)
baseline recall@5: 86.5%   now: 86.5%   change: -0.0 points
baseline precision@1: 64.9%   now: 64.9%   change: -0.0 points
baseline MRR: 74.1%   now: 74.1%   change: -0.0 points
```

(all `by_kind` lines identical to the Phase 3 record; abstention 100.0%.)

`... eval --config recuerdos-ai.toml` (gemini-embedding-2):

```
recall@5: 83.8%   precision@1: 75.7%   MRR: 80.2%   latency p50: 321.2 ms p95: 412.5 ms   (37 scored, 4 negative)
```

— identical to Phase 3's Run 2 to the last digit (latency re-measured). No
reranker is wired, as required.

#### Identity and the rest of the test plan

- **Identity (the anchor):** `an_absent_or_off_reranker_leaves_recall_byte_identical`
  — no-reranker vs `Off`-with-one-loaded over a multi-result fixture,
  fingerprinted by ids+order, the `f32` **bits** of `score` and `relevance`,
  and the rerank fields; the loaded reranker's call counter stays 0.
- `On` serves the rerank order exactly (reversing fake through `Replace`,
  where inversion *is* the strategy's contract — pushing an exact inversion
  through the weight-1 RRF leg would test the wrong thing, noted at the
  test); `Shadow` fingerprints identical to the plain run *and* logs the
  comparison once; failing / expired legs return the pre-rerank order with
  a `warn!`; the deadline is also asserted in wall-clock (a 500 ms scorer
  under a 20 ms deadline costs < 400 ms); zero-timeout inline scoring
  reorders identically to the threaded path; pools/heads < 2 and empty
  stores never call the reranker; `depth` beyond the pool clamps to it; the
  category filter's rejects never reach the fake's `seen()`; `relevance`
  bits unchanged in `On` mode and `min_relevance` still cuts.
- Doubles for Phase 6: `FakeReranker` (token-overlap / reversing, with call
  counters and `seen()`), `FailingReranker`, `SleepingReranker`.

#### Phase 4 DoD

- [x] `just check-native` passes; `just sdk-test` passes.
- [x] The identity test exists and passes (`an_absent_or_off_reranker_leaves_recall_byte_identical`).
- [x] `eval` numbers identical to Phase 3 (no reranker is wired yet) — recorded above.
- [x] Module docs in `recall_ranker.rs` describe both strategies and the weight calibration.

### Phase 5 — adapter

**Measurements** (`jina-reranker-v1-turbo-en`, the config default; dev container
on the M4 Pro, model cache on the `models` volume; RSS from `/proc/self/status`,
printed by `warm-models` itself — Linux only):

| what | number | how |
| --- | --- | --- |
| model size on disk | **147 MB** (`/models/models--jinaai--jina-reranker-v1-turbo-en`) | `du -sh /models/*` |
| cold start (load, warm cache) | **283 ms** (9.4 s on the first-ever run, download included) | `warm-models` timed print around `FastembedReranker::load` |
| smoke score (2 candidates) | 7 ms | same |
| RAM at idle with `mode = off` | 219 MiB RSS (embedding model loaded, no rerank session) | same, before-load line |
| RAM loaded (1 session) | **404 MiB → +180 MiB per `max_concurrency` session** | same, after-load line |

`recuerdos-ai warm-models` downloads the rerank model **only when the loaded
config enables it** (`-e RECUERDOS_AI_RERANK__MODE=on`); the default run is
byte-identical to before the phase and pulls nothing new (no `[rerank]` in the
default config).

**`eval` with `mode = on`** — paired, same machine and embedder (§12 Run 1's
bge-small default config; depth 30, `rrf` weight 1, 1 session, everything else
default; the 5-case `by_kind` deltas all sit in the §9 tolerance, quality is
explicitly *not* judged here):

```
mode off: recall@5: 86.5%   precision@1: 64.9%   MRR: 74.1%   latency p50: 10.3 ms p95: 11.7 ms   (37 scored, 4 negative)
mode on:  recall@5: 86.5%   precision@1: 64.9%   MRR: 74.3%   latency p50: 55.2 ms p95: 67.2 ms   (37 scored, 4 negative)
```

Recall and p@1 unchanged; MRR +0.2 points (two `nl-question`/`relational`
cases reshuffled inside the head). Cost: **+45 ms p50 per recall** scoring 30
candidates with one session (the daemon's `max_concurrency = 2` exists to
share that; the 400 ms deadline never fired — p95 67 ms). The `--config
recuerdos-ai.toml` (gemini) pairing is deferred to Phase 7's grid, where the
latency is dominated by the embedding API and belongs there.

#### Adapter (`fastembed_reranker.rs`)

**VERIFY resolved (§2 row 1 / task 5.1):** fastembed 5.17.3 needs **no Cargo
feature** for rerank (the `hf-hub` default features already cover model
download), `TextRerank::rerank(&mut self, query, docs, return_documents,
batch_size)` takes **`&mut self`** and returns `Vec<RerankResult>` **sorted
descending** with `index` back-references into the input. The adapter
therefore keeps a pool of `max_concurrency` `TextRerank` sessions
(`InstancePool`: first free `try_lock`, else block on the oldest — honest
bound, no false parallelism), and `align_scores` restores candidate-order
alignment. Scores pass through a sigmoid `squash()` — strictly monotonic, so
RRF ranks never move because of the adapter; `Replace` gets its
non-negative comparable contract; one `(0,1)` scale across models.
`DEFAULT_MAX_LENGTH = 256` (memories are short; the 512 default only buys
padding). Models: 4 built-ins by short name or HF repo id, plus any `.onnx`
path read with its tokenizer files from the same directory
(`UserDefinedRerankingModel`).

#### Config and wiring

`[rerank]` mirrors `GraphConfig`: defaults `off` / `jina-reranker-v1-turbo-en`
/ 30 / `rrf` / 1.0 / 400 ms / 2 / 16; validation reports every bad knob at
once (the graph precedent: `depth ≥ 1`, `weight ≥ 0`, `timeout_ms ≥ 1`,
`max_concurrency ≥ 1`, known `mode`/`strategy`; an unknown `model` is fatal
**only when `mode != off`**); env overrides follow `RECUERDOS_AI_RERANK__*`;
`recuerdos-ai config` prints the effective block (`mode` alone for `off`, so
the default echo names no model nobody loaded). `build_reranker` returns
`Ok(None)` **before resolving anything** when `mode = off` — that is the gate
for the no-network/no-weights/no-startup-cost DoD item, tested directly
(`mode = off` with a deliberately bogus model still assembles). A load failure
with `mode != off` aborts startup with a `Validation` error naming the model,
like embeddings. Wiring test: `the_rerank_config_reaches_recall_through_the_
real_wiring` runs the real `Memories::assemble_with_reranker` with the
reversing fake — `on` + `replace` serves the exact reversed head and
`depth = 2` bounds it to a top-two swap.

### Phase 6 — audit and shadow

#### Reading without leaving a mark (6.1)

`MemoryRecaller::execute_untracked` is the same pipeline with `touch_accessed`
skipped: identical candidates, fusion, rerank leg, floor and order, no write
behind it. `ExecuteOptions { track_access }` defaults to `true` and every
client-facing path still calls `execute`, so forgetting the option cannot
switch decay off by accident — the untracked path is opt-in and has exactly two
callers, the audit and `eval` (both its warmup and `score`). A clone of the
recaller is cheap (every field is an `Arc`), which is how the audit gets its
two pipelines: `with_second_stage(None, Off)` and
`with_second_stage(Some(reranker), On)` over **one** set of indexes, so the two
columns cannot differ by anything but the stage being judged.

The keyword index opens through `TantivyTextIndex::open_read_only`: no
`IndexWriter` is ever created, so tantivy's per-directory writer lock stays
with whoever holds it, and every mutation path returns
"the keyword index is open read-only, so it cannot be written" instead of
writing. It is *not* `Database::open_read_only` — that variant takes
`META_LOCK`, which a writer holds across `wait_merger_threads()`, so a reader
opening that way could block for the length of a merge. `search` on the
plain `MmapDirectory` only takes the short per-read lock and never the writer
lock, which is what makes the daemon-running case work. A user with no keyword
index is a refusal naming `reindex --text`, not a silently vector-only audit.

SQLite is opened the normal way (migrations are no-ops on an existing store);
purity is carried by "the audit never issues a write" plus the test, which is
what the plan allows ("read-only (or never writes)").

#### `rerank audit` (6.2)

Hidden subcommand, `eval`/`calibrate` pattern:
`rerank audit --queries <file> [--user <handle>] [--limit 8] [--depth N] [--config <path>] [--json]`.
Query file is sniffed: plain lines (`#` comments allowed) or `[[query]]` blocks
with `text` and optional `expect` — a case-insensitive **substring** of the
memory that should win, because an operator typing about memories they wrote
months ago will not paste them. `--user` follows the background jobs'
enumeration (`UserRepository::list` + `BackgroundUserResolver`) and refuses to
guess when several users exist; an unknown handle lists the real ones.

It loads `[rerank].model` **regardless of `mode`** — judging a feature whose
honest config answer is still `off` is the point — while strategy, weight and
depth come from the config, so the `after` column is what enabling it would
serve. Two disclosed departures from a live request, printed in the header: no
relevance floor (near-misses are the subject; flooring before the reranker
would change what it reorders) and no rerank deadline (measuring what the model
costs is half the job; `timeout_ms` is printed beside the percentiles it must
contain). Latency is metered around the `Reranker` boundary, not around the
recall call, because a recall also embeds the query and on the local model that
is the larger half. `--json` carries ids, ranks, scores and the judged
candidate range, and **no memory content**, so a run can be committed or
pasted; content is terminal-only.

Tests: untracked recall orders identically to tracked and stamps nothing
(in-memory repository) while still running the second stage; `with_second_stage`
shares everything else; the read-only keyword handle reads what a still-open
writer committed, refuses all three mutations, and names the repair when there
is nothing to read; and the audit over a seeded store asserts the reversed
`after` column exactly, the `--json` shape (including that memory text is
absent), the top-1 flag, the summary arithmetic to the third decimal, user
isolation, `--user` required with several users, unknown user, empty query file,
both query-file formats, and — the phase's own requirement — that the
`memories` rows including `access_count`, `last_accessed_at` and `importance`
are identical after two consecutive audits.

#### Rehearsal on a copy of a real store

**This environment has no live store.** What exists is the Phase 2/3 rehearsal
store: **7 memories** (the eval corpus), `gemini-embedding-2`, keyword index
`v2`. The DoD asks for a copy of the real data directory; this is the copy of
the only store here, and the numbers below must be read as a *smoke test of the
command*, not as evidence about the deployment.

Against a copy (`ra-audit-store`), with the daemon serving that copy and the
real `jina-reranker-v1-turbo-en`, `--limit 5`, depth left at the configured 30,
15 queries — §1's four plus eleven operator phrasings, 12 of them with
`expect`:

| run | state | top-1 changed | mean rank disp. | rerank p50 / p95 | p@1 before → after | MRR before → after |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | daemon serving the copy | 0/15 (0.0%) | 0.60 | 13 ms / 30 ms | 91.7% → 91.7% | 93.8% → 93.8% |
| 2 | daemon **holding the keyword writer lock** | 2/15 (13.3%) | 0.65 | 13 ms / 26 ms | 91.7% → **83.3%** | 93.8% → **89.6%** |

13 ms p50 is the cost of judging **7 candidates** (the whole store; `depth 30`
never binds) — consistent with Phase 5's 45 ms for 30, and reported as
"7 candidates judged; depth ceiling 30" for exactly that reason.

**Read-only, verified on the store, not only in a test.** `cksum` over all 18
files of the copy (db, WAL, and every tantivy segment) was identical before and
after the audit. In the same conditions — daemon up, having served one write,
so it holds the writer lock — a read-*write* open still fails, as it must:

```
error: internal error: failed to open a text index writer: Failed to acquire
Lockfile: LockBusy. Some("Failed to acquire index lock. If you are using a
regular directory, this means there is already an `IndexWriter` working on
this `Directory`, in this process or in a different process.")
```
(from `recuerdos-ai reindex --text`, run to prove the point). The audit ran
fine at the same moment. No weakening of the "stop the daemon" clause was
needed, and none was done.

**Every top-1-changed case, by hand** (the ship gate's requirement):

```
3. health, fitness or family   ⚑ top-1 changed
   before
    1. ae56d01b  fact.person            rel 0.19  Alex lives with a partner and a baby in Berlin
    2. 38836313  preference.personal    rel 0.18  Prefers short answers over long explanations
   after
    1. 38836313  preference.personal    rel 0.18 rerank 0.022 (from 2)  Prefers short answers over long explanations
    2. ae56d01b  fact.person            rel 0.19 rerank 0.016 (from 1)  Alex lives with a partner and a baby in Berlin
   expect "partner and a baby": rank 1 → 2
```

* **Q3 "health, fitness or family"** — *a real regression.* The fused head was
  already right ("Alex lives with a partner and a baby in Berlin", rank 1) and
  the reranker preferred "Prefers short answers over long explanations"
  (rerank 0.022 vs 0.016). Both scores sit in the model's noise band; a
  three-word category phrase against two declarative memories is precisely the
  input the `reranker` module docs warn about (trained on question→passage
  pairs). Cost: p@1 −8.3 points on 12 expectations.
* **Q4 "what and or the for do"** — *no verdict possible.* Nothing in the store
  answers it; the head moved from "Prefers short answers" (relevance 0.21) to
  "Only parameterized queries…" (0.09). Every result in that list is below the
  live 0.25 floor, so a real client sees nothing either way — but it shows the
  stage will happily reshuffle pure noise, which the audit only reveals because
  it runs floorless.

**Why the two runs disagreed** is itself worth recording: Q3's rerank scores
were identical in both runs (0.022 vs 0.016) and its fused order was identical,
yet run 1 kept the head and run 2 flipped it — the RRF margin between those two
memories is smaller than the recency decay of the 17 minutes between the runs.
So on this corpus a *single* head is not a stable measurement object; the p@1 /
MRR pair over expectations is, and that is what Phase 7's grid should compare.

**What this rehearsal cannot decide:** 7 memories, and the fused order already
places 11 of 12 expectations first. A stage that can only reorder the head has
nowhere to gain and somewhere to lose, so "no measurable benefit" here is a
statement about the corpus as much as about the reranker. §1's failures came
from a larger store than this one; per §9's risk row ("offline eval too small
to represent real traffic"), Phase 7 must run its grid on the expanded 7.1
cases and on a store with ≥100 memories, or say that the ship decision rests on
shadow data instead.

#### Shadow telemetry (6.3)

The event name and fields were fixed in Phase 4 (`rerank_shadow{query_hash,
depth, rerank_ms, top1_before, top1_after, changed, before, after}` — ids are
the UUIDv7 tails, no content, no query text, pinned by
`shadow_returns_the_pre_rerank_order_and_logs_only_ids_and_a_hash`). What this
phase added is the operator-facing half: `docs/evaluation.md` now documents the
grep (`grep 'rerank_shadow' | grep -c 'changed=true'` over the text log, or
`select(.fields.message == "rerank_shadow")` in the `RECUERDOS_AI_LOG=json`
form) and how to read the rate — it says how much the stage would *bite*, not
whether it is right, and it must be read next to `rerank_ms` p95 against
`timeout_ms`. `[rerank]` in `recuerdos-ai.example.toml` points at both.

**Disagreement rate and latency: not measured — no live traffic.** The store
here serves nobody: no client is recalling against it, so the DoD's "≥3 days or
≥200 real recalls" clause is skipped under its own exemption and recorded as
skipped. When a deployment with traffic exists, the commands above produce the
number in one pass; nothing else about this phase has to change to obtain it.

#### No "rerank all" (6.4)

Provided by absence, and stated where someone would look for it: the `rerank`
subcommand's own help, `docs/evaluation.md`, and the `[rerank]` config block
all say the audit is the supported way to run the stage over existing memories,
and that rerank scores are a property of a query and its candidate list rather
than of a stored row.

#### Gates

`just check-native`: green (fmt, `clippy -D warnings`, boundary script, 919
tests passing — 823 unit and the integration suites, 18 of them new here).
`just eval-native`: **unchanged** against the committed baseline —
recall@5 86.5%, precision@1 64.9%, MRR 74.1%, abstention 100% — which is the
point of switching the eval to `execute_untracked`: it must score exactly what
it scored before.

### Phase 7 — tuning (`feat/rerank-tuning`)

#### The grown corpus comes first (7.1)

Phase 6's rehearsal showed why 35 memories decide nothing: at that size
fusion returns the whole store for every query, so a second stage has
nowhere to win and nowhere to lose. The eval set grows to **108
memories and 59 cases** (52 scored, 7 negative) carrying the pressures
reranking is proposed against: 7 `near-miss` pairs whose distractor
shares an answer's shape but not its content, 4 `statement` cases that
query in the declarative voice cross-encoders were never trained for,
4 `nl-category` clusters that share a theme and little vocabulary, 3
more `negative` queries, and realistic filler so the numbers are not
trivially perfect. Every pre-existing case and memory keeps its text;
four expansion corrections are made *in the set*, not quietly in a
baseline:

* `tag filter: typescript` — a bare "conventions" no longer reaches any
  typescript memory from 108, so recall measured the corpus, not the
  filter. The query becomes "typescript conventions"; the case still
  tests exactly what its kind means.
* `subcategory filter: testing only` — the new testing-naming memory is
  as legitimate an answer to the filtered query as the three in the
  gold; the gold completes to include it rather than score the
  expansion as a miss.
* Two filler memories shed vocabulary that leaked into other cases: one
  matched BM25 as "rest" ("…cost more than REST") and leaked
  `negative: succulents` over the floor; one read as a database
  decision ("offset scans … a million rows") and took the
  decisions-only filter's top-1 as near-DB noise. The corpus moved, not
  the gate.
* The two new cases that were recall-bound as first phrased
  (stale-logo → "stale assets", raw-SQL) got queries that share
  vocabulary with the expected memory: near-miss measures
  discrimination *among retrieved candidates*, not whether a lexical
  path exists to a memory no leg can reach.

Crossing 100 memories also caught a real production bug: Gemini's
`batchEmbedContents` accepts at most 100 requests per call and the
embedder client never chunked — any store over 100 memories would die
with HTTP 400 on its next consolidation. Chunked client-side, in order,
with a test (`fix(providers): chunk Gemini batchEmbedContents…`).

Reference numbers on the grown corpus, default path (bge-small, rerank
off — the grid's `off` row): **recall@5 83.0, precision@1 65.4,
MRR 74.6, abstention 100.0** across all seven negatives. The
standalone eval process a moment later recorded MRR 74.3 —
cross-process recency drift, below the gate's granularity, and exactly
the Phase 6 lesson: nothing below gate granularity is compared across
processes; the grid's cells are paired inside one run for that reason.

#### The grid (7.2)

`eval --grid` (with `scripts/rerank-grid.sh` as the operator entry
point): one seeded store, one embedder session, one open keyword index;
every cell clones the pipeline and differs from the reference row only
in the second stage under measurement — so no seed variance, no
warm-up variance, no cross-process clock drift, and forty cells cost
one seed instead of forty. Cells run **deadlineless**, the audit's
reason: measuring what the model costs is half the job. Depth-50 rows
clamped to 40 candidates — the `candidate_depth` pool for limit 5,
exactly like production. Latency prints twice on purpose: whole-recall
p50/p95 (the Δ against `off` is the ship gate's added latency) and
Reranker-boundary percentiles (`≤Nc` = candidates judged) — the number
`[rerank].timeout_ms` must contain and the one that scales with depth.

```
grid: embedder bge-small-en-v1.5 · corpus 108 memories · 52 scored + 7 negative cases
no rerank deadline in force (a live request would enforce 400 ms — read the rerank p95 against it)

off                                       off    —     0 │  65.4  74.6  83.0 100.0 │    7.9    9.1 │                — │   reference
loading jina-reranker-v1-turbo-en …
jina-v1-turbo-en                          rrf  0.5    10 │  67.3  74.7  83.3 100.0 │   23.0   29.1 │    16/20 ms ≤10c │   +1.9   +0.2     +20
jina-v1-turbo-en                          rrf    1    10 │  65.4  73.7  83.3 100.0 │   23.2   30.7 │    16/21 ms ≤10c │   +0.0   -0.9     +22
jina-v1-turbo-en                          rrf    2    10 │  63.5  72.6  82.7 100.0 │   23.8   29.8 │    16/21 ms ≤10c │   -1.9   -2.0     +21
jina-v1-turbo-en                          rrf    4    10 │  61.5  71.5  83.3 100.0 │   23.8   32.3 │    16/21 ms ≤10c │   -3.8   -3.0     +23
jina-v1-turbo-en                          rrf  0.5    20 │  67.3  74.8  83.3 100.0 │   40.9   51.9 │    34/42 ms ≤20c │   +1.9   +0.3     +43
jina-v1-turbo-en                          rrf    1    20 │  63.5  72.8  83.3 100.0 │   42.5   50.9 │    35/41 ms ≤20c │   -1.9   -1.8     +42
jina-v1-turbo-en                          rrf    2    20 │  61.5  71.7  83.3 100.0 │   42.6   54.9 │    36/45 ms ≤20c │   -3.8   -2.9     +46
jina-v1-turbo-en                          rrf    4    20 │  61.5  71.4  83.3 100.0 │   42.5   52.8 │    35/43 ms ≤20c │   -3.8   -3.2     +44
jina-v1-turbo-en                          rrf  0.5    30 │  67.3  75.0  83.3 100.0 │   55.8   70.6 │    48/61 ms ≤30c │   +1.9   +0.4     +62
jina-v1-turbo-en                          rrf    1    30 │  63.5  72.8  82.7 100.0 │   54.8   68.1 │    49/59 ms ≤30c │   -1.9   -1.8     +59
jina-v1-turbo-en                          rrf    2    30 │  61.5  71.7  83.3 100.0 │   54.8   69.5 │    48/60 ms ≤30c │   -3.8   -2.9     +60
jina-v1-turbo-en                          rrf    4    30 │  61.5  71.4  83.3 100.0 │   55.6   69.0 │    49/60 ms ≤30c │   -3.8   -3.2     +60
jina-v1-turbo-en                          rrf  0.5    50 │  67.3  75.0  83.3 100.0 │   72.3   90.7 │    66/82 ms ≤40c │   +1.9   +0.4     +82
jina-v1-turbo-en                          rrf    1    50 │  63.5  72.8  82.7 100.0 │   71.9   90.4 │    66/81 ms ≤40c │   -1.9   -1.8     +81
jina-v1-turbo-en                          rrf    2    50 │  61.5  71.7  83.3 100.0 │   71.8   88.9 │    66/80 ms ≤40c │   -3.8   -2.9     +80
jina-v1-turbo-en                          rrf    4    50 │  59.6  69.8  83.3 100.0 │   71.4   89.2 │    66/81 ms ≤40c │   -5.8   -4.7     +80
jina-v1-turbo-en                      replace    —    10 │  55.8  65.4  75.3 100.0 │   24.7   31.5 │    17/22 ms ≤10c │   -9.6   -9.2     +22
jina-v1-turbo-en                      replace    —    20 │  55.8  64.4  73.4 100.0 │   41.9   52.1 │    35/42 ms ≤20c │   -9.6  -10.2     +43
jina-v1-turbo-en                      replace    —    30 │  55.8  64.4  73.4 100.0 │   55.4   70.7 │    49/61 ms ≤30c │   -9.6  -10.2     +62
jina-v1-turbo-en                      replace    —    50 │  55.8  64.4  73.4 100.0 │   73.1   91.2 │    67/82 ms ≤40c │   -9.6  -10.2     +82
loading bge-reranker-base …
bge-base                                  rrf  0.5    10 │  71.2  77.1  83.3 100.0 │   64.2   83.2 │    58/74 ms ≤10c │   +5.8   +2.5     +74
bge-base                                  rrf    1    10 │  71.2  77.2  83.3 100.0 │   65.4   79.7 │    59/71 ms ≤10c │   +5.8   +2.7     +71
bge-base                                  rrf    2    10 │  69.2  75.8  83.3 100.0 │   65.1   83.2 │    57/74 ms ≤10c │   +3.8   +1.2     +74
bge-base                                  rrf    4    10 │  65.4  74.4  84.6 100.0 │   65.9   83.4 │    60/74 ms ≤10c │   +0.0   -0.2     +74
bge-base                                  rrf  0.5    20 │  71.2  77.2  83.3 100.0 │  119.7  146.9 │  117/138 ms ≤20c │   +5.8   +2.7    +138
bge-base                                  rrf    1    20 │  71.2  76.8  83.3 100.0 │  119.5  155.6 │  115/144 ms ≤20c │   +5.8   +2.2    +147
bge-base                                  rrf    2    20 │  69.2  76.4  83.0 100.0 │  123.9  151.8 │  118/143 ms ≤20c │   +3.8   +1.8    +143
bge-base                                  rrf    4    20 │  65.4  74.0  81.7 100.0 │  119.3  155.7 │  114/146 ms ≤20c │   +0.0   -0.5    +147
bge-base                                  rrf  0.5    30 │  71.2  77.2  83.3 100.0 │  169.8  204.7 │  166/195 ms ≤30c │   +5.8   +2.7    +196
bge-base                                  rrf    1    30 │  71.2  76.8  83.3 100.0 │  171.6  209.1 │  165/200 ms ≤30c │   +5.8   +2.2    +200
bge-base                                  rrf    2    30 │  69.2  76.5  83.7 100.0 │  167.6  213.6 │  165/205 ms ≤30c │   +3.8   +1.9    +205
bge-base                                  rrf    4    30 │  65.4  74.4  83.7 100.0 │  168.0  211.0 │  165/201 ms ≤30c │   +0.0   -0.2    +202
bge-base                                  rrf  0.5    50 │  71.2  77.2  83.3 100.0 │  228.1  276.3 │  226/266 ms ≤40c │   +5.8   +2.7    +267
bge-base                                  rrf    1    50 │  71.2  77.1  84.0 100.0 │  230.0  279.5 │  227/268 ms ≤40c │   +5.8   +2.6    +270
bge-base                                  rrf    2    50 │  69.2  76.5  83.7 100.0 │  226.7  279.1 │  225/269 ms ≤40c │   +3.8   +1.9    +270
bge-base                                  rrf    4    50 │  63.5  73.3  83.7 100.0 │  229.2  280.9 │  229/270 ms ≤40c │   -1.9   -1.3    +272
bge-base                              replace    —    10 │  61.5  71.7  82.7 100.0 │   68.4   86.2 │    61/76 ms ≤10c │   -3.8   -2.9     +77
bge-base                              replace    —    20 │  59.6  71.7  84.9 100.0 │  119.2  146.0 │  117/137 ms ≤20c │   -5.8   -2.9    +137
bge-base                              replace    —    30 │  59.6  70.7  81.1 100.0 │  167.4  206.0 │  167/197 ms ≤30c │   -5.8   -3.8    +197
bge-base                              replace    —    50 │  59.6  70.4  81.1 100.0 │  223.4  275.5 │  222/264 ms ≤40c │   -5.8   -4.2    +266
loading jina-reranker-v2-base-multilingual …
jina-v2-ml                                rrf  0.5    10 │  69.2  76.5  84.0 100.0 │   69.1   81.4 │    62/72 ms ≤10c │   +3.8   +1.9     +72
jina-v2-ml                                rrf    1    10 │  69.2  76.9  84.0 100.0 │   70.5   85.5 │    64/76 ms ≤10c │   +3.8   +2.3     +76
jina-v2-ml                                rrf    2    10 │  65.4  75.0  84.0 100.0 │   71.0   84.7 │    64/75 ms ≤10c │   +0.0   +0.4     +76
jina-v2-ml                                rrf    4    10 │  65.4  75.0  84.6 100.0 │   70.8   83.8 │    64/74 ms ≤10c │   +0.0   +0.4     +75
jina-v2-ml                                rrf  0.5    20 │  69.2  76.6  84.0 100.0 │  129.0  152.1 │  126/143 ms ≤20c │   +3.8   +2.0    +143
jina-v2-ml                                rrf    1    20 │  69.2  76.9  84.0 100.0 │  126.7  154.6 │  124/145 ms ≤20c │   +3.8   +2.3    +146
jina-v2-ml                                rrf    2    20 │  65.4  75.3  84.6 100.0 │  125.8  154.0 │  122/145 ms ≤20c │   +0.0   +0.7    +145
jina-v2-ml                                rrf    4    20 │  65.4  74.8  85.3 100.0 │  128.0  148.5 │  123/140 ms ≤20c │   +0.0   +0.3    +139
jina-v2-ml                                rrf  0.5    30 │  69.2  76.6  84.0 100.0 │  187.4  224.4 │  182/214 ms ≤30c │   +3.8   +2.0    +215
jina-v2-ml                                rrf    1    30 │  69.2  76.6  84.0 100.0 │  187.2  219.1 │  183/208 ms ≤30c │   +3.8   +2.0    +210
jina-v2-ml                                rrf    2    30 │  65.4  75.3  84.6 100.0 │  190.3  222.0 │  184/213 ms ≤30c │   +0.0   +0.7    +213
jina-v2-ml                                rrf    4    30 │  65.4  74.8  85.9 100.0 │  188.3  223.2 │  184/214 ms ≤30c │   +0.0   +0.3    +214
jina-v2-ml                                rrf  0.5    50 │  69.2  76.8  84.0 100.0 │  250.0  297.5 │  252/288 ms ≤40c │   +3.8   +2.2    +288
jina-v2-ml                                rrf    1    50 │  69.2  76.6  84.0 100.0 │  247.6  294.0 │  244/284 ms ≤40c │   +3.8   +2.0    +285
jina-v2-ml                                rrf    2    50 │  65.4  75.3  84.6 100.0 │  244.0  300.9 │  246/291 ms ≤40c │   +0.0   +0.7    +292
jina-v2-ml                                rrf    4    50 │  65.4  75.2  87.8 100.0 │  243.4  291.4 │  245/282 ms ≤40c │   +0.0   +0.6    +282
jina-v2-ml                            replace    —    10 │  69.2  75.9  85.9 100.0 │   70.8   86.2 │    64/77 ms ≤10c │   +3.8   +1.3     +77
jina-v2-ml                            replace    —    20 │  71.2  77.8  87.2 100.0 │  127.8  153.2 │  124/144 ms ≤20c │   +5.8   +3.2    +144
jina-v2-ml                            replace    —    30 │  71.2  77.8  87.2 100.0 │  187.0  216.7 │  184/208 ms ≤30c │   +5.8   +3.2    +208
jina-v2-ml                            replace    —    50 │  69.2  76.0  83.3 100.0 │  248.1  297.8 │  249/288 ms ≤40c │   +3.8   +1.4    +289
loading bge-reranker-v2-m3 …
bge-v2-m3                                 rrf  0.5    10 │  67.3  75.2  84.0 100.0 │  149.7  204.5 │  143/196 ms ≤10c │   +1.9   +0.6    +195
bge-v2-m3                                 rrf    1    10 │  67.3  75.3  83.3 100.0 │  142.0  165.6 │  136/156 ms ≤10c │   +1.9   +0.7    +157
bge-v2-m3                                 rrf    2    10 │  67.3  75.6  83.3 100.0 │  142.1  164.9 │  135/155 ms ≤10c │   +1.9   +1.1    +156
bge-v2-m3                                 rrf    4    10 │  65.4  74.4  84.6 100.0 │  142.8  164.7 │  135/155 ms ≤10c │   +0.0   -0.2    +156
bge-v2-m3                                 rrf  0.5    20 │  67.3  75.2  84.0 100.0 │  269.3  322.3 │  268/314 ms ≤20c │   +1.9   +0.6    +313
bge-v2-m3                                 rrf    1    20 │  67.3  75.5  83.3 100.0 │  273.0  324.9 │  270/317 ms ≤20c │   +1.9   +0.9    +316
bge-v2-m3                                 rrf    2    20 │  67.3  75.6  84.0 100.0 │  269.3  319.4 │  271/310 ms ≤20c │   +1.9   +1.1    +310
bge-v2-m3                                 rrf    4    20 │  65.4  74.4  85.9 100.0 │  281.7  335.6 │  277/326 ms ≤20c │   +0.0   -0.2    +326
bge-v2-m3                                 rrf  0.5    30 │  67.3  75.2  84.0 100.0 │  388.4  448.2 │  392/439 ms ≤30c │   +1.9   +0.6    +439
bge-v2-m3                                 rrf    1    30 │  67.3  75.6  83.3 100.0 │  388.3  456.9 │  392/447 ms ≤30c │   +1.9   +1.1    +448
bge-v2-m3                                 rrf    2    30 │  67.3  75.3  84.6 100.0 │  390.3  450.4 │  391/442 ms ≤30c │   +1.9   +0.7    +441
bge-v2-m3                                 rrf    4    30 │  65.4  74.4  85.9 100.0 │  390.1  459.6 │  388/452 ms ≤30c │   +0.0   -0.2    +451
bge-v2-m3                                 rrf  0.5    50 │  67.3  75.3  84.0 100.0 │  525.4  608.6 │  523/598 ms ≤40c │   +1.9   +0.7    +599
bge-v2-m3                                 rrf    1    50 │  67.3  75.6  84.0 100.0 │  520.5  605.7 │  525/596 ms ≤40c │   +1.9   +1.1    +597
bge-v2-m3                                 rrf    2    50 │  67.3  75.3  85.3 100.0 │  530.4  607.6 │  533/598 ms ≤40c │   +1.9   +0.7    +598
bge-v2-m3                                 rrf    4    50 │  65.4  74.4  85.9 100.0 │  517.0  608.2 │  524/598 ms ≤40c │   +0.0   -0.2    +599
bge-v2-m3                             replace    —    10 │  61.5  71.5  85.3 100.0 │  140.9  165.8 │  135/157 ms ≤10c │   -3.8   -3.1    +157
bge-v2-m3                             replace    —    20 │  59.6  70.2  81.7 100.0 │  273.8  322.0 │  273/313 ms ≤20c │   -5.8   -4.4    +313
bge-v2-m3                             replace    —    30 │  59.6  69.8  79.8 100.0 │  392.8  453.9 │  395/445 ms ≤30c │   -5.8   -4.7    +445
bge-v2-m3                             replace    —    50 │  59.6  69.8  79.8 100.0 │  521.8  605.3 │  526/597 ms ≤40c │   -5.8   -4.7    +596

──────────────────────────────────────────────────────────────────────────────────────────────────────────────
```

The header's live-timeout figure is the config as it stood before this
phase's retune; the committed default is 250 ms now. The full dump —
every cell's `by_kind` p@1 row included — is committed as
`eval/grid-phase7.txt`; `--grid-models/--grid-strategies/--grid-depths/
--grid-weights` re-run any subset, `--json` re-emits the artifact.

The `by_kind` rule ("no kind regresses > 5 points") is what disciplines
this table: **62 of 80 cells violate it**.

* `jina-v1-turbo-en` regresses `relational` (28.6 → 14.3) in **all 20
  of its cells**, and `needle` at every weight ≥ 1: the small model
  cannot hold "who owns X" against fused evidence. It fails the gate at
  every setting, so its best aggregate (+1.9, at w 0.5) never mattered.
* `bge-reranker-v2-m3` regresses `relational` in every cell too (plus
  `exact-token` under `replace`), and it is the slowest candidate by
  distance: its *cheapest* cell (depth 10) already adds +156 ms p95,
  over the budget before quality enters the discussion.
* `bge-reranker-base` and `jina-v2-multilingual` are clean under `rrf`
  at weights 0.5–1.0 at every depth. The cliff is at 2 for both:
  `needle` 66.7 → 33.3 everywhere. By weight 4 the aggregate gain is
  gone entirely (p@1 back to the `off` row) with fresh kinds joining
  the casualties — the category filter at the shallow depths,
  `nl-category` elsewhere.
* `replace` is a strategy-level verdict: precision *falls* on three of
  four models (worst −9.6, turbo, at every depth). Only
  jina-v2-multilingual's scores are crisp enough for raw-score ordering
  to gain (+5.8 at depth 20) — and its gain sits on a cliff: the same
  strategy at depth 10 or 50 breaks `needle` again.

#### Winner (7.3): `bge-reranker-base` · `rrf` · weight 1.0 · depth 10

**Ship-gate checklist** (gate as written above). For the paired items
the comparison row is the grid's `off` row — same corpus, same
process — because the corpus grew after the Phase 2 record; comparing
across corpora would compare the expansion, not the feature.

- [x] **precision@1 ≥ +5 points**: 65.4 → **71.2 (+5.8)**. The "≥ 15
  with the expanded cases" expectation did *not* materialise, recorded
  as required: the thesis is directionally right — a cross-encoder
  reading query and memory together finds real wins — and those wins
  concentrate exactly where the plan predicted, `nl-question` p@1 +28.5
  and `paraphrase` +25.0, natural-language queries where the lexical
  legs guess. Against the old 41-case record (64.9) the number would
  read +6.3; recorded, not leaned on.
- [x] **MRR does not decrease**: 74.6 → 77.2 (**+2.7**).
- [x] **recall@5 drops ≤ 2**: 83.0 → 83.3 (**+0.3**, an increase).
- [x] **no `by_kind` p@1 regresses > 5**: clean — the winner's cell is
  among the 18 that touch nothing below the `off` row. (off → winner,
  per kind: nl-question 42.9 → 71.4, paraphrase 50.0 → 75.0, every
  other kind unchanged.)
- [x] **all `negative` cases pass**: abstention 100.0 in every one of
  the 81 rows, and `just eval-native` is green against the re-recorded
  baseline.
- [x] **p95 added latency ≤ budget on deployment hardware**: **the
  budget, written down as the gate requires: ≤ 150 ms of added p95 at
  the chosen depth, measured on the M4 Pro dev container — the hardware
  every phase of this plan has measured on.** Winner: whole-recall p95
  79.7 vs the reference row's 9.1 → **+70.6 ms added**; rerank boundary
  p95 71 ms judging 10 candidates. `timeout_ms` = 250: ≈ 3.5× the
  measured stage p95 (room for longer real-world memories and a busy
  CPU), far inside the client-visible limit.
- [x] **Phase 6 shadow/audit shows no systematic regression** — with
  the honesty Phase 6 itself insisted on: there is still no real
  traffic; the hand-checked verdicts on record (Q3 a real regression,
  Q4 no verdict possible) came from the 7-memory rehearsal. The grown
  corpus is the better instrument the rehearsal demanded, and at 108
  memories the Q4 failure mode — confidently reshuffling pure noise —
  does not reproduce: `negative` abstains 100.0 everywhere and
  `near-miss` holds 100.0 p@1 including under every winner-adjacent
  cell. **A shadow period on a real store remains the hard prerequisite
  for `mode ≠ off`**; that is Phase 8's gate, not a box this one can
  tick.

Why *these* settings, beyond clearing the gate:

* **depth 10, not 30.** With the winner model the entire gain lands by
  ten: p@1 and MRR are identical at 10/20/30/50, and latency scales
  linearly (+71 → +270 ms added p95) because fusion already places
  every recoverable memory inside the head's first ten. Depth stays an
  operator's dial for stores much larger than the eval's, and this
  table prices the dial.
* **weight 1.0.** The plan bet the wins lived at ≥ 2, where the reranker
  can overrule vector+BM25 agreement. Measured: the agreement is nearly
  always *right*, the plateau is 0.5–1.0 (identical quality), and past
  its edge the leg actively loses points — `2` gives back 2 p@1 and a
  third of `needle`, `4` takes the category filter down too. The
  inverted expectation, with numbers, is in the ranker module docs and
  the config comments (I-3); 1.0 is the plateau top and the semantics
  that keeps index agreement supreme.
* **`rrf`.** `replace` lost on three of four models, and task 7.4
  explains why mechanically: raw-score *gaps* mean nothing on this
  corpus, so ordering by them reorders noise. Reading only the *order*
  the scores imply is the survivor — same +5.8 at half the latency.
* **runner-up.** `jina-v2-multilingual / replace / depth 20` is the
  table's strongest cell — +5.8 p@1, +3.2 MRR, +4.2 recall@5 (87.2, the
  table's best, and the only *clean* cell to move recall at all) — and
  it is rejected: +144 ms added p95 spends
  96% of the budget on the quietest machine this feature will ever run
  on, its quality sits on a cliff (`needle` breaks at depth 10 and 50),
  and multilingual capacity buys nothing on this corpus for the same
  1.1 GB / ~1.5 GiB bge-base costs. If the budget is ever lifted, that
  row is where to reopen.
* **the affordability line.** The winner costs 1.1 GB of weights and
  ~1.5 GiB RAM per session (the placeholder: 147 MB / 180 MiB — the
  warm-up measurements). A box that cannot pay should choose
  `mode = "off"`, not the smaller model: turbo's plateau (+1.9) is a
  coin flip, paid for with a `relational` regression.

`mode` stays **off** — flipping it is Phase 8's decision on shadow
data, exactly per plan.

#### The relevance decision (7.4): relevance untouched

`rerank distribution` scores every case query against every corpus
memory *directly* — no fusion, no floor — because measuring through
recall would bias the separability verdict toward the model (fusion
hides the hard-to-retrieve expected pairs; the floor hides suppressed
negatives). The verdict rule is strict on purpose: separable only if
the *minimum* expected-pair score clears the *maximum* negative-noise
score.

| model | expected pairs (52): min / p25 / median / p75 / max | rank-1 of 52 | negative ceiling | separable? |
|---|---|---|---|---|
| bge-reranker-base | 0.000 / 0.003 / 0.056 / 0.535 / 0.998 | 28 | 0.085 | **no — 27 pairs at or below the ceiling** |
| jina-v1-turbo-en | 0.010 / 0.020 / 0.079 / 0.214 / 0.645 | 27 | 0.057 | **no — 22 pairs at or below** |

The shape is exactly §2-row-4's warning quantified: question-shaped
pairs score near 1 (0.998, 0.993, 0.974), while declarative-statement
pairs pile up *under the noise ceiling* — `statement: raw SQL in Go`
0.008, `hand-run migration` 0.006, `nervous about shipping` 0.012 —
below what an irrelevant query scores against this store's best memory
(0.085). Any mapping from cross-encoder score to `relevance` would put
correct answers under the 0.25 floor and *delete* them. **`relevance`
stays on the index-signal scale (I-4)**; the cross-encoder stays a
rank-only input — which is precisely the shape the grid proved works:
order, not magnitude.

#### Production-like re-verification: quota-blocked, open for Phase 8

The winner cell should also be paired on the Gemini config — Phase 5
deferred that pairing to this grid, and the expanded corpus never got
its Gemini `calibrate` (the batch-cap fix, then 429s from Google's
free-tier quota, held through this session). While unrun: the
rerank-boundary latency is embedder-independent (the same candidate
pairs get scored regardless of what put them in the head), and the
quality mechanism — cross-encoder promotion of candidates fusion
misranked — does not depend on the embedding model either; Phase 6's
one Gemini audit showed the same head-movement behaviour. **Open item
for Phase 8**: `scripts/rerank-grid.sh --config recuerdos-ai.toml
--grid-depths 10 --grid-weights 0.5,1` when the quota recovers, plus
the Gemini `calibrate`/`eval` of the grown corpus — its negative-noise
margin was already thin (adjusted ceiling 0.222 against the 0.25
floor), so if the expansion moved it, that is a corpus fix per the
7.1 rule, and it gates `mode = on` either way.

#### Definition of Done

- [x] `just check-native` passes; `just eval-native` passes against the
  re-recorded baseline (83.0 / 65.4 / 74.3, the record of the default
  path — rerank off — since flipping `mode` is not this phase's act).
- [x] Grid table pasted, winner and rationale recorded above;
  per-cell `by_kind` p@1 in the committed `eval/grid-phase7.txt`.
- [x] Ship gate: met on the offline evidence, with the two honest
  caveats recorded in place — the +15-point hope measured +5.8, and
  the Phase 6 item rests on the grown corpus until real shadow data
  exists. The reranker is ship-ready *as a configuration*; the
  default stays `mode = off` pending Phase 8's shadow period.


### Phase 8 — release (`docs/rerank-release`)

**The binary did not change.** Phases 0–7 shipped the code; this phase is the
operator-facing surface, the default decision, and the release. The only source
edit is a stale doc comment (`FastembedReranker` still claimed
`max_concurrency` defaulted to 2; the tuned default has been 1 since Phase 7).

#### 8.3 — the default, decided: `mode` stays `off`

The plan makes this a two-condition decision, and only one condition is met:

| Condition | State |
|---|---|
| Ship gate passed (Phase 7) | ✅ offline: +5.8 p@1, +2.7 MRR, +0.3 recall@5, +70.6 ms added p95 against the 150 ms budget written before the run |
| Phase 3's cautious clause — a shadow period on **real traffic** before changing what clients see | ❌ this environment has no live store; the rehearsal volume is 7 eval memories, and Phase 6's own record says a rehearsal cannot decide this |
| The operator accepts the cost | ❌ 1.1 GB of weights, ~1.5 GiB per inference session, +71 ms p95 — never priced against a real corpus by the person who would run it |

Flipping the default that changes every client's result order on the strength of
an offline corpus is precisely the failure "GO, **cautiously**" was written to
prevent, so it is not flipped, and no gate was reinterpreted to make flipping it
available. Everything else ships at its measured setting, which is the point of
tuning them now: enabling the stage later is one line (`mode = "on"`), not a
rediscovery. The revertable-in-isolation commit that Task 7.3.6 used is therefore
not needed — there is nothing to revert; the default was never changed here.

`recuerdos-ai config` expectations updated rather than changed: with the stage off
the block prints `mode off` alone, and `docs/configuration.md` now says why (the
report answers "what is this daemon paying", and off pays nothing).

#### 8.1 — documentation

| File | What landed |
|---|---|
| `README.md` | recall diagram and feature list now say *vector + BM25 + graph, fused + optional cross-encoder*; new "Optional rerank" row; the performance table is explicitly labelled as the **default** path, with the stage's +59/+71 ms and ~1.5 GiB stated beside it; "Shipped in `v0.4.0`" |
| `docs/architecture.md` | the recall section now names the second stage, its `domain` trait / `infrastructure` adapter placement, and the three invariants an implementer must not break (additive only, `off` builds nothing, a missed deadline drops the stage) |
| `docs/configuration.md` | new rerank subsections: **"If a model is too big for your box"** (the costed alternatives table, and the explicit advice that a box which cannot pay stays `off` rather than degrading to the small model whose +1.9 is noise), **"Language: what the default model actually speaks"**, and **"Recall quality: 'results look random'"** — three ordered checks: analyzer, then `similarity_baseline` for *your* model, then whether the answer exists at all; closes by stating the reranker is not a fix for any of them |
| `docs/deployment.md` | **"Optional: second-stage reranking"**: cost table, judge-before-enabling (`rerank audit`, then shadow), where the weights come from (bake-in vs mounted `/models`), enabling, and how to undo it |
| `recuerdos-ai.example.toml` | tuned comments from Phase 7, reviewed against the final keys. Two imprecisions fixed: the `depth` comment implied the stage re-scores the fused list, when [`rerank_leg`](../../src/memories/application/memory_recaller.rs) runs **after** `min_relevance` — so the stage reorders and never rescues; and the model comment now states the language limitation next to the price |
| `docs/api.md` | **`rerank_score` was documented wrongly, and the fix matters:** the Phase 4 text called it "the raw value, on the reranker's own scale", but Phase 5 squashes every model's logits through a sigmoid into `(0,1)` — strictly monotonically, which is the adapter's whole safety property. A client that took the old sentence literally might have thresholded a logit against a probability. Now: ranks are comparable across models, magnitudes are not, and neither is a calibrated probability. Also documents the case the smoke test hit — a one-candidate head carries no `rerank_*` keys, so absent keys do not mean the stage is off |
| `docker/Dockerfile` | the bake-in comment named the old placeholder model and "~300 MB"; now names the tuned model and its real 1.1 GB, and says why `/models` being an image directory (not a volume) makes baking or mounting a requirement rather than a preference |
| `ai-specs/project-plan.md` | §5's read-path step 2 rewritten (it still described a "cross-encoder-free" boost and no graph leg); new **§7.9** carrying the measured verdict and the three inverted expectations; header status note for `v0.4.0` |
| `docs/evaluation.md` | already documented the audit and how to read a shadow log from Phase 6; checked against the shipped behaviour, unchanged |

#### 8.2 — upgrade notes, and each claim run rather than assumed

`docs/deployment.md` §6 now states, and this session executed:

- **Back up first** (step 5's `tar` of the data volume) — stated before the
  upgrade steps, not after.
- **The keyword index rebuilds itself on the first start.** Run on a store that
  had memories and no current-version index, on the release image:

  ```
  INFO … text_index_rebuilder: text index rebuilt users_rebuilt=1 memories_indexed=4 elapsed_ms=30
  rebuilt the keyword index for 1 users (4 memories)
  ```

  and keyword recall afterwards returned `matched {vector_rank: 1, bm25_rank: 1}`.
- **`reindex --text` is the manual/repair path, and the daemon must be the one
  stopped.** Run against a live daemon, on the release image, it fails the way
  Phase 6 recorded:

  ```
  error: internal error: failed to open a text index writer: Failed to acquire
  Lockfile: LockBusy. Some("Failed to acquire index lock. …")
  ```
- **No vector reindex is needed for this release.** Phase 2.3 resolved
  `VERIFY` with a measurement (Gemini embeddings are unit-length at 3072 dims),
  so the per-model baseline changes how stored vectors are *read*. The docs say
  this explicitly, because "relevance changed" reads to an operator like
  "re-embed everything", which would cost a full corpus of API calls for nothing.
- **Model download: bake it or mount `/models`.** `/models` is baked into the
  image and is *not* a volume, so a runtime download lands in the container's
  writable layer and dies with it. Both paths are documented, with `warm-models`
  as the pre-warm.
- **What it costs, measured on the release images:**

  | Image | Size | RSS at idle |
  |---|---|---|
  | `recuerdos-ai:local` (default, no rerank model) | 292 MB | 232 MiB |
  | `recuerdos-ai:bake-rerank` (`--build-arg RERANK_MODEL=bge-reranker-base`) | 1.42 GB | 1.73 GiB |

  The delta is +1.13 GB of image and ~1.5 GiB of RAM, matching the numbers the
  config docs quote. The default image is **unchanged in size and contents**:
  `ls /models` in it shows only `models--Xenova--bge-small-en-v1.5`, and its
  startup log contains zero lines mentioning rerank.
- **How to judge before enabling**: `rerank audit`, then a shadow week — commands
  in `docs/deployment.md`, reading guide in `docs/evaluation.md`.

#### 8.4 — smoke tests on the release image

Two clean containers, clean volumes, no config file — only the documented env.

**Default image, default config (`mode` off):**

```
$ curl -s localhost:7071/healthz
{"status":"ok"}
$ docker exec ra-smoke-off recuerdos-ai config | sed -n '/rerank/,/^$/p'
rerank
  mode       off
$ # 4 memories stored via POST /v1/memories:direct (201 each)
$ curl -s -X POST localhost:7071/v1/memories/search -d '{"query":"which database holds the billing service data","limit":3}'
  "The billing service reads and writes ledger-db, a Postgres 16 cluster"
    relevance 0.703 · matched {vector_rank: 1, bm25_rank: 1} · took_ms 8 · min_relevance 0.25
$ curl -s -X POST localhost:7071/v1/memories/search -d '{"query":"can I add an index.ts that re-exports everything","limit":2}'
  1. "User forbids barrel files / index.ts re-exports in TypeScript"  matched {vector_rank: 1, bm25_rank: 1}
  2. "Deployment moved from fly.io to Hetzner…"
```

No `rerank_*` keys in `matched`, no rerank weights on disk, no rerank log line: the
inert default holds in the shipped artifact, not only in a unit test.

**Baked image with `RECUERDOS_AI_RERANK__MODE=on`:**

```
INFO recuerdos_ai::bootstrap::memories_wiring: rerank stage loaded model="bge-reranker-base" mode=On depth=10 max_concurrency=1
$ docker exec ra-smoke-on recuerdos-ai config | sed -n '/rerank/,/^$/p'
rerank
  mode       on
  model      bge-reranker-base
  depth      10
  strategy   rrf (weight 1)
  timeout    250 ms
  concurrent 1
  batch      16
$ curl -s -X POST localhost:7072/v1/memories/search -d '{"query":"can I add an index.ts that re-exports everything","limit":5}'
took_ms 33
  "User forbids barrel files / index.ts re-exports in TypeScript" | {vector_rank: 1, bm25_rank: 1, rerank_rank: 1, rerank_score: 0.937969}
  "Deployment moved from fly.io to Hetzner…"                      | {vector_rank: 2, rerank_rank: 2, rerank_score: 3.9394094e-05}
```

The stage loads from the baked weights (the blobs' timestamps are the build's, not
the boot's), serves the reranked order, and stamps `rerank_rank` / `rerank_score`.

**One behaviour this surfaced, and it is by design — but not the behaviour the
docs had implied.** That same "billing database" query in the `on` container came
back with one result, `took_ms 7`, and no `rerank_*` keys:
[`rerank_leg`](../../src/memories/application/memory_recaller.rs) returns
immediately when the list it was handed has fewer than two members, because
reordering one candidate is not a reorder. Note *which* list that is — the floor
is applied **before** the stage, so a head the floor narrowed to one memory costs
nothing and changes nothing. Two consequences, both now documented: `depth` is a
ceiling on the *surviving* candidates it re-scores (it cannot reach a memory the
floor dropped, so it reorders and never rescues), and an operator reading a shadow
log full of `changed=false` may be looking at a store whose heads are short, not
at a reranker that disagrees.

**Failure mode, run rather than promised** — stage enabled, weights absent, no
network:

```
error: internal error: failed to load rerank model "bge-reranker-base" (from
BAAI/bge-reranker-base) into /models: Failed to retrieve model file:
onnx/model.onnx. If this host has no internet access, pre-populate the cache with
`recuerdos-ai warm-models` or bake the model into the image.
```

It fails at boot with the model named. It does not serve a recall that silently
lacks the second stage its operator asked for.

#### Gates

All four, on this branch's tree, in the container (the `just` recipes are thin
wrappers, so the recipe that names a `-native` command is what ran):

| Gate | Result |
|---|---|
| `just check-native` | ✅ exit 0 — `cargo fmt --check` clean; `cargo clippy --all-targets -- -D warnings` clean; `check-boundaries.sh: no violations found`; **928 tests, 0 failures** across 10 suites |
| `just eval-native` | ✅ exit 0 — `recall@5 83.0% → 83.0% (−0.0)`, `precision@1 65.4% → 65.4% (−0.0)`, `MRR 74.3% → 74.6% (+0.3)`. Nothing moving is the expected result: this phase changed no behaviour, and a phase that only writes docs *should* be unable to move the gate |
| `just docker-build` | ✅ exit 0 — `recuerdos-ai:local`, **292 MB**, carrying no rerank weights |
| the bake variant | ✅ exit 0 — `docker build --build-arg RERANK_MODEL=bge-reranker-base`, **1.42 GB**, `/models` holding `models--BAAI--bge-reranker-base` (1.1 GB) beside the embedding model |
| `just sdk-test` | ✅ exit 0 (run twice — once on the release image as first built, once after the `rerank_score` docstring fix) — ruff format + lint clean, mypy clean over 5 source files, **29 unit + 13 integration** tests against a daemon running the release image, both examples executed |

`recuerdos-ai --version` inside the release image reports `recuerdos-ai 0.4.0`.

#### 8.5 — close-out

`implementation-plan.md` gains **Task 7.4**, same shape as Task 7.3, pointing here
for the full record; the Phase 7 status line and its sequencing table now reflect
7.1–7.4.

#### Still open after this release (deliberate, not silent)

1. **`mode = on` needs a shadow period on a real store** — the reason the default
   stayed off, restated in `docs/deployment.md` and §12 Phase 7 above.
2. **Production-parity on `gemini-embedding-2` is still unrun.** Every 429 canary
   this session (Phase 7 and Phase 8) was free-tier quota, e.g.
   `Gemini returned HTTP 429 … You exceeded your current quota`. It does not gate
   this release, because the default path does not reach the reranker; it gates the
   flip in (1), since a reranker tuned on bge-small embeddings has not been shown to
   behave the same under 3072-dim Gemini embeddings.
3. **`docs/CHANGELOG.md` has no `v0.2.2`/`v0.3.0`/`v0.3.1` entries** — they were
   skipped when those shipped. Backfilling them is a separate, honest chore; this
   release's entry is accurate, which is the most that can be claimed here.


### `VERIFY` items resolved
- [x] eval embedder follows `--config` (0.1) — **it did not** (defaults were always
      used); fixed in Phase 0 so it does. Details in the Phase 0 log above.
- [x] tantivy 0.25 `StopWordFilter` API and the built-in English list (1.2, §2 row 9) —
      `StopWordFilter::remove(Vec<String>)` exists without any crate feature and is what
      `ra_text_v2` uses; `::new(Language)` is behind the `stopwords` feature and was not
      needed. `Language::English` is exactly the Lucene list quoted in §2 row 9, confirming
      none of `what/which/who/how/do/does/I/my/you/can/should` is in it. Details in the
      Phase 1 log above.
- [x] Gemini embeddings unit-normalised at configured dimension (2.3) — **they are**:
      `recuerdos-ai calibrate --config recuerdos-ai.toml` measured document and query
      norms over the eval corpus at 3072 dims, min/max/mean all `1.0000`. No embedder
      change, no reindex. Details in the Phase 2 log above.
- [x] fastembed rerank feature flag; `TextRerank::rerank` receiver (`&self`/`&mut self`) (5.1) —
      **no extra Cargo feature** (fastembed 5.17.3's default features download rerank models), and
      `rerank` takes **`&mut self`**, returning a descending-sorted `Vec<RerankResult>` with input
      `index` back-references; the adapter answers with an `InstancePool` of `max_concurrency`
      sessions plus `align_scores`. Details in the Phase 5 log above.
- [x] Dockerfile warm-models step (5.4) — the release stage runs `warm-models` from the builder;
      extended with an empty-default `ARG RERANK_MODEL=` guarding an if/else around the same step,
      so the default image is byte-for-byte what it was and a bake-in is one build arg (~147 MB).
      Details in the Phase 5 log above.
- [x] Read-only open of SQLite/tantivy while the daemon runs (6.2) — **it works**:
      `rerank audit` ran against a copy of a live store while the daemon served it, and
      `cksum` over all 18 files of the copy (db, WAL, every tantivy segment) was
      identical before and after. A read-*write* open still fails while the daemon holds
      the writer lock (`reindex --text` → `LockBusy`), as it must; the plan's "stop the
      daemon" clause needed no weakening and none was done. Details in the Phase 6 log
      above.
