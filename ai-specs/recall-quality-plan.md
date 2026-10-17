# Recuerdos AI — Recall Quality v2: Fast, Accurate Recall and Concept Relationships

**v0.1 · drafted 2026-10-17 · against `main` @ `bfe1d57` (v0.4.3) · companion to
[reranking-plan.md](reranking-plan.md) and [recall-accuracy-proposal.md](recall-accuracy-proposal.md)**

This plan turns the findings of the 2026-10-17 recall investigation into phased,
independently shippable work. Each phase changes what recall returns without
adding a model call at query time. The goal is the one the project was built
around: **fast recall, and accurate relationships between concepts**.

Like the reranking plan, it is written so that an agent (or a human) can pick up
**one phase at a time**, work until its **Definition of Done** holds, record the
numbers in [§17 Results log](#17-results-log), and stop.

Line references are to `main` @ `bfe1d57`.

---

## 0. How an agent must use this document

1. **Work phases in order**, except Phase 1, which is independent and may run in
   parallel with Phase 0. Every phase from 2 on depends on Phase 0's eval changes;
   without them its numbers are not comparable.
2. **One branch per phase**, named in the phase header. One logical change per
   commit, conventional-commit style (`fix(…)`, `feat(…)`, `chore(…)`, `docs(…)`),
   matching `git log`.
3. **The standard check passes at the end of every phase:** `just check-native`
   (`cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` +
   `bash scripts/check-boundaries.sh` + `cargo test`). Docker users run `just check`.
4. **Every phase that touches retrieval runs the eval** (`just eval-native`) and,
   once Phase 0.5 exists, the real-store comparison (`recall audit`). Record both in §17.
5. **Respect the architecture** (`docs/architecture.md`). Contracts (traits) live
   in `domain`, implementations in `infrastructure`, wiring in `bootstrap`. Every
   method touching user data takes `&UserContext`. Do not weaken
   `scripts/check-boundaries.sh`.
6. **"Knob at zero, no change" is a hard rule.** Every new behaviour ships behind a
   config value whose neutral setting leaves the pre-change path *byte-identical*
   (ids, order, `score` and `relevance` bits), pinned by a test. The pattern is
   the rerank identity test,
   `an_absent_or_off_reranker_leaves_recall_byte_identical`.
7. **Items marked `VERIFY` were not checked while writing this plan.** Confirm them
   against the code, crate docs or model cards before relying on them, and note the
   answer in §18.
8. **If a DoD cannot be met, stop and record why in §17.** Do not weaken the DoD to
   make it pass. Gates are decisions, not formalities: Phase 6 of the reranking
   plan is the precedent for a feature that measured well offline and did nothing
   on the real store.
9. **Never generate eval data with access to the eval's queries.** Probe and alias
   fixtures (Phases 5–6) are produced from memory content only, after the cases they
   are scored on are frozen. A fixture that has seen the questions measures nothing.

---

## 1. Why this plan exists (evidence)

### 1.1 The reranker does not earn its place

The cross-encoder stage (reranking-plan Phases 4–8) is correct as specified. On the
real store, though, it **changed 0 of 13 top results** (p@1 72.73 → 72.73; MRR
+1.06, which is one query) and cost 400–1200 ms per recall on the field host
(reranking-plan §12, "first audit against a real store"). Recall without it costs
8–25 ms (`docs/performance.md`). The reasons are structural:

- it only reorders the 4–5 results left after the relevance floor;
- at `rrf` weight 1 it cannot overrule vector+BM25 agreement, and weights ≥ 2 were
  measured worse;
- its model mis-scores one-sentence declarative memories (27 of 52 correct pairs
  at or below the negative-noise ceiling, Phase 7.4);
- it is a 278M-parameter model on a shared CPU (20–120 ms per pair, contended).

### 1.2 Where the remaining errors actually are

Per-leg debug output of the eval (`RA_EVAL_DEBUG=1`, 108 memories, 59 cases,
bge-small) shows three failure classes. Only the first is something a head
reranker can address.

| Class | Example | Mechanism | Fixed by |
|---|---|---|---|
| **Coin-flip ordering** | `auth review`, `the failed migration`, `hand-run migration` | Vector and BM25 disagree symmetrically (v1/k2 vs v2/k1). RRF gives both candidates the same rank evidence, and recency/id decides. In all three, the correct memory has the higher calibrated relevance (0.673 vs 0.659, 0.708 vs 0.649, 0.742 vs 0.725). | Phase 2 |
| **Buried relationships** | `who owns a decision about a service` → "Nadia is the tech lead of the Meridian team" | A hop-2 answer shares no words with the query, so only the graph finds it. One RRF leg scores at most 1/61 ≈ 0.0164, which loses to any memory both other legs return in their top ~60. More graph weight promotes the *bridge* memories, not the answers. | Phase 3 (and 6) |
| **Vocabulary gaps** | `dietary requirement` → "vegetarian"; `installing project dependencies` → pnpm; "anything about how I like my code organised" | The right memory is in no leg's head, so no reorder can reach it. | Phase 5 |

### 1.3 What was measured (2026-10-17)

All runs use `main` @ `bfe1d57`, default config (bge-small-en-v1.5), and no reranker.
Experiment switches are in
[experiments/recall-accuracy-2026-10-17.patch](experiments/recall-accuracy-2026-10-17.patch).

| Variant | recall@5 | p@1 | MRR | relational recall | abstention | p95 |
|---|---|---|---|---|---|---|
| control (= committed baseline) | 83.0 | 65.4 | 74.6 | 57.1 | 100 | 9.2 ms |
| reranker, Phase 7 winner (idle box) | 83.3 | 71.2 | 77.2 | — | 100 | +70.6 ms |
| ties broken by relevance | 83.0 | 69.2 | 76.5 | 57.1 | 100 | 9.0 ms |
| **ties by relevance + graph expansion slot** | **86.9** | 69.2 | **78.4** | **85.7** | 100 | 9.1 ms |
| bge query instruction | 83.3 | 63.5 | 73.1 | 57.1 | 100 | ✗ |
| category/tags/entities prepended to embedded text | 81.7 | 63.5 | 72.3 | 57.1 | 85.7 | ✗ |
| graph `rank_weight` 0.6–1.0 | 83.0–84.9 | 67.3–69.2 | 75.5–76.2 | 57–71 (p@1 → 0) | 100 | ✗ |
| query-to-triple seeding, no threshold | 83.0–83.7 | 55.8–63.5 | 70.1–73.9 | 57–71 | **0** | ✗ |

> **Correction that shapes Phase 0 and Phase 2.** The eval seeds every memory within
> a few milliseconds and recalls a moment later, so the recency band is the same for
> every memory and symmetric rank disagreements become **exact** score ties. In a
> real store, memories differ in age, the band separates such pairs, and a pure
> tie-break would rarely fire. The +3.8 p@1 above is therefore an upper bound
> measured under a simplification. Phase 0 makes the eval age-aware. Phase 2
> implements the fix as a bounded relevance multiplier that still acts when ages
> differ, and re-measures it. The graph slot does not depend on ages; its gain is
> structural.

---

## 2. Findings register

Every finding has an owner phase. "Defect" means behaviour that contradicts the
code's own documented intent. "Limit" means the design works as written but caps
accuracy.

| ID | Finding | Type | Where (main @ `bfe1d57`) | Phase |
|---|---|---|---|---|
| R1 | A timed-out rerank keeps running on a detached thread and holds the only model session (`max_concurrency = 1`). The next recall queues behind work nobody will read, `took_ms` includes that wait, and the CPU burns it on a shared box. | Defect | `memory_recaller.rs:630,639`; `fastembed_reranker.rs:213`; `config.rs:555` | 1 |
| R2 | The reranker cannot reach the errors that remain (§1.1). | Limit | — | 1 (off), 8 (docs) |
| F1 | RRF discards match strength. Symmetric disagreements tie on rank evidence and are settled by recency/id, never by the calibrated relevance computed right after. | Limit | `recall_ranker.rs:582–590, 779`; `memory_recaller.rs:404–428` | 2 |
| G1 | A graph-only answer can never enter the top 5 under RRF. Hop/edge-count ordering favours the bridge memories. | Limit | `sqlite_entity_graph.rs:387`; `config.rs:424–425` | 3 |
| G2 | Graph-only hits get a constant relevance of 0.35 (above the 0.25 floor). Safe only while seeds are exact names; any fuzzy seeding without a threshold leaks noise past the floor (measured: abstention 100 → 0). | Hazard | `recall_ranker.rs:630` | 3 (guard), 6 (constraint) |
| G3 | Entity names are canonicalised only by case, whitespace and possessive. "Postgres" and "PostgreSQL" are separate nodes, so relationships fragment. | Limit | `entity_key.rs` | 6 |
| G4 | Past-tense questions ("what did X run on *before*") cannot prefer the superseded fact. Relevance favours the current one. | Limit | — | 7 |
| E1 | The local and OpenAI-compatible/Ollama embedders ignore query vs document mode. The trait doc calls them symmetric, which is wrong for EmbeddingGemma, nomic-embed, e5 and Qwen3-Embedding (and for bge, though its instruction measured worse here). | Defect | `fastembed_embedder.rs:78`; `remote_embedder.rs:147`; `embedder.rs:16` | 4 |
| V1 | The eval corpus defines 4 memories twice (108 rows, 104 distinct), so duplicates take top-5 slots. | Defect (eval) | `eval/cases.toml` | 0 |
| V2 | Eval seeding has no notion of age (see the correction above). | Limit (eval) | `RecallCore` uses `SystemClock`, `memories_wiring.rs:279` | 0 |
| V3 | Relational p@1 penalises correct behaviour: for multi-hop questions the bridge *should* rank first and the answer beside it. | Limit (eval) | `bootstrap/eval.rs` | 0 |
| V4 | There is no tool to compare two retrieval configurations over a **real** store. `rerank audit` compares only reranked vs not. | Gap | `memories_wiring.rs:350–351` | 0 |

---

## 3. Design invariants (must hold after every phase)

1. **Knob at zero, no change** (§0.6): each new config value has a neutral setting,
   pinned by an identity test.
2. **No model call is added to the recall path.** Phases 5–6 add work at *ingest*
   or in nightly consolidation, never per query. The recall p95 budget per phase is
   **≤ +5 ms** on the eval and on `scripts/bench.sh 20000`.
3. **`relevance` keeps its meaning**: absolute, thresholdable, calibrated per model
   (reranking-plan invariant I-4). Phase 2 uses relevance to order results; it does
   not change how relevance is computed. Phase 5 extends relevance only through a
   separately calibrated baseline (task 5.4).
4. **A failing optional component degrades and never fails the request** (probe
   KNN, alias expansion, graph slot: `warn!` and fall back). A *startup* failure of
   an explicitly enabled component fails loudly.
5. **API changes are additive only.** New response fields are optional and omitted
   when absent (`skip_serializing_if`). The Python SDK's discriminated unions must
   keep parsing: **no new wire values in existing `Literal` fields** (e.g.
   `RerankSkipped.reason` is `Literal["deadline", "model"]` in the SDK).
6. **User isolation**: every new table carries `user_id`, every new query filters
   on it, and every new method takes `&UserContext`.
7. **Calibration is documented where it lives** (reranking-plan invariant I-3):
   every new weight, slot count or threshold has a doc comment explaining its value
   with the measurement behind it.
8. **The memories table stays the system of record.** Probe vectors, entity vectors
   and alias edges are derived, safe to rebuild, and rebuilt by a command.

---

## 4. Measurement protocol and gates

**Instruments.**
- `eval` (default config = bge-small, which is also the field host's embedder
  today) is the CI gate.
- `eval --config <other>` covers other embedders (Phase 4).
- `scripts/recall-grid.sh` (Phase 0.6) sweeps a knob.
- `recall audit --compare-config` (Phase 0.5) replays real queries over a real store.
- `scripts/bench.sh 20000 500` measures latency at scale.

**Phase gate.** All must hold, measured against the Phase 0 baseline; adjust only
in §17 with a reason.

- [ ] p@1 does not drop, and improves by ≥ 2 points **or** the phase's target kind
      improves by ≥ 15 points (`complete@5` for relational/chain kinds).
- [ ] MRR does not drop. recall@5 drops by ≤ 1 point.
- [ ] No `by_kind` p@1 or recall drops by > 5 points **unless** §17 traces the drop
      to a case whose control result was decided by an exact tie (the Phase 1
      "noise-propped" precedent), with the debug lines pasted.
- [ ] Abstention stays 100% (every `negative` case returns nothing at the floor).
- [ ] Recall p95 rises by ≤ 5 ms (eval and `bench.sh 20000`).
- [ ] `recall audit` on a copy of the field host's store, with the operator's query
      file: no query whose `expect` was at rank 1 loses it. Each top-1 change is
      reviewed by hand, with the verdict written in §17.

A phase that fails its gate records the numbers and stops. Its knob stays at the
neutral value, and the code may still merge as inert (like the reranker), but only
if §17 says why that is worth keeping.

---

## 5. What happens to existing memories

| Phase | Existing data needs | How | Model calls | Daemon |
|---|---|---|---|---|
| 0 | nothing | eval/tooling only | none | — |
| 1 | nothing | — | none | restart for config |
| 2 | nothing | query-time ordering | none | restart for config |
| 3 | nothing | query-time ordering | none | restart for config |
| 4 | **query-only prompt:** nothing (recalibrate). **Document prompt:** re-embed every memory. | the pin refuses to start until `recuerdos-ai reindex` | embeddings only | stopped for reindex |
| 5 | probes for existing memories; keyword index rebuild (`questions` field) | `recuerdos-ai probes backfill` (budgeted, resumable) + automatic text-index rebuild (`TEXT_INDEX_VERSION` → v3) | **LLM** (one per memory) + embeddings | running (backfill) |
| 6 | entity vectors + alias edges | `recuerdos-ai graph aliases --rebuild`; nightly incremental | embeddings; optional LLM confirm | running |
| 7 | nothing | query-time | none | restart for config |

---

## 6. Phase map

| Phase | Deliverable | Branch | Size | Ships on its own? |
|---|---|---|---|---|
| 0 | Age-aware, deduplicated eval; `complete@k`; new kinds; real-store `recall audit`; grid script | `chore/recall-eval-fidelity` | M | yes (tooling) |
| 1 | Reranker off on the field host; deadline work bounded and cancellable | `fix/rerank-deadline-work` | M | yes |
| 2 | Relevance-aware ordering (`[retrieval].relevance_weight`) | `feat/relevance-affinity` | S | yes |
| 3 | Graph expansion slot (`[graph].expansion_slots`) | `feat/graph-expansion` | S–M | yes |
| 4 | Per-model embedding prompts, pin fingerprint | `fix/embedding-prompts` | M | yes |
| 5 | Ingest-time question probes | `feat/question-probes` | L | yes (off by default until gated) |
| 6 | Entity resolution (alias edges) | `feat/entity-aliases` | M–L | yes |
| 7 | Temporal cues for "before / previously" questions | `feat/temporal-recall` | S–M | yes (exploratory) |
| 8 | Docs, defaults, release | `docs/recall-quality-release` | S | yes |

---

## 7. Phase 0: Make the eval measure what production does

**Goal.** Eval numbers transfer to a real store: memories have ages, there are no
duplicates, multi-hop is scored as a chain, and there is a tool to compare two
configurations over a real store.

**Branch:** `chore/recall-eval-fidelity`

### Tasks

- **0.1 Injectable clock for the eval.**
  - `RecallCore::build` hard-codes `SystemClock` (`memories_wiring.rs:279`). Add a
    crate-visible `Memories::build_with_clock(config, database, clock)` and route
    `RecallCore` through it. Production callers keep `build` and `SystemClock`.
  - In `bootstrap/eval.rs`, use a `SettableClock(Arc<Mutex<DateTime<Utc>>>)`
    implementing `Clock`. Before each seed, set it to `EVAL_NOW − age_days`. Before
    scoring, set it to `EVAL_NOW`, a fixed instant (e.g. `2026-06-01T00:00:00Z`).
  - Fixing `now` also removes the cross-process recency drift the reranking plan
    recorded (Phase 7: MRR 74.3 vs 74.6). After this task the eval is fully
    deterministic, except latency.
- **0.2 Per-memory ages.**
  - Add `age_days: Option<f32>` to `SeedMemory`.
  - Without it, derive a stable pseudo-random age in `[0, 365)` from a hash of the
    content (e.g. FNV-1a of the bytes, mod 365): no RNG state, identical across runs
    and machines.
  - Then audit the corpus for **order-dependent pairs** and give each an explicit
    `age_days` that encodes the intended order. These include `current-fact`
    supersessions, the Kafka→Kinesis pair, relations whose validity depends on
    `created_at` (graph `valid_from` is the memory's `created_at`), and any
    `near-miss` pair whose distractor is the "older" fact.
  - Validation rejects a corpus where a `current-fact` case's expected memory is not
    the newest of the memories sharing its subject entity.
- **0.3 Remove the duplicates (V1).** Delete the second definition of the 4 duplicated
  memories (pnpm, barrel files, table-driven Go tests, Vitest). Extend `validate()`
  to reject duplicate `content`, with a test.
- **0.4 `complete@k` per kind (V3).** A case is *complete* when **every** `expect`
  appears in the top k. Add it to `CaseResult` and `Report`
  (`complete_at_k_by_kind`, `#[serde(default)]` so old baselines load), print it,
  and make `compare()` gate it like `precision_at_1` (skip when the baseline lacks
  it). Relational/chain gates read `complete@5`; p@1 stays reported.
- **0.5 `recall audit --compare-config` (V4).** Generalise `rerank audit`
  (`bootstrap/rerank_audit.rs`, pipelines built in `memories_wiring.rs:350`):
  - `recuerdos-ai recall audit --queries <file> [--user <h>] [--limit 8] --config a.toml --compare-config b.toml [--json]`
    builds **two pipelines over one read-only store** with the existing guarantees:
    `execute_untracked`, read-only tantivy, no writes, `--user` handling.
  - Refuse when the two configs name different embedders: the store's vectors fit
    exactly one.
  - Report per query both top-k lists, the rank of `expect`, top-1 changed, and the
    summary (p@1, MRR, complete@k before → after, latency p50/p95 per side).
  - Keep `rerank audit` as a thin alias (A = config with `mode = off`, B = config as
    given) so documented commands keep working.
- **0.6 `scripts/recall-grid.sh`.** Loops `RECUERDOS_AI_*` env overrides over `eval`
  and prints one row per cell plus per-kind p@1/recall/complete@k deltas against the
  first row. Deterministic after 0.1, so separate processes are comparable; latency
  columns are informational.
- **0.7 New case kinds, `informational = true` until their phase lands.** Write and
  **freeze** these before any Phase 5/6 fixture exists (§0.9):
  - `chain` (≥ 4): two-hop questions with bridge + answer in `expect`.
  - `temporal` (≥ 4): "before / previously / used to / originally".
  - `alias` (≥ 4): the query names an entity by a variant (Postgres/PostgreSQL,
    k8s/Kubernetes, JS/JavaScript, "the API"/"backend API") absent from the
    answering memory.
  - `vocab-gap` (≥ 6): the answer shares no content word with the query
    (`dietary requirement`-style), across categories.

  Add corpus memories only where needed; keep realistic filler.
- **0.8 Re-record the baseline** (`just eval-record`) after 0.1–0.4. Paste the old →
  new numbers per kind in §17: ages and dedupe move them deliberately.
- **0.9 Real-store baseline.**
  - Agree with the operator on a query file (§1 queries + ≥ 10 of their own, with
    `expect` substrings).
  - Run `recall audit` with A = B = the deployed config to record a deterministic
    self-comparison (must show 0 changes) and the per-query ranks.
  - Store the query file in the operator's notes, not the repo (it describes their
    life).

### Tests

- The settable clock reaches the saver and the recaller: a memory seeded with
  `age_days = 30` reports `created_at = EVAL_NOW − 30 d`.
- Hash-derived ages are stable across two runs.
- `validate()` rejects duplicates and mis-ordered current facts.
- `complete@k` arithmetic: all, some, none, single expectation.
- An old baseline without `complete_at_k_by_kind` still loads and skips its gate.
- `recall audit`:
  - identical configs → zero changes and identical numbers;
  - different embedders → refused;
  - purity (memories, access counts and importance byte-identical after two runs,
    the Phase 6 test pattern);
  - `--json` contains no memory content.

### Definition of Done

- [ ] `just check-native` passes.
- [ ] Two consecutive `eval` runs produce identical recall/p@1/MRR/complete@k.
- [ ] New baseline committed; §17 shows the per-kind movement and explains it.
- [ ] `recall audit` self-comparison on a copy of the field host's store recorded
      in §17 (0 changes).
- [ ] The four experiment variants of §1.3 re-run on the age-aware eval through
      `scripts/recall-grid.sh` with the experiment patch. Their numbers go in §17 as
      the inputs to Phases 2–3.

---

## 8. Phase 1: Reranker off, and bounded deadline work

**Goal.** Recall stops paying for the reranker. If anyone turns it back on, a missed
deadline costs only the remainder of one chunk, never the next request's latency.

**Branch:** `fix/rerank-deadline-work`

### Tasks

- **1.1 Operations (no code).**
  - Set `[rerank].mode = "off"` on the field host and drop the `bake-rerank` image.
  - Record the recall p50/p95 before and after from `took_ms`.
  - Expected: 105–568 ms → ~10–25 ms, and ~1.5 GiB RSS freed.
- **1.2 Timed session acquisition.**
  - Replace `InstancePool`'s "block on `instances[0]`" (`fastembed_reranker.rs:205–216`)
    with a **token channel** used as a semaphore: `max_concurrency` tokens, where
    `recv_timeout(remaining)` acquires and dropping the guard returns the token.
    Each token indexes one session. This gives a timed lock without a new dependency.
  - Extend the domain contract with a defaulted method:

    ```rust
    /// Scores within `deadline`; `Err(RerankBusy)` when no session frees in time.
    fn score_before(&self, query: &str, candidates: &[RerankCandidate<'_>], deadline: Instant)
        -> Result<Vec<f32>> { self.score(query, candidates) }
    ```

    The fastembed adapter overrides it with the timed acquire. Model the busy case
    as a new `RaError` variant **or** a typed marker inside `RaError::Internal`. Pick
    the variant, keep it internal, and `VERIFY` that no HTTP status mapping changes.
- **1.3 Chunked, cancellable scoring with partial application.**
  - In `rerank_scores` (`memory_recaller.rs:603–655`), the worker scores the head in
    chunks of `Reranker::preferred_chunk()` (a new defaulted method returning
    `None` = whole head; the adapter returns its `batch_size`).
  - After each chunk it sends `(offset, scores)` on the channel and checks an
    `Arc<AtomicBool>` cancel flag.
  - At the deadline, the recaller sets the flag, keeps the **contiguous prefix**
    already received, and applies the rerank to it when ≥ 2 candidates were judged
    (`RerankTrace::Applied { judged: n }`, already a field). Otherwise it reports
    `Skipped`.
  - The worker exits after the chunk in flight and returns its session. Abandoned
    work is bounded by one chunk instead of a whole head.
- **1.4 Wire compatibility.**
  - A busy or partial-deadline outcome is reported on the wire as
    `skipped`/`deadline` (when < 2 judged) or `applied` with `judged < depth`. **No
    new `reason` value** (invariant 3.5: the SDK's `Literal["deadline", "model"]`
    would reject it).
  - The distinction (`busy`, `partial`) goes to the `warn!` line and to
    `recall audit`/`rerank audit` counters.
- **1.5 Timing truthfulness.**
  - Split the leg's elapsed time into `queued_ms` (until a session was acquired) and
    `inference_ms`, in logs and in the audit's latency report.
  - On the field host with `mode = "on"`, fire 3 concurrent recalls and record both
    numbers in §17. This confirms or refutes the hypothesis that R1 explains the
    34 → 120 ms/pair drift (reranking-plan §12, part three).
- **1.6 Docs.**
  - `README.md`: the "+5.8 precision@1" line gains the real-store result (0/13 top-1
    changes) and the recommendation to leave it off.
  - `docs/configuration.md` `[rerank]`: the sizing section explains chunked partial
    application.

### Tests

- `max_concurrency = 1` with a `SleepingReranker`: a second concurrent call returns
  `busy` (wire: `skipped`/`deadline`) within its deadline, not after the first
  call's full sleep.
- Cancellation: a chunked fake with 5 chunks and a deadline after chunk 2 → the
  worker stops after at most chunk 3 (counter), and the session token returns.
- Partial application: 2 of 5 judged → the first two reordered by score, the rest
  in fused order, `judged = 2`.
- 1 judged → `skipped`.
- Default `score_before`/`preferred_chunk` keep the Phase 4 behaviour (identity
  over the existing rerank tests).
- The SDK test suite (`just sdk-test`) passes unchanged.

### Definition of Done

- [ ] `just check-native` and `just sdk-test` pass.
- [ ] Field host on `mode = "off"`; p50/p95 before/after recorded in §17.
- [ ] Concurrency measurement (1.5) recorded, with the R1 hypothesis marked
      confirmed or refuted.
- [ ] `eval` with `mode = off` identical to Phase 0's baseline.

---

## 9. Phase 2: Relevance-aware ordering

**Goal.** When the legs' rank evidence is equal or nearly equal, the memory that
matches the query better comes first, including when the two memories differ in
age.

**Branch:** `feat/relevance-affinity`

### Design

Today, `score = (rrf_vector + rrf_bm25 + graph_term) × recency × quality`
(`recall_ranker.rs` `score()`). Relevance is computed afterwards in the recaller
(`memory_recaller.rs:417–428`) and used only by the floor. Add a third bounded
multiplier, **affinity**, built from relevance with the same shape as the existing
bands:

```
affinity(r) = 1 − δ · (1 − r)         r ∈ [0, 1],  δ = [retrieval].relevance_weight ∈ [0, 0.5]
score'      = score × affinity(relevance)
```

- `δ = 0` → `affinity ≡ 1` → byte-identical ordering (the identity test).
- For a symmetric tie (equal rank evidence), the comparison becomes
  `recency_a·quality_a·affinity_a` vs the same for `b`. Relevance now competes with
  age instead of being ignored. With δ = 0.2, a relevance gap of 0.06 is worth
  ≈ 1.3%, and one day of age (90-day half-life) ≈ 0.77%. The grid decides how much
  weight relevance deserves.
- Bounded by construction: affinity ∈ `[1 − δ, 1]`. With δ ≤ 0.5 it cannot reverse
  a large rank gap (rank 1 vs rank 10 on one leg ≈ 13% apart). The module docs
  carry the arithmetic (invariant 3.7).

### Tasks

- **2.1 Ranker.**
  - `RecallRanker::with_relevance_weight(δ)`.
  - `pub fn apply_relevance(&self, ranked: &mut Vec<ScoredMemory>)` multiplies
    `score` by `affinity(relevance)` and re-sorts with `sort_ranked`.
  - **When δ = 0, return before touching anything** (the `apply_rerank` empty-leg
    pattern: identity by code path, not by arithmetic).
- **2.2 `sort_ranked` total order.** Make the comparator total and explicit:
  `score desc`, then `relevance desc`, then `created_at desc`, then `id asc`. Use
  `f32::total_cmp` so a NaN can never make the sort panic (Rust ≥ 1.81 sorts may
  panic on inconsistent comparators). During fusion, relevance is still 0 for every
  candidate, so the extra key is inert there.
- **2.3 Recaller ordering.** In `execute_with`, the order becomes:
  1. fuse → attach relevance;
  2. **`apply_relevance`** (new);
  3. floor;
  4. rerank leg;
  5. graph slot (Phase 3);
  6. truncate; `touch_accessed`.
- **2.4 Rerank compatibility.** `apply_rerank`'s `Rrf` branch recovers the band as
  `score / base`. With affinity applied, the recovered band includes affinity, so
  the rerank leg is added *inside* recency × quality × affinity. That is intended:
  document it in the module docs and pin it with a test (rerank weight 0 still
  moves nothing).
- **2.5 Config.**
  - `[retrieval].relevance_weight` (`f32`, validated `0.0..=0.5`; NaN rejected).
  - Env override `RECUERDOS_AI_RETRIEVAL__RELEVANCE_WEIGHT`.
  - Shown by `recuerdos-ai config`; included in the shipped example **inside
    `[retrieval]`** (the v0.4.1 orphaned-key lesson; `unrecognised_keys` will catch
    a misplacement).
  - Default stays **0.0** until 2.6 picks a value.
- **2.6 Calibration grid** (age-aware eval, then `recall audit`):
  - δ ∈ {0, 0.05, 0.1, 0.2, 0.3, 0.5}.
  - Reference rows: the experiment patch's `RA_EXP_FUSE=tiebreak` (exact ties only)
    and the field host's deployed config.
  - Choose the smallest δ on the gain plateau that passes the §4 gate. Write the
    chosen value and its measurement into the doc comments of
    `DEFAULT_RELEVANCE_WEIGHT` and the config field.
- **2.7 Docs.** `docs/configuration.md` (what it does, how to read it, neutral
  value) and the `recall_ranker.rs` module docs ("Relevance leads; recency and
  quality break ties" gains the affinity band and its bound).

### Tests

- δ = 0: fingerprint (ids, order, `score`/`relevance` bits) identical to the
  pre-change build over the existing recaller fixtures (the anchor).
- Exact symmetric tie (v1/k2 vs v2/k1), equal ages → the higher-relevance memory
  first.
- Same tie with a 1-day age gap: at δ = 0.2 relevance wins for a 0.06 gap; at
  δ = 0.05 recency wins. Both pinned, which documents the trade.
- Bound: at δ = 0.5, a memory 5 ranks lower on both legs never overtakes, whatever
  its relevance (arithmetic pinned).
- Graph-only hit (relevance = `GRAPH_HOP_RELEVANCE`) under δ > 0: penalised exactly
  as the formula says. Phase 3's slot is unaffected (structural).
- NaN relevance cannot panic the sort.
- Rerank `Rrf` weight 0 with δ > 0 → order unchanged by the rerank leg.

### Definition of Done

- [ ] `just check-native` passes.
- [ ] Grid table (eval and `recall audit`) pasted in §17. Chosen δ and rationale in
      the doc comments.
- [ ] §4 gate holds at the chosen δ. Otherwise the default stays 0.0 and §17 says why.
- [ ] Baseline re-recorded at the chosen default.

---

## 10. Phase 3: Graph as expansion, not as a competing ranker

**Goal.** When the query names a known entity, the best graph-connected memory is
shown next to the top result, so an agent receives the chain (bridge fact + answer)
instead of only the bridge.

**Branch:** `feat/graph-expansion`

### Design

RRF arithmetic makes a one-leg memory lose to any two-leg memory (G1). More graph
weight only raises the bridge memories (measured, §1.3). So the graph does not
compete for rank. It **reserves slots**:

```
after floor and rerank, before truncate:
  for s in 1..=expansion_slots:
     pick the first id in graph-leg order whose position in `ranked`
       is ≥ limit − s           (i.e. it would not otherwise be visible)
       and that survived filters and the floor (it is in `ranked` at all)
     move it to index s          (never index 0: the top result is untouched)
```

Properties:
- **p@1 cannot change**, because index 0 is never touched (pinned).
- Filters and the floor are respected, because only memories already in `ranked`
  can move.
- With no seeds (the query names nothing known), nothing changes.
- `expansion_slots = 0` → identity.

### Tasks

- **3.1 Config.**
  - `[graph].expansion_slots` (`usize`, validated `0..=3`, default 0 until 3.5).
  - Plumbed into `MemoryRecaller` next to `max_hops`/`hop_limit`.
  - Inert when `[graph].enabled = false`.
- **3.2 Recaller.**
  - Keep `graph_hits` (already computed in `execute_with`) and apply the slot step
    at the position in the design.
  - The experiment patch placed it before the floor. Moving it after the floor and
    after the rerank leg is deliberate (the floor's verdict stands, and the reranker
    cannot bury it). **Re-measure**, and record any difference from §1.3 in §17.
- **3.3 Response provenance (optional, additive).**
  - `matched.graph_rank` is already on the REST response. Optionally add
    `matched.via` (the seed entity's display name) so an agent can see *why* a
    memory is related.
  - This needs `EntityGraph::neighbours` to return `(memory_id, hop, seed_key)`.
    Extend the recursive CTE (`sqlite_entity_graph.rs:358–388`) to carry the
    originating seed through `reachable` (`MIN(depth)` per key, keep any seed for
    the minimum), and add a struct `GraphHit`.
  - REST: `skip_serializing_if`. SDK: optional field with a present/absent parsing
    test.
  - MCP text: **do not** add it in this phase. The tool output is the agent's
    context budget, and Phase 8 decides.
- **3.4 G2 guard.**
  - A test that the graph leg's seeds come only from `EntityGraph::seeds` (exact
    canonical names) or alias classes (Phase 6).
  - A doc comment on `GRAPH_HOP_RELEVANCE` stating the constraint: any seed source
    that is not exact must carry a similarity threshold, or graph-only relevance
    must be derived from the path rather than this constant.
- **3.5 Grid.**
  - `expansion_slots` ∈ {0, 1, 2} × Phase 2's δ, and `rank_weight` ∈ {0.3, 0.15}.
    The graph's RRF term is less needed once the slot carries relationships;
    re-check whether it still earns its precision cost.
  - Gate on `complete@5` for `relational` + `chain`, and on everything else not
    regressing.

### Tests

- Slots 0: fingerprint identical (anchor).
- Bridge/answer fixture ("billing service maintained by Meridian team", "Nadia is
  tech lead of Meridian team", query "who owns decisions about the billing
  service"): with slots 1 the answer is at index 1 and index 0 is unchanged.
- No seed → no change.
- The candidate is already visible → no move.
- A category filter excludes the graph hit → no move.
- The graph hit is below the floor → no move.
- `limit = 1` → nothing visible changes.
- Rerank on + slot: the slot is applied after the rerank leg (order pinned).
- (3.3) `via` names the seed. REST omits it when absent.

### Definition of Done

- [ ] `just check-native` (and `just sdk-test` if 3.3 lands) passes.
- [ ] Grid pasted in §17. Chosen `expansion_slots` default with rationale in doc comments.
- [ ] §4 gate holds, including `complete@5` for `relational`/`chain` up ≥ 15 points.
- [ ] `recall audit` on the field host's store: every changed result reviewed by hand.

---

## 11. Phase 4: Per-model embedding prompts

**Goal.** Every embedding model is called the way its model card says it was
trained (E1). A deployment on Ollama or LM Studio models stops silently losing
retrieval quality.

**Branch:** `fix/embedding-prompts`

### Tasks

- **4.1 Prompt profiles.** New module
  `providers/infrastructure/embeddings/prompts.rs` (adapter knowledge, not domain):

  ```rust
  pub struct PromptProfile { pub id: &'static str, pub query: Option<&'static str>, pub document: Option<&'static str> }
  pub fn profile_for(model_id: &str) -> Option<&'static PromptProfile>
  ```

  Matching is case-insensitive on the model name with the provider prefix and tag
  stripped (`google/embeddinggemma-300m`, `embeddinggemma:300m`,
  `embeddinggemma-300M-GGUF` → `embeddinggemma`). Initial table, **each row
  `VERIFY` against its model card before merging**:

  | Family | Query | Document |
  |---|---|---|
  | `embeddinggemma` | `task: search result \| query: ` | `title: none \| text: ` |
  | `nomic-embed-text` (v1, v1.5) | `search_query: ` | `search_document: ` |
  | `e5` / `multilingual-e5` | `query: ` | `passage: ` |
  | `qwen3-embedding` | `Instruct: Given a question, retrieve stored notes that answer it\nQuery: ` | — |
  | `mxbai-embed-large`, `snowflake-arctic-embed` | `Represent this sentence for searching relevant passages: ` | — |
  | `bge-*-v1.5` | — (**measured worse here**, §1.3) | — |
  | OpenAI `text-embedding-3-*` | — | — |

  Gemini keeps `taskType` and gets no profile.
- **4.2 Config escape hatch.**
  - `[embeddings].query_prompt` / `[embeddings].document_prompt` (`Option<String>`).
    An explicit value wins over the table, and an explicit empty string disables a
    table entry.
  - Startup logs the effective profile at `info`
    (`embedding prompts: query="search_query: " document="search_document: " (from table: nomic-embed-text)`).
- **4.3 Apply in both embedders.**
  - `FastembedEmbedder::embed` and `RemoteEmbedder::embed` stop ignoring `task`
    (`fastembed_embedder.rs:78`, `remote_embedder.rs:147`). Prepend the profile's
    prompt for that task.
  - The startup dimension probe embeds as `Document`, which is unchanged.
- **4.4 Pin fingerprint.** A document prompt changes every stored vector, so it must
  be part of the store's identity:
  - Add `Embedder::pin_id()` (default: `model_id()`). Embedders with a document
    prompt return `"{model_id}#doc:{8-hex fingerprint of the prompt}"`.
  - `SqliteMemoryRepository` pins and verifies `pin_id()` instead of `model_id()`
    (`sqlite_memory_repository.rs:97–140`). No migration: existing stores hold the
    plain model id. Enabling a document prompt on one makes `verify_pin` refuse to
    start with the existing "run `recuerdos-ai reindex`" message, extended to say
    the prompt changed.
  - A query-only prompt does not touch the pin.
  - `SIMILARITY_BASELINES` stays keyed by `model_id()`, but each row's doc comment
    states which profile it was measured with.
- **4.5 Fix the contract's doc** (`embedder.rs:10–26`): replace "the local ONNX one,
  OpenAI's endpoint" with the actual rule (asymmetric models need prompts, supplied
  by the adapter's profile table), and point at `prompts.rs`.
- **4.6 Measure.**
  - Run `eval --config` against at least `embeddinggemma` and `nomic-embed-text`
    served by Ollama, with and without the profile (`document_prompt = ""` to
    disable).
  - Run `calibrate` for each and add `SIMILARITY_BASELINES` rows where none exist.
  - Paste the tables in §17.

### Tests

- `profile_for` matches the name variants above, and returns `None` for unknown and
  for bge.
- A wiremock remote embedder sends `search_query: …` for `Query` and
  `search_document: …` for `Document` (request-body assertion), and no prefix when
  the config override is `""`.
- Explicit config beats the table.
- Pin:
  - an existing store pinned `nomic-embed-text` + a newly enabled document prompt
    → startup refused with the reindex hint;
  - after `reindex`, the pin is `nomic-embed-text#doc:…` and startup succeeds;
  - a query-only change does not refuse.
- bge-small path is byte-identical (no profile) with the existing fastembed tests.

### Definition of Done

- [ ] `just check-native` passes.
- [ ] Measurements for ≥ 2 prompt-requiring models in §17 (with vs without), plus
      `calibrate` output.
- [ ] Release notes text drafted (for Phase 8): who must reindex and why.
- [ ] Every 4.1 table row's `VERIFY` resolved in §18 with the model-card link.

---

## 12. Phase 5: Ingest-time question probes

**Goal.** Close vocabulary gaps (§1.2, third class) by doing at write time what the
cross-encoder tried at query time: bridge "a question" to "a statement". Each memory
is indexed together with a few questions it answers.

**Branch:** `feat/question-probes`

### Design

The approach is Hypothetical Prompt Embeddings (HyPE, arXiv 2607.29402), precomputed
at ingest.

- **Ingest.** Extraction (already an LLM call) returns 0–3 `questions` per memory.
  They are stored, embedded as `Document`, and indexed in BM25.
- **Recall.** The vector leg runs one extra KNN over the probe vectors, using the
  same query embedding (no new model call). Hits merge by memory id taking the max
  similarity, so a memory found through a probe gets a vector rank.
- **Relevance.** `relevance` uses `max(correct_memory(sim_memory), correct_probe(sim_probe))`,
  each with its own calibrated baseline. Question↔question cosines run higher than
  question↔statement, so reusing the memory baseline would let probe noise pass the
  floor.

### Tasks

- **5.1 Extraction.**
  - `prompts/extraction.md` gains a `questions` field: up to 3 short questions a
    user or agent would ask that this memory answers, in different words from the
    memory, no yes/no questions, no copying the memory.
  - `Candidate` and `NewMemory` gain `questions: Vec<String>` (`#[serde(default)]`).
    Validated: ≤ 3, each ≤ 160 chars, trimmed and deduplicated.
  - `[retrieval.probes].enabled = false` by default. With the understanding provider
    set to `"none"` the feature is unavailable and says so once at startup.
  - The REST/MCP save surfaces accept optional client-supplied `questions` (additive),
    so `:direct` writers can provide them.
- **5.2 Storage.** Migration `V10__memory_probes.sql`:
  - `memory_probes(id TEXT PK, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE, memory_id TEXT NOT NULL, text TEXT NOT NULL, created_at TEXT NOT NULL)`
    + index `(user_id, memory_id)`.
  - A `vec0` table `vec_memory_probes(probe_id TEXT PRIMARY KEY, user_id TEXT PARTITION KEY, memory_id TEXT, embedding float[N])`,
    created lazily at the store's dimension, exactly like `vec_memories`
    (`sqlite_vector_index.rs:37–50`).
  - A new domain contract `ProbeIndex` (`upsert_many(context, memory_id, probes)`,
    `remove(context, memory_id)`, `search(context, vector, k) -> Vec<(MemoryId, f32)>`),
    with an `SqliteProbeIndex` adapter.
- **5.3 Keep probes in sync on every write path.** Each of these owns a test:
  - `DirectMemorySaver` (`direct_memory_saver.rs:57–75`): embed the content and
    probes in **one** batch call, then write them.
  - The reconciler's store path: as the saver.
  - `MemoryUpdater`: a content change deletes the probes and marks the memory for
    regeneration (the backfill picks it up). A label-only change keeps them.
  - `MemoryForgetter`: remove the probes.
  - Consolidation merge: the merged memory gets the de-duplicated union (≤ 3,
    preferring the survivor's), re-embedded.
  - `reindex` (vectors): re-embed the probes too (`sqlite_reindexer.rs`).
  - Keyword index: add a `questions` text field and bump `TEXT_INDEX_VERSION` to
    `v3`. The Phase 1 (reranking plan) machinery rebuilds every user's index
    automatically on first start, and the rebuilder reads probes alongside content.
- **5.4 Recall.**
  - In `execute_with` (`memory_recaller.rs:350–355`), when probes are enabled, run
    `probes.search(context, &embedding, depth)` and merge it into the vector leg by
    memory id with max-similarity, before `ranked(...)` builds `vector_hits`.
  - A probe failure warns and falls back to the memory-only leg (invariant 3.4).
  - Relevance: `RelevanceCalibration` gains `probe_baseline`, and `relevance()` takes
    the max of the two corrected similarities.
  - `calibrate` reports a probe noise ceiling and signal floor (negatives vs
    expected memories through their probes) and suggests the baseline.
  - Add a `PROBE_BASELINES` map next to `SIMILARITY_BASELINES`, with the same
    warn-on-unknown fallback.
- **5.5 Backfill.**
  - `recuerdos-ai probes backfill [--budget N]`, modelled on `GraphBackfiller`
    (`consolidation/application/graph_backfiller.rs`): per-user watermark,
    resumable, budgeted LLM calls, `--dry-run` count.
  - Nightly consolidation regenerates probes for memories marked by 5.3.
  - Prints per-user counts and token spend.
- **5.6 Eval without leakage (§0.9).**
  - `scripts/generate-probes.py` reads **only** the `[[memory]]` blocks of
    `eval/cases.toml` and runs the real extraction prompt with a configured model
    (record the model and its version). It writes `eval/probes.toml`
    (`content → questions`). It must not open the cases.
  - The `vocab-gap` cases are frozen in Phase 0.7, before the first generation.
  - The eval seeds probes from `eval/probes.toml` when
    `[retrieval.probes].enabled = true`.
  - A test asserts that every probe key matches a seeded memory, and that
    `generate-probes.py` contains no reference to `[[case]]`.
- **5.7 Grid and cost.**
  - Probes on/off × Phase 2/3 defaults on the eval.
  - `scripts/bench.sh 20000 500` p95 with probes (3 probes/memory → ~60k probe
    vectors).
  - Ingest token cost per memory.
  - Gate per §4 with `vocab-gap`, `needle`, `nl-question`, `nl-category` as the
    target kinds.

### Tests

- Extraction parses `questions`, validates the limits, and tolerates its absence.
- Every 5.3 write path keeps the probes consistent (insert, update content, update
  labels, forget, merge, reindex).
- Probe search is user-scoped (two users, same question text, no crossing).
- Merging into the vector leg takes the max and does not duplicate ids.
- A probe failure degrades with `warn!`.
- Relevance with probes: probe noise at the measured ceiling stays under the floor;
  an expected memory reached only through a probe clears it.
- `probes.enabled = false` → fingerprint identical (anchor).
- Backfill is idempotent, resumes from the watermark, and respects the budget.

### Definition of Done

- [ ] `just check-native` and `just sdk-test` pass.
- [ ] `eval` with probes: target kinds up per §4, no regression elsewhere,
      abstention 100%.
- [ ] `bench.sh 20000` p95 delta ≤ 5 ms recorded.
- [ ] Field host: backfill run on a copy, then `recall audit` (A = probes off,
      B = on), reviewed by hand.
- [ ] Default decision recorded for Phase 8. It may stay off: LLM cost at ingest is
      the operator's call.

---

## 13. Phase 6: Entity resolution (aliases)

**Goal.** One concept is one node. "Postgres", "PostgreSQL" and "the database" (where
it refers to the same thing) join the same relationship neighbourhood, so concept
relationships stop fragmenting (G3).

**Branch:** `feat/entity-aliases`

### Tasks

- **6.1 Entity vectors.**
  - `vec_entities(entity_key TEXT, user_id TEXT PARTITION KEY, embedding float[N], PRIMARY KEY …)`
    (`VERIFY` vec0 composite-key support; otherwise a synthetic id).
  - Embed `"{name} ({kind})"` once per new canonical key at ingest, batched with the
    memory's embedding call.
  - Backfill: `recuerdos-ai graph aliases --rebuild` (embeddings only, no LLM).
- **6.2 Alias detection.**
  - For each new entity key, find its nearest existing keys **of compatible kind**
    with cosine ≥ τ.
  - τ is calibrated per model with a new `calibrate --aliases` mode, using the
    Phase 0.7 `alias` pairs as positives and same-kind non-aliases as negatives.
    HippoRAG 2 used 0.8 as a starting point; `VERIFY` on this corpus.
  - Optional LLM confirmation (`[graph].alias_confirm = true`) batches candidate
    pairs into the nightly consolidation run, using the understanding provider.
- **6.3 Storage.**
  - `entity_aliases(user_id, key_a, key_b, score, source, created_at)` with
    `key_a < key_b`.
  - Alias classes are the connected components, computed on read (personal-scale
    graphs are small) or cached per user and invalidated on write.
- **6.4 Traversal.**
  - Seeds expand to their alias class before the walk.
  - In the recursive CTE (`sqlite_entity_graph.rs:358`), alias edges are
    **zero-hop**: an alias of a depth-*d* key is also depth *d*.
  - Seeding itself stays exact-name (G2): an alias class is entered only through a
    name the user actually wrote.
- **6.5 Hygiene.**
  - Forgetting the last memory that declares a key removes its vector and alias
    rows.
  - Merges re-project as today.
  - `recuerdos-ai graph aliases --list` shows each class with its score and source,
    so an operator can delete a wrong alias (`--remove a b`).
- **6.6 Gate.** `alias` and `chain` kinds as targets; everything else per §4.

### Tests

- Alias edge creation above τ and none below it; kinds must be compatible.
- A seed expands to its class.
- The zero-hop CTE (alias of a hop-1 key is hop 1).
- User isolation of vectors and aliases.
- Forget cleanup.
- `--remove` sticks: a removed pair is never re-created by a later pass (store it
  as `source = "rejected"`).
- `[graph].aliases = false` → fingerprint identical.

### Definition of Done

- [ ] `just check-native` passes.
- [ ] τ per model recorded with its calibration table.
- [ ] Gate holds. `recall audit` on the field host's copy reviewed by hand.

---

## 14. Phase 7: Temporal cues (exploratory)

**Goal.** "What did we use *before*?" prefers the superseded or past fact (G4, the
one top-1 the Phase 2 change can lose).

**Branch:** `feat/temporal-recall`

### Tasks

- **7.1 Cue detector.** A small, closed, documented list (`before`, `previously`,
  `used to`, `originally`, `formerly`, `in the past`, `prior to`, `earlier`),
  matched on word boundaries against the query. No model call.
- **7.2 Two candidate mechanisms, measured separately** on the `temporal` kind:
  - **(a) Closed edges.** The graph leg also walks edges whose `invalid_at` is set
    (today's `as_of` machinery, with "include closed" in place of a point in time),
    and orders memories asserting closed edges first.
  - **(b) Recency neutralised.** For cued queries, replace the recency multiplier by
    1.0, so age stops favouring the newest fact and affinity (Phase 2) decides.
- **7.3** Ship the mechanism that passes the gate, behind `[retrieval].temporal_cues`
  (default per measurement). If neither passes, record that and stop. The case stays
  a documented limitation.

### Definition of Done

- [ ] Both mechanisms measured, numbers in §17, decision recorded.

---

## 15. Phase 8: Docs, defaults, release

**Branch:** `docs/recall-quality-release`

- **8.1 Defaults.**
  - Each knob's default is set from its phase's measurement, in a **separate
    commit per knob** (revertable alone: the Task 7.3.6 / reranking 8.3 pattern).
  - `[rerank].mode` stays `off`, and the docs say why with the real-store numbers.
- **8.2 Docs.**
  - `README.md` (recall description, accuracy claims with real-store evidence).
  - `docs/architecture.md` ("Recall: three legs" gains affinity, expansion slots,
    probes, aliases).
  - `docs/configuration.md` (every new key, its neutral value, how to measure it on
    your store with `recall audit`).
  - `docs/evaluation.md` (age-aware eval, `complete@k`, `recall audit`).
  - `docs/performance.md` (bench with probes).
  - `docs/mcp.md` if Phase 3.3's `via` reaches MCP text.
- **8.3 Upgrade notes.**
  - Who must `reindex` (Phase 4 document prompts).
  - What runs automatically on first start (keyword index v3 rebuild).
  - What to run optionally (`probes backfill`, `graph aliases --rebuild`) and their
    cost.
  - Back up the data directory first.
- **8.4 Release.**
  - Version bump in the `chore(release)` style.
  - `just check-native`, `just sdk-test`, `just docker-build`.
  - Smoke test: the release image answers recall with the new defaults.
  - Field host verified over the wire. Notes hand-written, as for v0.4.1–0.4.3.
- **8.5 Close-out.** A task entry in `implementation-plan.md` pointing here, and this
  file's header status updated to "shipped" with the phases that shipped on and off.

### Definition of Done

- [ ] All checks pass. The release smoke test is recorded in §17.
- [ ] Docs reviewed against the final config keys (`unrecognised_keys` reports
      nothing on the shipped example).

---

## 16. Risks and follow-ups

| Risk | Mitigation |
|---|---|
| Age-aware eval moves every baseline number | Expected and recorded (0.8). All later phases compare against the new baseline only. |
| Affinity lets relevance override real rank evidence | Bounded band (δ ≤ 0.5), arithmetic pinned, smallest δ on the plateau chosen. |
| Expansion slot surfaces an irrelevant neighbour at #2 | Only when the query names a known entity, never at #1, only memories that passed filters and the floor; `recall audit` hand review. |
| Probe noise passes the floor | Separate calibrated probe baseline (5.4); negatives in the gate. |
| Probe generation cost / no LLM configured | Off by default; budgeted backfill; client-supplied questions for `:direct`. |
| Wrong aliases merge distinct concepts | Kind compatibility, calibrated τ, optional LLM confirm, `--list/--remove`, rejected pairs remembered. |
| Prompt-table rows go stale as model families evolve | Each row cites its model card. Explicit config override. Unknown models get no prompt and an info log. |
| Eval corpus still small (104 memories) | Real-store `recall audit` is part of every gate (§4). |

**Follow-ups (not in this plan):**
- Stemming in the keyword leg (`tests`/`testing`; reranking-plan §11).
- Query-embedding LRU cache for repeated agent queries.
- An approximate-NN index beyond ~100k memories (`docs/performance.md`).
- Topic/cluster nodes for abstract `nl-category` queries, if Phase 5 leaves them
  failing.
- Personalized PageRank over the graph, only if Phases 3 + 6 leave `chain`
  incomplete. Its seeding gain did not materialise here (§1.3) because RRF outvotes
  one-leg answers.

---

## 17. Results log

Fill in as phases complete. Paste raw command output, not summaries.

### Environment
- Eval embedder: `bge-small-en-v1.5` (default config = field host's embedder as of 2026-10-17).
- Field host config summary: _TBD_
- Hardware for latency: _TBD_

### Pre-plan measurements (2026-10-17)
See §1.3 and [recall-accuracy-proposal.md](recall-accuracy-proposal.md) §2. They were
measured on the same-age eval and are superseded by Phase 0's re-run.

### Phase 0 — eval fidelity
- Per-kind movement from ages + dedupe: _TBD_
- Determinism check (two runs identical): _TBD_
- `recall audit` self-comparison on the field host's copy: _TBD_
- §1.3 variants re-run on the age-aware eval: _TBD_

### Phase 1 — reranker off, bounded deadline work
- Field host p50/p95 before → after `mode = off`: _TBD_
- `queued_ms` vs `inference_ms` under 3 concurrent recalls; R1 hypothesis: _TBD_

### Phase 2 — relevance affinity
- δ grid (eval + audit): _TBD_ · chosen δ and rationale: _TBD_

### Phase 3 — graph expansion
- slots × δ × rank_weight grid: _TBD_ · `complete@5` relational/chain: _TBD_ · hand review: _TBD_

### Phase 4 — embedding prompts
- Per-model with/without tables + `calibrate`: _TBD_

### Phase 5 — question probes
- Eval with/without; bench p95; ingest token cost; field audit: _TBD_

### Phase 6 — aliases
- τ calibration per model; gate; field audit: _TBD_

### Phase 7 — temporal cues
- Mechanism (a) vs (b): _TBD_ · decision: _TBD_

### Phase 8 — release
- Smoke test; final defaults per knob: _TBD_

---

## 18. `VERIFY` items

- [ ] Phase 1.2: the new busy error variant (or marker) changes no HTTP status mapping.
- [ ] Phase 4.1: each prompt-table row against its model card (EmbeddingGemma,
      nomic-embed-text v1/v1.5, e5, Qwen3-Embedding, mxbai-embed-large,
      snowflake-arctic-embed).
- [ ] Phase 4.1: the model-name forms Ollama, LM Studio and llama.cpp report for
      those models (what `model_id()` actually contains).
- [ ] Phase 5.2: `vec0` `PARTITION KEY` plus a non-key metadata column
      (`memory_id`) on the bundled sqlite-vec version.
- [ ] Phase 6.1: `vec0` composite primary keys, or the synthetic-id fallback.
- [ ] Phase 6.2: alias threshold τ per model on this corpus (HippoRAG 2's 0.8 is a
      starting point, not a value).

## Sources

- BGE v1.5 query instruction — [model card](https://huggingface.co/BAAI/bge-small-en-v1.5)
- EmbeddingGemma prompts — [model card](https://ai.google.dev/gemma/docs/embeddinggemma/model_card)
- Fusion functions (convex combination vs RRF) — [Bruch, Gai, Ingber](https://arxiv.org/abs/2210.11934)
- HippoRAG 2 (query-to-triple linking, synonym edges at 0.8, ablations) — [arXiv 2502.14802](https://arxiv.org/html/2502.14802v2)
- Hypothetical Prompt Embeddings — [arXiv 2607.29402](https://arxiv.org/pdf/2607.29402)
- Graphiti hybrid search and graph-distance reranking — [Zep docs](https://help.getzep.com/graphiti/working-with-data/searching)
