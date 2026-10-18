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

- [x] `just check-native` passes.
- [x] Two consecutive `eval` runs produce identical recall/p@1/MRR/complete@k —
      at the shipped default, yes (seven runs in seven processes, identical to the
      decimal). At other recency settings it does not always hold, and §17 records
      the cause and the noise floor rather than hiding it.
- [x] New baseline committed; §17 shows the per-kind movement and explains it.
- [ ] `recall audit` self-comparison on a copy of the field host's store recorded
      in §17 (0 changes). **Not met.** The tool and its purity/refusal/JSON tests
      are in place; the field half needs a copy of the operator's store and a query
      file only they can write. Blocked on an agreed window, not on code.
- [x] The four experiment variants of §1.3 re-run on the age-aware eval through
      `scripts/recall-grid.sh` with the experiment patch. Their numbers go in §17 as
      the inputs to Phases 2–3. (§1.3's four, plus query-to-triple seeding without
      a threshold, so G2's abstention collapse was confirmed on the aged corpus too.)

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

- [x] `just check-native` and `just sdk-test` pass.
- [ ] Field host on `mode = "off"`; p50/p95 before/after recorded in §17.
      **Not met.** This needs the deployed machine: the flip is a config edit there
      and the numbers have to come from its traffic. The command that produces both
      columns in one run is in §17. Blocked on access, not on code.
- [ ] Concurrency measurement (1.5) recorded, with the R1 hypothesis marked
      confirmed or refuted. **Not met**, same reason — but it is a measurement now
      rather than an inference: three concurrent searches with `RUST_LOG=debug`
      report `queued_ms` and `inference_ms` for every leg, and §17 records which
      shape confirms R1 and which refutes it.
- [x] `eval` with `mode = off` identical to Phase 0's baseline. Every column
      `change: -0.0`, which is what `mode = off` being the default is for.

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
  1.2% of a score. *(First written as "≈ 1.3% against one day of age ≈ 0.77%",
  which compares against the raw half-life: the recency multiplier is
  `MULTIPLIER_FLOOR`-bounded, so a day is 0.15% of a score and three days 0.46%.
  The pinned exchange-rate test therefore uses three days — at one day both
  candidate δ values let relevance win and the test proves nothing. §17, Phase 2.)*
  The grid decides how much weight relevance deserves.
- Bounded by construction: affinity ∈ `[1 − δ, 1]`. *(This bullet originally claimed
  that with δ ≤ 0.5 affinity cannot reverse a large rank gap — "rank 1 vs rank 10 on
  one leg ≈ 13% apart". It can. δ is a spread, and the maximum relevance gap, 0
  against 1, reverses a gap of r ranks once δ > 1 − 61/(60 + r): 0.047 at rank 4,
  0.129 at rank 10, 0.238 at rank 20 — δ = 0.5 reverses anything nearer than ~62
  ranks. So the bound is not what keeps relevance from becoming the primary key;
  the grid is, which is why §17 ships 0.05 rather than the top of the range.)* The
  module docs carry the table (invariant 3.7).

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
- Same tie with an age gap: at δ = 0.2 relevance wins for a 0.06 gap; at δ = 0.05
  recency wins. Both pinned, which documents the trade. *(Measured at a three-day
  gap: with the floored recency band, a one-day gap lets relevance win at both δ
  values and the test proves nothing.)*
- Bound: how far relevance can overrule rank evidence — a nine-rank gap survives the
  maximum relevance gap at δ = 0.05 and 0.10, and does not at 0.20 and 0.50, which
  brackets the documented reversal threshold of 0.129. *(Written as "at δ = 0.5 a
  memory 5 ranks lower never overtakes, whatever its relevance", which the formula
  contradicts — at δ = 0.5 it does. §17 Phase 2 carries the arithmetic and the test
  that replaced it, `how_far_relevance_can_overrule_a_rank_gap`.)*
- Graph-only hit (relevance = `GRAPH_HOP_RELEVANCE`) under δ > 0: penalised exactly
  as the formula says. Phase 3's slot is unaffected (structural).
- NaN relevance cannot panic the sort.
- Rerank `Rrf` weight 0 with δ > 0 → order unchanged by the rerank leg.

### Definition of Done

- [x] `just check-native` passes — 997 tests (17 new, the 11 `--ignored` ONNX ones
      still hand-run), clippy `-D warnings` clean, boundaries clean. `eval` green and
      `sdk-test` skipped: nothing on the wire changed, only a CLI report section and
      a config key.
- [ ] Grid table (eval and `recall audit`) pasted in §17. Chosen δ and rationale in
      the doc comments. **Half met.** The eval grid, its gated re-read, the chosen
      δ = 0.05 and the reasoning are in §17, in `DEFAULT_RELEVANCE_WEIGHT`, the
      config field, the example file and `docs/configuration.md`. The `recall audit`
      half is not met and is not approximated: it needs a copy of the deployed store
      plus the operator's `expect` file. §17 records the command and what to read
      off it.
- [x] §4 gate holds at δ = 0.05: p@1 +3.8 (needs ≥ +2), MRR +2.7, recall@5 ±0, no
      `by_kind` regression, abstention 100.0. Caveat on one row — the +5 ms latency
      limit is measured at eval scale (+0.5 ms) and only *argued* at 20k: `scripts/
      bench.sh 20000` was not re-run. Recorded in §17 rather than waved through.
- [x] Baseline re-recorded at the chosen default: p@1 50.0 → 53.85, MRR 63.9 →
      66.63, recall@5/complete@5/every other kind identical, and `just eval-native`
      reads `change: -0.0` four times.

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
| Affinity lets relevance override real rank evidence | The band's *bound* is not the protection — δ = 0.5 reverses any gap nearer than ~62 ranks (§17, Phase 2), so the protection is the chosen value: δ = 0.05, whose maximum relevance gap stops between rank 4 and rank 5. Pinned arithmetically (`how_far_relevance_can_overrule_a_rank_gap`), and `0.0` remains a different code path for anyone who wants none of it. |
| Phase 2's default was chosen on a corpus that favours relevance by construction | True of the instrument, and the reason the shipped value is the smallest that clears §4 rather than the largest that scores. The larger δ values (0.2–0.5) are recorded as measured-but-unshipped, and the falsifier — `recall audit` with the operator's `expect` file — is spelled out in §17 as owed. If it holds there, raising the default is a release note, not a config edit. |
| `scripts/recall-grid.sh`'s absolute columns include the informational cases since Task 0.7, so they no longer match the gated baseline | Phase 0's grid table (83.3 reference) and Phase 2's (75.8) are not the same measurement, and a reader who assumes they are will mis-read both. Phase 2's entry records both readings and the `-- --baseline … --max-drop 100` invocation that produces the gated one; the script itself is a follow-up below. |
| Expansion slot surfaces an irrelevant neighbour at #2 | Only when the query names a known entity, never at #1, only memories that passed filters and the floor; `recall audit` hand review. |
| Probe noise passes the floor | Separate calibrated probe baseline (5.4); negatives in the gate. |
| Probe generation cost / no LLM configured | Off by default; budgeted backfill; client-supplied questions for `:direct`. |
| Wrong aliases merge distinct concepts | Kind compatibility, calibrated τ, optional LLM confirm, `--list/--remove`, rejected pairs remembered. |
| Prompt-table rows go stale as model families evolve | Each row cites its model card. Explicit config override. Unknown models get no prompt and an info log. |
| Eval corpus still small (110 memories) | Real-store `recall audit` is part of every gate (§4) — and that half is still unmet: see Phase 0's Task 0.9. |
| Two `eval` runs need not agree | Measured floor: 0.7 recall points, 1.7 if a whole single-expectation case flips. Equal-BM25 documents (~38 of them match "project" through the category field) reorder with Tantivy segment layout. Below ~2 recall / ~1 p@1 a Δ is one case moving, not a result; re-run twice before gating on it. Root cause and candidate fixes in §17 Phase 0 and the follow-up below. |
| A busy rerank stage looks like a slow one on the wire | Both stay `skipped`/`deadline`, deliberately (invariant 3.5: shipped SDKs parse two reason values, so a third is a breaking change). The distinction lives on the domain trace, in the `warn!` line beside `queued_ms`/`inference_ms`, and in `rerank audit`'s `skipped_busy`. `a_busy_skip_says_exactly_what_a_slow_one_says` pins the transport half so nobody "just adds a field" later. |
| Phase 1's field halves (1.1, 1.5) went unmeasured | Code and measurement path shipped; §17 records which two numbers are owed, the command that produces each, and the shape that would confirm or refute R1. R1 is left open rather than called confirmed by construction. |

**Follow-ups (not in this plan):**
- Stemming in the keyword leg (`tests`/`testing`; reranking-plan §11).
- Query-embedding LRU cache for repeated agent queries.
- An approximate-NN index beyond ~100k memories (`docs/performance.md`).
- Topic/cluster nodes for abstract `nl-category` queries, if Phase 5 leaves them
  failing.
- Personalized PageRank over the graph, only if Phases 3 + 6 leave `chain`
  incomplete. Its seeding gain did not materialise here (§1.3) because RRF outvotes
  one-leg answers.
- Make the eval bit-reproducible, so "two runs identical" can be a promise instead
  of a measurement. The vector leg (`ORDER BY distance`) and the score sequence are
  already identical across processes; the leak is that tied documents are handed to
  the keyword collector in segment-layout order, and the eval's store is one commit
  per memory with a merge thread racing. Three fixes, each a decision rather than a
  patch: order the keyword leg by `(score, id)` (canonical, right for production —
  but a rebuilt store gets fresh UUIDv7 ids, so eval runs still differ); tie-break
  on content (stable across rebuilds, changes production's arbitrary order); or let
  the eval supply `MemoryId`s to the saver (deterministic, and a security-shaped API
  change). Until one is taken, treat the noise floor as part of every gate.
- `rerank audit --concurrency N`: replay the query file with N recalls in flight, so
  the queued/inference split and `skipped_busy` stop being structurally zero and
  Phase 1.5's R1 question becomes one command on a real host instead of a
  `journalctl` grep. Deliberately not built in Phase 1 — the command's contract is
  "one query at a time, no deadline" because it exists to measure what the model
  *costs*, and giving it concurrency changes which question it answers. That is a
  design decision, not a flag.
- `RerankTrace::Applied::complete`: the one field an `applied_partial` counter would
  need, and no reader for it until the audit above exists. A partial leg is visible
  today as `judged` below the requested depth, on the wire and in the trace.
- `scripts/recall-grid.sh` should render the gated reading by default. Its awk takes
  the bare summary line, which since Task 0.7 marks 21 cases `*` informational is not
  the subset `eval/baseline.json` gates on — so a row that reads "recall 75.8" beside
  Phase 0's "83.3" is a change in instrument, not a regression. Passing
  `-- --baseline eval/baseline.json --max-drop 100` already puts the gated
  `baseline … now: …` lines in each cell's output; parsing them and printing a
  gated column next to the wide one is a day's care, and it removes a trap for every
  later phase's grid.
- Port or delete `ai-specs/experiments/recall-accuracy-2026-10-17.patch`. It no
  longer applies (`git apply --check` fails at `memory_recaller.rs:427` — it predates
  Phase 1's deadline plumbing and Phase 2's affinity stage), which makes it a trap
  for anyone reaching for the `RA_EXP_FUSE=tiebreak` reference row Phase 2's Task 2.6
  asked for. Either re-cut it against the current recaller or drop it and let §1.3's
  recorded numbers stand alone; a tracked patch nobody can apply is worse than either.
- `scopes_are_carried_from_the_key_that_authenticated` is load-sensitive: one 401 in
  a full-suite run under CPU contention, 5/5 isolated, suite green every other time.
  Worth making deterministic before it is mistaken for product fallout — same class
  of measurement error Phase 1's own timing tests had, in a test that guards
  isolation, which is the wrong place to learn that lesson under load.

---

## 17. Results log

Fill in as phases complete. Paste raw command output, not summaries.

### Environment
- Eval embedder: `bge-small-en-v1.5` (default config = field host's embedder as of 2026-10-17).
- Field host config summary: _TBD_ — blocked with the rest of Task 0.9, with
  Phase 1's 1.1 and 1.5, and with Phase 2's `recall audit` half, all for the same
  reason: the host is the operator's.
- Rerank models: the container's `/models` volume holds all four shipped rerankers
  (4.6 GB), so the `--ignored` ONNX tests run here without a download. No CI job runs
  ignored tests, so they are run by hand when the files under them change.
- Hardware for latency: every number in this log is from the dev container
  (`docker/Dockerfile.dev`) on an arm64 Docker VM, not the field host. The p50/p95
  columns are comparable to each other, not to production.

### Pre-plan measurements (2026-10-17)
See §1.3 and [recall-accuracy-proposal.md](recall-accuracy-proposal.md) §2. They were
measured on the same-age eval and are superseded by Phase 0's re-run.

### Phase 0 — eval fidelity

Everything below was measured in the dev container (`docker/Dockerfile.dev`,
arm64 Docker VM) on `bge-small-en-v1.5` with no reranker. There is no
`eval/config.toml` in this repo: the eval runs on library defaults plus whatever
env a cell sets, so a cell neutralises recency with
`RECUERDOS_AI_RETRIEVAL__RECENCY_HALF_LIFE_DAYS` rather than with a file — and
`validate()` refuses `recency_half_life_days = 0` as a typo guard, so "no decay"
is expressed as a century (36500), not zero.

#### The re-recorded baseline (Task 0.8)

`eval --write-baseline eval/baseline.json`, 59 gated cases (80 with the
informational kinds Phase 0.7 added):

| metric | before | after |
|---|---|---|
| recall@5 | 83.0 | **83.3** |
| precision@1 | 65.4 | **50.0** |
| MRR | 74.3 | **63.9** |
| complete@5 | _not recorded_ | **80.8** |
| abstention | 100.0 | 100.0 |
| latency p50 / p95 | 7.6 / 9.6 ms | 7.9 / 9.0 ms |

Per kind (`n` = cases of that kind in the gated set; recall is *what* came
back, p@1 is *which one came first*):

| kind | n | recall → | p@1 → | complete@5 |
|---|---|---|---|---|
| category-filter | 2 | 100.0 → 100.0 | 100.0 → 100.0 | 100.0 |
| current-fact | 1 | 100.0 → 100.0 | 100.0 → **0.0** | 100.0 |
| exact-token | 4 | 100.0 → 100.0 | 100.0 → 100.0 | 100.0 |
| near-miss | 7 | 100.0 → 100.0 | 100.0 → **42.9** | 100.0 |
| needle | 3 | 66.7 → 66.7 | 66.7 → **33.3** | 66.7 |
| negative | 7 | 100.0 → 100.0 | 100.0 → 100.0 | 100.0 |
| nl-category | 8 | 64.6 → 66.7 | 62.5 → **37.5** | **50.0** |
| nl-question | 7 | 85.7 → 85.7 | 42.9 → **57.1** | 85.7 |
| paraphrase | 4 | 100.0 → 100.0 | 50.0 → **100.0** | 100.0 |
| relational | 7 | 57.1 → 57.1 | 28.6 → **0.0** | 57.1 |
| statement | 4 | 75.0 → 75.0 | 50.0 → **25.0** | 75.0 |
| subcategory-filter | 4 | 100.0 → 100.0 | 100.0 → 100.0 | 100.0 |
| tag-filter | 1 | 100.0 → 100.0 | 0.0 → 0.0 | 100.0 |

**What moved, and why only one column moved.** Recall@5 is within a third of a
point of where it was, and 12 of 13 kinds are identical: ageing the corpus does
not change *which* memories are relevant enough to come back. What it changes
is the head of the list. With one shared age every memory in a query's
neighbourhood was multiplied by the same recency factor, so the head was
decided by "which sentence looks most like the question" — and it was decided
*consistently*, which is what made p@1 look healthy at 65.4. Give the corpus
real ages in `[0, 365)` and the multiplier separates those same candidates:
p@1 50.0, MRR 63.9. This is the correction §1.3 predicted ("the +3.8 p@1 above
is an upper bound measured under a simplification"), measured from the other
side: the old number was the artefact.

The two kinds that got *better* say the same thing in reverse — `paraphrase`
50.0 → 100.0 and `nl-question` 42.9 → 57.1 are cases whose right answer happens
to be young and whose distractor is old. A p@1 that can be won or lost by an
age roll is not a measurement of ranking, and it is now honest about that.
`current-fact` (n = 1) flipping 100.0 → 0.0 is one case, not a trend, and the
corpus does not actually pin its premise: that memory declares no entities, so
`validate()`'s "expected fact must be the newest about its subject" rule skips
it, and hash-aged neighbours can lawfully be younger than its 12 days. Phase 7
owns that case (G4).

**The two changes are separable, and they pull opposite ways.** Measured
between the Phase 0 commits, in this container:

| corpus state | recall@5 | p@1 | MRR |
|---|---|---|---|
| control (pre-Phase-0, same-age, 4 duplicates) | 83.0 | 65.4 | 74.6 |
| dedupe only, still same-age | 83.0 | 69.2 | 76.2 |
| ages only, duplicates still in | 81.4 | 50.0 | 64.1 |
| ages + dedupe (what is committed) | 83.3 | 50.0 | 63.9 |

Deleting the four duplicated memories is worth +3.8 p@1 on its own — V1 was
taking real top-5 slots. The ages are then worth −19.2, and they are the
finding, not the regression.

#### Determinism, and its measured limit (Task 0.1, and the DoD check)

Seven `eval` runs in seven separate processes at the shipped default:
recall 83.3, p@1 50.0, MRR 63.9, complete@5 80.8 — **identical in all seven**,
which is what the DoD asks for.

The DoD's premise needed a stress test, so the same experiment was run at other
recency settings, and it does not always hold:

| cell (three runs each, one process per run) | recall@5 | p@1 | MRR |
|---|---|---|---|
| `recency_half_life_days = 30` | 84.0 / 83.3 / 83.3 | 46.2 / 46.2 / 46.2 | 63.2 flat |
| `recency_half_life_days = 36500` | 83.0 / 83.0 / 83.0 | 69.2 flat | 76.5 / 76.5 / 76.2 |

One case moves: `nl-category: setting up my machine` (3 expectations), whose
third answer — "The devcontainer runs the daemon on port 7070…" — is either in
the top 5 or not. **Noise floor: 0.7 recall points, or 1.7 if a whole
single-expectation case flips** (1/59 of the mean).

Root cause, measured rather than assumed. Instrumenting each leg for the flaky
query across processes found the vector leg's `ORDER BY distance` list
**bit-identical**, the keyword leg's segment count and full score sequence
**identical**, and the graph leg **silent** (`seeds=0`). What differed was which
*memory* carried which keyword score, and therefore its `bm25_rank`: the query
("getting my laptop set up for this project") matches ~38 documents through the
`category` field alone — every `fact.project` memory contains "project" — and
those documents tie on BM25. Tantivy hands tied documents to the collector in
segment-layout order, and the eval's store is built by one commit per memory
(110 of them) with a merge thread racing, so which document sits in which
segment varies per process. `sort_ranked`'s documented promise ("ties must not
depend on the order rows came back from SQLite") holds one layer above the leak.

Why this is recorded instead of fixed here: every fix is a design decision the
plan has not made. Ordering the keyword leg by `(score, id)` makes the leg
canonical and is right for production — but a *rebuilt* store gets fresh UUIDv7
ids, so it would still vary between eval runs; content-derived tie-breaks reach
the same result and change production's arbitrary order; and giving the eval
deterministic ids means letting a caller supply a `MemoryId` to the saver, which
is a security-shaped API decision. Follow-up in §16. What Phase 0 needs, and now
has, is a number: read any Δ below ~2 recall points, or below ~1 point of p@1
and MRR, as one case moving, not as a result — and re-run twice if a gate
decision lands there.

#### `[graph].enabled` was not measurable, and now is (found by the grid)

The first `recall-grid.sh` run printed a `graph-off` row identical to the
reference on every metric. `seed()` contained
`config.graph.enabled = true`, from when the eval existed to *decide* whether
the graph shipped (Task 7.3.6). The override beat the env var, so the harness
was reporting its own choice as a result. Removed (`cce9b5d`); with the shipped
default on, nothing else changed. The honest row:

`graph-off`: recall −3.8, complete@5 −3.9, MRR −1.3 — and **relational recall
57.1 → 28.6 (−28.5)**, which is the number the override was hiding, next to
`chain`'s hole (complete@5 50.0) as Phase 3's starting position.

#### The knob grid (Task 0.6)

`scripts/recall-grid.sh` — one `eval` process per cell, so each row is a config
a deployment could boot (see the script header for why this is not an in-process
grid like `eval --grid`). Default cells, one run each:

```
cell           recall    p@1 complete    MRR │   p50/p95  abst │ Δrecall  Δp@1 Δcomp ΔMRR
reference        83.3   50.0     80.8   63.9 │  8.1/ 9.4 100.0 │   reference
graph-off        79.5   50.0     76.9   62.6 │  8.0/ 9.1 100.0 │     -3.8   +0.0   -3.9  -1.3
graph-rrf        85.3   50.0     82.7   64.6 │  8.2/ 9.7 100.0 │     +2.0   +0.0   +1.9  +0.7
graph-quiet      81.4   50.0     78.8   63.0 │  8.4/ 9.7 100.0 │     -1.9   +0.0   -2.0  -0.9
recency-100y     83.0   69.2     78.8   76.5 │  8.3/ 9.8 100.0 │     -0.3  +19.2   -2.0 +12.6
recency-30d      83.3   46.2     80.8   63.2 │  8.1/ 9.7 100.0 │     +0.0   -3.8   +0.0  -0.7
```

Read three things off it, all of them inputs to Phase 1–3:

1. **Recency is the most expensive knob in the pipeline.** Turn it off (a century
   half-life) and p@1 goes 50.0 → 69.2 and MRR 63.9 → 76.5 while recall gives
   back 0.3. Every point of that 19 is age deciding the head instead of
   relevance — the same 19 the re-recorded baseline lost, seen from the other
   switch. G1/F2's "does recency belong in the rank score at all" question now
   has a size on the age-aware corpus.
2. **The graph weight tuned in Task 7.3.7 is not the best value on an aged
   corpus.** The hop wants *more* weight now, not less: 1.0 (raw three-leg RRF)
   is +2.0 recall / +1.9 complete@5 over the shipped 0.3, and 0.05 is −1.9. The
   per-kind detail is entirely `relational` (+14.3 up, −14.2 down) with no other
   kind moving. Phase 3's grid should search above 0.3, not below it.
3. **Graph-off costs completeness, not just recall**: complete@5 −3.9, on
   `relational` alone.

#### The four new kinds (Task 0.7), frozen

```
alias       recall 80.0   p@1 20.0    complete@5 80.0
chain       recall 75.0   p@1 100.0   complete@5 50.0
temporal    recall 80.0   p@1 0.0     complete@5 80.0
vocab-gap   recall 14.3   p@1 0.0     complete@5 14.3
```

* `chain` (Phase 3) — the pure `complete@5` case: p@1 is 100.0, so the right
  thing is always first, and half the two-hop answers still arrive without their
  bridge. V3 in one table row.
* `temporal` (Phase 7) — finds the superseded fact four times in five and puts
  it first **never**. The recency multiplier is the only component in the
  pipeline with an opinion about time, and it always votes for the newer
  memory. G4 measured.
* `alias` (Phase 6) — recall 80.0 says the vector leg mostly papers over the
  name mismatch; p@1 20.0 says it does not win with it. `alias: k8s` misses
  outright: "k8s" is not in the Kubernetes decision's embedding neighbourhood.
* `vocab-gap` (Phase 5) — six of seven answers never reach the top five. This is
  the worst number in the file and it is the phase-5 target; §0.9's freeze is
  what stops it being improved by generating probes from these queries.

`validate()` now refuses a `vocab-gap` case whose expectation shares a content
word with its query (content, tags and category — everywhere the keyword leg
looks), which is what makes the 14.3 mean what it says.

#### `recall audit` on a real store (Task 0.9)

**Not run — blocked, and it needs the operator, not the code.** The tool is in
place (`recall audit --config A --compare-config B --queries F [--json]`, with
`A = B` recording a self-comparison) and its purity, refusal and JSON-leak
tests pass in the harness, but the field half of the task needs two things only
the operator can supply: a copy of the deployed store, and a query file with
`expect` substrings written by the person whose life it describes (§0.9 keeps
that file out of the repo). Both the self-comparison (must show 0 rank changes)
and the per-query ranks stay _TBD_ until a window is agreed. Until then §4's
"real-store `recall audit` is part of every gate" is unsatisfied by
measurement, not by tooling.

#### Deviations from the task text (all deliberate, none silent)

- **0.2 — "a `SettableClock`".** Reused the existing `FixedClock`, giving it
  `set()`. Two clocks that both pretend to be Tuesday is one too many ways to be
  wrong.
- **0.2 — hash-derived ages.** FNV-1a over the content bytes folded into
  **seconds** in `[0, 365)`, not a seeded RNG: a `StdRng` is stable only while the
  number and order of draws are, so adding one memory anywhere in the file re-ages
  every memory after it. A collision in seconds would be an exact score tie, which
  is the thing this task exists to remove.
- **0.3 — "delete the duplicates".** Deleted the four *plain* copies and kept the
  ones inside the subcategory sections: the `subcategory-filter` cases need a
  memory that carries a subcategory, and the plain twin carried nothing the
  survivor does not. `validate()` now refuses a content defined twice, so V1
  cannot come back either way.
- **0.6 — "`recency_half_life_days = 0` in `eval/config.toml`".** There is no
  `eval/config.toml` in this repo — the eval runs on library defaults plus env —
  and `validate()` refuses `0` as a typo guard, so the no-decay cell is `36500`.
  `rerank.mode` is a legal but **opt-in** cell: `mode = on` loads 1.1 GB of
  cross-encoder, the same reason `rerank-grid.sh` keeps the big models opt-in.
- **0.5 — "`recall audit` compares two configs".** Implemented, plus two things
  the design needed: the refusal is keyed on `embeddings.model` and
  `data_dir`/`data_path` (read from the daemon's own `status` when one is
  reachable), because a "different store" that differs only in its rerank knob is
  the comparison the tool exists to make; and there is no per-side `--depth` —
  `--depth` overrides both sides, since a depth difference is a different question
  than the one being audited.
- **0.5 — not in the plan.** `seed()`'s `config.graph.enabled = true` override
  removed. See the grid section above: it made a knob the grid was built to
  measure unmeasurable.
- **0.7 — `informational` cases.** `validate()` refuses a `vocab-gap` case whose
  expectation shares a content word with its query, and the parse test now refuses
  an `informational` flag on any kind outside the four Task 0.7 owns. Phase 2.5's
  "nothing is muted" promise survives as "nothing is muted without a named owner".
- **0.9 — real-store run.** Blocked on the operator's store copy and query file;
  recorded above rather than approximated with a synthetic store, which would
  measure the harness.

#### §1.3's experiment variants, re-run on the age-aware eval (DoD)

`ai-specs/experiments/recall-accuracy-2026-10-17.patch` applied (uncommitted,
reverted after the run — it is scaffolding, not code), then
`scripts/recall-grid.sh --no-defaults` with one cell per variant. **Overall
columns read over all 80 cases** (the grid parses the eval's own report, which
includes the informational kinds); the per-kind Δ's are unaffected by that, and
the control row's gated equivalent is the committed baseline.

```
cell           recall    p@1 complete    MRR │   p50/p95  abst │ Δrecall  Δp@1 Δcomp ΔMRR
control          75.8   42.5     72.6   56.5 │  8.6/10.6 100.0 │   reference
tiebreak         75.8   42.5     72.6   56.5 │  8.3/ 9.9 100.0 │     +0.0   +0.0   +0.0  +0.0
slot             78.5   42.5     75.3   57.4 │  8.1/ 9.7 100.0 │     +2.7   +0.0   +2.7  +0.9
tiebreak-slot    78.5   42.5     75.3   57.4 │  8.5/10.0 100.0 │     +2.7   +0.0   +2.7  +0.9
bge-instruct     69.2   42.5     65.8   55.3 │ 11.3/13.7  28.6 │     -6.6   +0.0   -6.8  -1.2
context-full     78.1   41.1     75.3   55.9 │  8.3/ 9.7  85.7 │     +2.3   -1.4   +2.7  -0.6
triples          75.8   39.7     72.6   54.9 │ 31.4/34.8   0.0 │     +0.0   -2.8   +0.0  -1.6
```

**The relevance tie-break now does nothing at all.** `RA_EXP_FUSE=tiebreak` moved
*no metric and no case*: run against this same control in one grid, its full
report was identical cell for cell — same 80 cases, same hits, same top-hit
marks — with only p95 differing, by 0.2 ms. 100.0 abstention, every Δ +0.0. §1.3's +3.8 p@1 for this variant was earned entirely on exact score ties,
and exact ties were a product of the corpus being seeded in one millisecond.
Give the memories real ages and the ties separate, so a rule that only fires on a
tie has nothing to fire on. That is the plan's correction confirmed from the
other side, and it is the strongest argument for Phase 2 being a **bounded
relevance multiplier that acts while ages differ** rather than a tie-break:
implementing §1.3's variant as measured would ship a knob that changes nothing in
production.

**The graph expansion slot's gain is real and structural**, as the same note
predicted: +2.7 recall, +2.7 complete@5, +0.9 MRR, p@1 untouched (which is the
point — the promoted neighbour takes slot #2, not #1), all of it landing on
`relational` (+28.6, two of its seven cases) and nowhere else. Layering the
tie-break on top adds precisely nothing. Phase 3 starts from here, and §4's
"relational + chain complete@5 up ≥ 15 points" gate has a mechanism attached to it
now.

**The two embedding-prompt variants keep their shape and swap their verdicts.**
The bge instruction is worse than ever — −6.6 recall, and abstention collapses
100.0 → 28.6, because the instruction text drags unrelated memories over the
floor — and it is the only cell that costs latency on the query itself
(8.6 → 11.3 ms p50). §1.3's ✗ becomes a measured ✗ with a reason. Prepending
labels and entities (`context-full`), which §1.3 recorded as a recall *loss*
(83.0 → 81.7), now gains +2.3 recall and +2.7 complete@5 while still costing a
negative (abstention 85.7) and 1.4 points of p@1. Where it helps is the
informative part: **`alias` +20.0 and `vocab-gap` +14.3** — it improves exactly
the two kinds that exist to measure vocabulary mismatch, which is what Phase 4
and Phase 5 are being asked to do without wrecking the negatives.

**Query-to-triple seeding without a threshold is still the hazard it was.**
Abstention 100.0 → **0.0**: every negative now returns something, p@1 −2.8, and
p95 34.8 ms because the experiment embeds every live relation per query. G2
reproduced on the aged corpus, so Phase 3's guard (and Phase 6's constraint) stay
mandatory, not precautionary.

### Phase 1 — reranker off, bounded deadline work
- Field host p50/p95 before → after `mode = off`: _TBD — blocked, see "Blocked on
  the field host" below_
- `queued_ms` vs `inference_ms` under 3 concurrent recalls; R1 hypothesis: _TBD —
  blocked; the measurement is now one command, see below_

The code half was measured in the dev container (`docker/Dockerfile.dev`, arm64
Docker VM). The field half was **not** measured — see "Blocked on the field host"
below; it is recorded as owed rather than estimated.

Nothing in this phase could move the eval, and it did not: `[rerank].mode`
defaults to `off`, so the stage that all five commits change is never loaded by a
gated case.

| | |
|---|---|
| commits | `9784a40` timed session acquire · `b34d6a3` chunked scoring with partial application · `9d32971` the busy/partial split · `8e98ec5` `queued_ms` vs `inference_ms` · `ee7d482` docs |
| `just check-native` | clean — `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `check-boundaries.sh` ("no violations found"), **980 tests** (14 added here, 0 removed), 11 ignored |
| `eval` at `mode = off` | recall@5 83.3 · p@1 50.0 · MRR 63.9 · complete@5 80.8 · abstention 100.0 — every column `change: -0.0` against Phase 0's baseline, which is the DoD requirement and not a result |
| `just sdk-test` | 36 + 14 passed. The suite is the point of 1.4: nothing in it had to change, because nothing on the wire did |
| `cargo test -- --ignored rerank` | 3 passed on the real model — `jina-reranker-v1-turbo-en`, the 147 MB one the other two ONNX tests already use — including the chunk-vs-whole-head invariance test above |

#### 1.2 — waiting for a session is bounded

`InstancePool` had one path to a session: `instances[0].lock()`, which parks. A
recall whose rerank deadline had already expired stayed parked, then scored a head
nobody was waiting for any more, then released the session for the next caller to
park behind — the work reranking-plan §12 (part three) measured as 34 → 120 ms per
pair and could not attribute.

It hands out tokens now: `max_concurrency` of them, each indexing one session.
`acquire_until(deadline)` takes one and returns `Err(budget_left)` when none is
free by the deadline, **having entered no session**; the guard returns the token in
`drop`, so a panic mid-scoring does not retire a session. `Reranker::score_before`
is the domain's version of that (`Err(RaError::RerankBusy)` — a variant, not a
string to match on, which is the thing `RaError` exists to avoid), and the adapter
overrides it while every other implementation keeps the default: score the head,
never report busy. The recall path turns busy into `skipped`/`deadline` before a
handler sees it, so the transport gained a mapping that adds nothing observable —
pinned by `a_busy_rerank_reaching_the_transport_looks_like_any_internal_error`,
alongside the existing table test that no other variant's mapping moved.

#### 1.3 — the head is scored in runs

`Reranker::preferred_chunk()` (defaulted to `None` = "the head as given"; the
adapter answers `[rerank].batch_size`) lets `rerank_scores` score a run at a time:
the worker sends each run's scores as they land and checks an `Arc<AtomicBool>`
*before starting* a run — ONNX cannot be pre-empted mid-batch, so the chunk is the
grain a deadline can cut at, and what a missed deadline abandons is one batch
instead of ten candidates nobody reads.

What finished before the deadline is **applied**. Those ranks are real evidence
about those candidates, and `apply_rerank` already treated a leg covering part of
the head as an ordering for that part; `RerankTrace::Applied { judged }` already
carried the number, so the wire gained nothing. Two is the floor, because one score
cannot reorder anything: a leg that judged one candidate is `skipped`, not an
`applied` that changed nothing.

A *model* error is deliberately not treated that way. Applying the prefix of a leg
that failed would answer `applied` while the model is broken, which hides exactly
what the operator needs to see, and `model` is the reason they can act on. Deadline
and busy apply the prefix; model does not.

Chunking is a yield point, not a change in what is judged. `align_rerank` was split
into `align_positions` (per run) and `order_best_first` (once, stable, so the tie
semantics are the ones Phase 4 shipped), which is what makes a chunked leg and a
whole-head leg produce the same list. Pinned twice: the doubles' chunked and
unchunked legs agree candidate for candidate, and
`scoring_the_head_in_runs_returns_the_same_scores_as_one_call` compares one
whole-head call against three runs of `batch_size = 2` on the real model
(`--ignored`: nothing in CI runs ignored tests — the `fastembed_reranker` header's
"nightly CI" predates the workflows in the repo — so this was run by hand, against
the model cached in the container's `/models` volume, and it passed).

#### 1.4 — nothing moved on the wire, and the split has somewhere to live

`skipped`/`deadline` for a leg cut off before two candidates, `applied` with
`judged` below the requested depth for one cut off after: both already in the
shipped SDKs. `busy` is a field on the **domain** trace (`RerankTrace::Skipped`)
and `RerankResponse` drops it on the way out, which is pinned rather than implicit —
`a_busy_skip_says_exactly_what_a_slow_one_says` asserts a busy skip serialises
byte-identically to a slow one and that the object's keys stay exactly the four a
client parses. Invariant 3.5, from this side.

Three readers use it: `recall audit`/`rerank audit` count `skipped_busy` and label
a busy stage `busy`/`skipped-busy` instead of "ran out of time"; the eval's "the
stage never ran" warning names a session shortage rather than calling every skip a
model failure; and the shadow log records `judged` beside `depth`, so a comparison
that only looked at part of the head reads as one.

#### 1.5 — queue and inference stop being one number

`score_before` answers with `RerankScored { scores, queued, inferred }`. The
adapter measures the queue where the session is handed out — the only place that
can say — and derives inference as the call minus the queue, so the two halves sum
to what the caller experienced instead of drifting apart. A model with no pool
keeps the default, which reports the whole call as inference and never invents a
zero for a wait it could not observe.

Where an operator reads it:

| Surface | What it says |
|---|---|
| `debug` (`name = rerank_leg`) | every leg: `judged`, `depth`, `complete`, `rerank_ms`, `queued_ms`, `inference_ms` |
| `warn` (skipped leg) | as above, plus `busy` |
| `warn` (partial apply, 1.3) | as above |
| `info` (`rerank_shadow`) | as above — shadow mode is the "tell me about the stage" mode |
| `rerank audit` | `Call.queued_ms`/`inferred_ms`, percentiles in the JSON, one human line |

The audit's split is `None` today, by construction: the command replays one query
at a time with no deadline (that is deliberate — it exists to measure what the
model costs), so nothing there ever waits. It prints "not measured" rather than
two zeros, because a healthy 0 ms and an unmeasured queue are different facts. A
busy refusal *is* metered: 250 ms of waiting to judge nothing is the most expensive
thing this stage can do to a recall, and leaving it out would make a host that
refuses every call look cheaper than one that answers slowly.

#### Tests added (14)

| test | pins |
|---|---|
| `a_saturated_pool_refuses_a_caller_at_its_deadline` | refusal at the budget, and the refusing caller never entered a session |
| `a_session_that_frees_in_time_is_used_rather_than_refused` | waiting is honoured *and* reported: the waiter's `queued` is the holder's remaining hold |
| `a_panicking_score_gives_its_session_back` | the token outlives a panic; the pool does not shrink |
| `a_stage_that_cannot_get_a_session_is_skipped_as_a_deadline` | busy is `skipped`/`deadline` to the client, `busy` in the trace, and judged nothing |
| `a_second_recall_does_not_queue_behind_a_busy_session` | the concurrency case: one acquisition, one head judged — the exact form of "it did not queue" |
| `a_head_finished_part_way_is_applied_and_the_tail_keeps_its_order` | a partial leg reorders the judged prefix and leaves the tail as fusion ordered it |
| `a_leg_that_gives_up_stops_at_the_next_chunk_boundary` | at most one run of work is spent after the caller walks away |
| `one_judged_candidate_is_a_skip_rather_than_an_applied_of_one` | the floor of two, so the trace never claims a reorder that cannot exist |
| `runs_are_a_yield_point_and_not_a_change_to_the_ranking` | chunked and unchunked legs are identical candidate for candidate |
| `scoring_the_head_in_runs_returns_the_same_scores_as_one_call` | the same claim on the real model (`--ignored`) |
| `a_busy_skip_says_exactly_what_a_slow_one_says` | invariant 3.5 from the transport side: same JSON, same four keys |
| `a_busy_rerank_reaching_the_transport_looks_like_any_internal_error` | busy at the transport adds nothing a client can observe |
| `a_refused_session_is_not_reported_as_having_run_out_of_time` | the audit's vocabulary: `busy` and `deadline` are different rows |
| `a_metered_deadline_call_keeps_queue_and_inference_apart` | the split survives into the audit's samples, and the deadline-free path says `None` |

The five timing-sensitive ones among them read what the leg *did* rather than what
a clock said it should have done by now: `judged` comes from the trace and the
expectations are built from it, a counter owned by a detached thread is waited for
(`waits_for`), and "nobody queued" is an acquisition count rather than an elapsed
time. Written the other way — "two 40 ms runs fit a 90 ms budget", and asserting a
refusal the instant `execute_traced` returns — they passed alone and failed the
first time the suite ran with the other five test binaries competing for the CPU.
That is a measurement error in the test, and it is now the kind that cannot come
back quietly.

#### Blocked on the field host

Two halves of this phase cannot be done from a checkout, and neither is
approximated here. Both need the deployed machine, which is the operator's:

- **1.1 — `mode = "off"`, and the p50/p95 before and after.** Still owed, and the
  expected direction is the plan's (105–568 ms → ~10–25 ms, ~1.5 GiB RSS freed).
  Two ways to get the number, both from the server's own timing rather than a
  client's clock: sample `took_ms` from the response over N searches of a query the
  operator really asks, once on the current config and once after the flip; or run
  `recall audit --config on.toml --compare-config off.toml --queries F`, which
  prints a "recall latency: … vs …" line for both columns in one pass and writes
  nothing. `took_ms` is a response field, not a log line, so there is no third way
  through `journalctl`.
- **1.5 — three concurrent recalls, and what `queued_ms` says.** The R1 hypothesis
  ("cost per pair grew 34 → 120 ms because abandoned work accumulated") has an
  answer now, and it is one command rather than an inference. Run the daemon with
  `RUST_LOG=debug`, fire three searches at once, and read the stage's lines:

  | What to read | Under 1.2–1.3 it should say | If instead it says |
  |---|---|---|
  | `rerank_leg` (debug, one per leg that judged something) | `queued_ms` ≈ 0, `inference_ms` ≈ a full head — this is the caller that got the session | a large `queued_ms`: sessions are fewer than the recall wants |
  | `rerank leg failed …` (warn) × 2 | `busy=true`, `queued_ms` ≈ the whole budget, `inference_ms=0` — refused, having scored nothing | `inference_ms` above 0: the stage is still doing work nobody waits for, so R1's mechanism survives and this phase is wrong about it |

  Then set `max_concurrency = 2` and fire three again: the third caller's
  `queued_ms` should be roughly one head, which is the queue itself rather than the
  refusal — the number the sizing recipe in `docs/configuration.md` multiplies.
  Whether *this* is what turned 34 ms into 120 ms on the field host can only be
  answered on the field host, so R1 stays neither confirmed nor refuted until
  someone reads these lines there.

The corresponding DoD boxes ("field host on `mode = off`", "concurrency measurement
recorded") are left unticked for the same reason Task 0.9's is: the code and the
measurement path exist, the measurement does not.

#### Deviations from the task text (all deliberate, none silent)

- **1.2 — "token channel used as a semaphore".** An `mpsc::Receiver` is not `Sync`,
  so the channel version of a semaphore has to put the receiver behind a mutex, and
  then every waiter's timeout lives under one lock: two callers whose deadlines
  expire together contend on the mutex rather than on the token, and the code has a
  lock-ordering question where it should have none. It is a `VecDeque` of free
  indices under a `Mutex` with a `Condvar` (`wait_timeout_while`) — same semantics,
  tokens index sessions, `Session::drop` returns the token including on a panic, and
  sessions stay `Mutex<T>` so exclusivity is typed twice rather than once.
- **1.2 — `score_before` returns `RerankScored`, not `Vec<f32>`.** 1.5 asks for the
  queue and the inference apart, and only the pool can see the queue: the alternative
  was to change this signature twice in one phase. The default implementation still
  does exactly what the task's version did — call `score`, never report busy — and
  reports the whole call as inference, because for a model with no pool that *is*
  the true answer.
- **1.3 — "sends `(offset, scores)`".** Sends scores only. One worker scores the
  runs in order over an ordered channel, so the receiver's own counter is the
  offset; carrying a number that is already implied is a second source of truth to
  disagree with the first.
- **1.3 — partial application does not extend to model errors.** The task says the
  prefix is applied at a deadline, and it is; a *model* failure mid-leg still
  reports `skipped`/`model`. Applying a third of a broken model's head would turn
  the one reason an operator can act on into an `applied` that looks healthy.
- **1.4 — the counters exist, and are zero here by construction.** A replay of one
  query at a time with no deadline cannot produce a busy row, and `skipped_busy`
  reports 0 for it. It is counted rather than assumed away, in the words the
  existing `skipped_deadline` row already uses, because the mapping is where the
  misreading would live: a report that calls a session shortage "ran out of time"
  sends the operator to `timeout_ms`, the one setting that cannot help. What the
  task's "counters" did *not* get is a `partial` counter: a partial leg is already
  `judged < depth` on the wire, in the trace, and in `rerank_candidates_low`, so a
  counter for it would need a new `Applied::complete` field with no reader until
  someone builds a deadline-aware audit. Recorded in §16 instead.
- **1.5 — the audit's split is `None` today.** Same reasoning: deadline-free replay,
  nothing to wait for. It prints "not measured" rather than two zeros, and a busy
  refusal — the one call that could report a queue — is metered as a call, since
  leaving it out would make a refusing host look cheaper than a slow one.
- **Tests: no clock is trusted with an assertion.** How many candidates were judged
  is read back from the trace and the expectation is built from that; the counters a
  detached scoring thread owns are waited for; the concurrency test gates on the
  first caller actually holding the session and proves "it did not queue" with an
  acquisition count. The first version of these tests assumed intervals
  ("two 40 ms runs fit a 90 ms budget") and asserted a refusal the instant the
  request returned; it passed alone and failed with five other test binaries on the
  same four cores.
- **1.6 — five more pages than the task named.** `docs/deployment.md`, `docs/api.md`,
  `docs/sdk-python.md`, `docs/evaluation.md` and the example config all stated the
  behaviour this phase removed — "the head is scored in one call", "the stage is
  dropped and the pre-rerank order is served", a `depth = 3` sizing recipe that is
  now unnecessary. Leaving them would have made the phase's own docs change a
  contradiction.

#### Note for whoever next runs the whole suite

`tests/identity_isolation.rs::scopes_are_carried_from_the_key_that_authenticated`
failed once, in one full-suite run of this phase, with the daemon answering 401 for
a profile the test had just written. It passed 5/5 isolated and the full suite
passed on every other run here, including the final `check-native`. It is
load-sensitive and unrelated to anything in this phase (no code path touched here is
in its stack); recorded so the next failure is not read as Phase 1 fallout.

### Phase 2 — relevance affinity

**Shipped default: `[retrieval].relevance_weight = 0.05`.** Affinity is a third
bounded multiplier on the fused score, `score ×= 1 − δ·(1 − relevance)`, applied
between the legs' fusion and the floor: it can change what comes first among what
was returned and cannot change what was returned.

#### The anchor first (δ = 0)

Before the default moved, the knob at zero was checked to be nothing at all:
`eval --baseline` reported `change: -0.0` on all four gated metrics against the
Phase 1 baseline (83.3 / 50.0 / 63.9 / 80.8), and the full suite passed with **not
one pre-existing test edited** — the 980 carried through, 13 new ones beside them
at that moment, the phase closing at 997 with 17. That is the "knob at zero, no
change" rule kept by code path rather than by arithmetic: `apply_relevance` returns
before reading the list.

#### The δ grid (Task 2.6)

`scripts/recall-grid.sh`, one `eval` process per cell, δ swept through
`RECUERDOS_AI_RETRIEVAL__RELEVANCE_WEIGHT`. Two readings per row, because the
script's columns are not what `eval/baseline.json` gates on: the script renders the
*bare* summary — every case, including the 21 marked `*` informational at Task 0.7 —
while the baseline gates on 52. Phase 0's grid prints 83.3 in the same column
because those 21 cases did not exist yet, so the two tables are not comparable.
The gated reading below comes from passing the gate through the grid itself
(`-- --baseline eval/baseline.json --max-drop 100`).

Gated, against the Phase 1 baseline — the subset the §4 gate is written for:

| δ | recall@5 | precision@1 | MRR | complete@5 | abstention |
|---|---|---|---|---|---|
| **0** — the recorded baseline | 83.3 | 50.0 | 63.9 | 80.8 | 100.0 |
| **0.05 — shipped** | 83.3 | **53.8** (+3.8) | **66.6** (+2.7) | 80.8 | 100.0 |
| 0.1 | 83.3 | 53.8 (+3.8) | 66.6 (+2.7) | 80.8 | 100.0 |
| 0.2 | 83.3 | 59.6 (+9.6) | 69.8 (+5.9) | 80.8 | 100.0 |
| 0.3 | 84.0 (+0.6) | 63.5 (+13.5) | 72.2 (+8.3) | 80.8 | 100.0 |
| 0.5 | 84.0 (+0.6) | 67.3 (+17.3) | 74.3 (+10.4) | 80.8 | 100.0 |

`eval --baseline` prints one line per regressed kind and printed none at any δ, so
the gate's "no kind drops by > 5" row holds across the whole curve rather than
merely not being violated on average.

The script's own table (all 80 cases; the p50/p95 column is the latency reading).
The grid was run twice and the four accuracy columns came out identical both
times; the latency columns moved ±0.6 ms, which is the box and not the knob:

```
cell           recall    p@1 complete    MRR │   p50/p95  abst │ Δrecall  Δp@1 Δcomp ΔMRR
────────────────────────────────────────────────────────────────────────────────────────────
delta-0          75.8   42.5     72.6   56.5 │  8.5/10.2 100.0 │   reference
delta-005        75.8   45.2     72.6   58.7 │  8.7/10.7 100.0 │     +0.0   +2.7   +0.0  +2.2
delta-01         77.2   45.2     74.0   59.2 │  8.7/11.0 100.0 │     +1.4   +2.7   +1.4  +2.7
delta-02         77.2   50.7     74.0   62.2 │  9.0/10.4 100.0 │     +1.4   +8.2   +1.4  +5.7
delta-03         77.6   54.8     74.0   64.9 │  9.0/10.7 100.0 │     +1.8  +12.3   +1.4  +8.4
delta-05         77.6   57.5     74.0   66.6 │  9.0/11.0 100.0 │     +1.8  +15.0   +1.4 +10.1
```

**Which cases.** At δ = 0.05 exactly two gated cases gained their top hit and none
lost one — both of them the sentence this phase was opened for, "a newer, weaker
memory sitting on the one that answered better":

```
near-miss: cursor pagination why      · → ✓
nl-category: how reviews move         · → ✓
```

δ = 0.2 adds `statement: many exports in one file`, `statement: nervous about
shipping`, `nl-category: setting up my machine`, and two informational `alias`
cases. Everything the curve buys is movement *within* the head: complete@5 never
moves, because affinity cannot surface a memory the legs never returned.

**Why 0.05 and not 0.2.** Task 2.6 asks for "the smallest δ on the gain plateau",
and there is no plateau to point at: p@1 climbs monotonically to the ceiling of the
validated range (+3.8, +3.8, +9.6, +13.5, +17.3) with nothing regressing anywhere.
So the instruction is applied as its conservative reading — the smallest δ that
clears the §4 gate — and the choice is recorded rather than dressed up. The bigger
values were measured and left unshipped for three reasons:

1. **The eval favours relevance partly by construction.** Its expected memory is
   the topically nearest one, so a curve driven by a relevance multiplier is
   rewarded by the corpus's own shape. A monotone line is weak evidence about a δ
   large enough to matter, and the instrument that could falsify it — `recall
   audit` over a real store with the operator's `expect` file — is the half of this
   phase that is blocked.
2. **At 0.2 relevance outranks the indexes rather than voting among them.** δ is a
   spread; 0.2 is a 20% spread against the 14.8% that separates rank 1 from rank 10
   on a leg, so the maximum relevance gap overrules that disagreement. That is a
   different pipeline from the one the project documents, and a different promise to
   every deployment already running.
3. **The evidence only covers near-ties.** Two gated cases at 0.05, five at 0.2, all
   of them the same shape. Nothing here says anything about the behaviour of a
   relevance-dominated ranking on a messy real store.

**§4 gate at δ = 0.05.** p@1 +3.8 (needs ≥ +2) ✓ · MRR +2.7, no drop ✓ · recall@5
±0 (needs ≤ 1) ✓ · no `by_kind` regression printed ✓ · abstention 100.0 ✓ ·
latency: eval p95 10.2 → 10.7 ms between the grid's own δ = 0 and δ = 0.05 cells,
inside the +5 ms row; **`scripts/bench.sh 20000` was not re-run** (see "Not
measured").

**Baseline re-recorded** (DoD, at the shipped default). `eval/baseline.json` moves
p@1 50.0 → 53.85 and MRR 63.9 → 66.63, `near-miss` p@1 42.9 → 57.1 and
`nl-category` p@1 37.5 → 50.0; recall@5, complete@5 and every other kind identical,
and `just eval-native` now reads `change: -0.0` four times. Its latency fields also
moved (p50 7.92 → 8.55, p95 9.03 → 10.36), which is *more* than the knob accounts
for: the grid's δ = 0 cell measured p95 10.2 on this machine that day, so ≈1.2 ms
of that is the box. `compare()` does not gate on latency, so this is a note for
whoever reads the file next rather than a bar quietly raised.

#### Tests added (17)

| test | pins |
|---|---|
| `a_nan_score_cannot_break_the_ordering` | the comparator is total: every row survives, real scores keep their order, the NaN arrives last — and does so whatever the input order |
| `zero_weight_leaves_the_fused_order_and_scores_bit_identical` | δ = 0 leaves ids, score bits and relevance bits alone, with relevance deliberately pointed against the tie-breaks |
| `affinity_spans_exactly_the_band_the_module_docs_promise` | `[1 − δ, 1]` at three δ values over out-of-range relevance too, the two ends exact, monotone between |
| `a_nan_weight_switches_affinity_off_rather_than_poisoning_every_score` | a δ that escaped config validation changes nothing instead of multiplying the whole list into NaN |
| `an_exact_symmetric_tie_goes_to_the_more_relevant_memory` | v1/k2 against v2/k1 with equal ages: the better match wins, in both assignments, so it is relevance and not the id tie-break |
| `relevance_and_age_are_worth_this_much_against_each_other` | the exchange rate both ways — Δr = 0.06 beats three days at δ = 0.2 and loses them at 0.05 |
| `how_far_relevance_can_overrule_a_rank_gap` | a nine-rank gap survives the maximum relevance gap at δ = 0.05/0.10 and not at 0.20/0.50, bracketing the documented 0.129 |
| `a_graph_only_hit_is_penalised_exactly_as_the_formula_says` | a hop's estimated relevance is taxed by `δ·(1 − 0.35)` like any other, before Phase 3 gives it a slot |
| `a_nan_relevance_costs_the_row_its_number_not_the_request` | NaN relevance → NaN score → that row last, and the request answered |
| `the_rerank_leg_is_added_inside_affinity_not_on_top_of_it` | the two stages commute: δ-then-rerank and rerank-then-δ give the same numbers when they disagree about which memory wins |
| `a_zero_weight_rerank_leg_leaves_an_affinity_ordered_list_alone` | "turn the reranker off" and "turn relevance off" stay separate escapes |
| `relevance_weight_defaults_to_the_value_the_ranker_shares` | the shipped default is the grid's 0.05, and config and ranker read one constant so the report, the field docs and the ranker cannot drift |
| `relevance_weight_is_read_from_file_and_env` | the file form, and the env form the grid drives itself through |
| `relevance_weight_outside_the_range_is_rejected` | 0.51, 2.0 and −0.1 are refusals with the key named, not clamp-worthy typos |
| `a_nan_relevance_weight_is_refused_rather_than_read_as_off` | TOML's `nan` does not become a config line that reads as a number and behaves as nothing |
| `every_off_spelling_of_the_relevance_weight_runs_the_same_recall` | 0.0, −0.1 and NaN produce the identical list bit for bit through the whole pipeline |
| `a_relevance_weight_discounts_every_result_by_exactly_its_relevance` | through the pipeline: every score is the fused one times `1 − δ·(1 − relevance)`, relevance untouched, no row added or dropped, order by the discounted number — at the shipped default as well as 0.2 |

#### Deviations from the task text (all deliberate, none silent)

- **2.2 — the relevance key does not go in the canonical comparator.** The task
  adds relevance as `sort_ranked`'s second key; done there it would also re-decide
  the exact ties that exist *today*, at δ = 0, in a corpus whose exact ties are its
  own noise floor — which breaks this phase's anchor test on the way in. The
  relevance key therefore lives in `apply_relevance`'s own comparator
  (`sort_ranked_with_relevance`), which only runs when δ > 0. The canonical
  comparator did get the part of 2.2 that is a pure safety fix: `f32::total_cmp`
  instead of `partial_cmp(…).unwrap_or(Equal)`, wrapped so a NaN sorts *last*
  (a total order puts a positive NaN's bit pattern above `+infinity`, so compared
  bare, the score that could not be computed would win the list).
- **2.6 — the "bound" test as written is arithmetically false, so it is not
  pinned.** "At δ = 0.5, a memory 5 ranks lower on both legs never overtakes,
  whatever its relevance" is the opposite of what the formula does: the maximum
  relevance gap (0 against 1) reverses an r-rank gap once δ > 1 − 61/(60 + r), which
  is 0.047 at rank 4, 0.129 at rank 10, 0.238 at rank 20 — and δ = 0.5 reverses
  anything nearer than ~62 ranks. So the bound is not the safety property and the
  pinned test is the honest one (`how_far_relevance_can_overrule_a_rank_gap`), with
  the module docs carrying the table and saying out loud that the top of the range
  is a measuring stick. The same error was in §9's design bullet ("with δ ≤ 0.5 it
  cannot reverse a large rank gap") and is corrected there.
- **The design note's exchange rate ignored the floored band.** "A relevance gap of
  0.06 is worth ≈1.3%, and one day of age ≈0.77%" reads recency's raw half-life; the
  multiplier is `MULTIPLIER_FLOOR`-bounded, so a day is 0.15% of a score and three
  days 0.46%. The pinned exchange-rate test therefore uses a **three-day** gap: at
  one day both δ values come out the same way and the test proves nothing. The
  module docs carry the corrected rate and the asymmetry it implies (for the
  day-apart memories a personal store competes with, δ does more per point than the
  rank gaps suggest).
- **2.1 — `apply_relevance` takes `&mut [ScoredMemory]`, not `&mut Vec<ScoredMemory>`.**
  It needs no `mem::take` (unlike `apply_rerank`'s `Replace` branch), and a slice is
  the widest thing the caller can hand it.
- **2.6 — no plateau, so no literal reading.** Recorded above under "Why 0.05 and
  not 0.2": the curve is monotone to the ceiling, and "smallest δ that clears the
  gate" is the interpretation taken.
- **2.6 — the `RA_EXP_FUSE=tiebreak` reference row could not be measured.**
  `ai-specs/experiments/recall-accuracy-2026-10-17.patch` no longer applies
  (`git apply --check` fails at `memory_recaller.rs:427`; it predates Phase 1's
  deadline plumbing and Phase 2's affinity stage). Porting it is a §16 follow-up.
  It would not change the choice: the pure-tiebreak variant sits between δ = 0 and
  δ = 0.05 by construction, and both are measured.

#### Not measured (blocked, and deliberately not approximated)

- **`recall audit` on a real store with the operator's `expect` file** — the second
  instrument in Task 2.6 and in every §4 gate. Same blocker as Task 0.9 and Phase
  1's field halves: it needs a copy of the deployed store, which is the operator's.
  The recipe, which writes nothing and works with the daemon running:

  ```bash
  # Put the knob inside [retrieval], not at the end of the file: a key that lands
  # under the last section in the file is the v0.4.1 orphaned-key bug, and the
  # audit would then compare the same config with itself.
  awk '{print} /^\[retrieval\]$/ {print "relevance_weight = 0.05"}' \
      /etc/recuerdos-ai/recuerdos-ai.toml > /tmp/affinity.toml
  # (no [retrieval] header in that file? then
  #  printf '[retrieval]\nrelevance_weight = 0.05\n' >> /tmp/affinity.toml)

  recuerdos-ai recall audit --config /tmp/affinity.toml \
      --compare-config /etc/recuerdos-ai/recuerdos-ai.toml \
      --queries /tmp/questions.txt --user <handle>
  ```

  What to read: **no query whose `expect` was at rank 1 loses it** (§4's last row),
  and how many queries gain rank 1. Repeat at 0.2 and 0.5 — if the eval's curve
  holds on real memories with real expectations, the case for a higher default
  becomes real, and it should be raised in a release note rather than silently.
- **`scripts/bench.sh 20000 500`** for the §4 latency row. The added work is one
  multiply per returned candidate plus a re-sort of a list already truncated to
  `limit`; the eval's own p95 moved +0.5 ms at the shipped δ. Recorded as unmeasured
  rather than reasoned away, and it is the only §4 row Phase 2 does not close.

#### Consequence for the phases after this one

Phase 3's graph slot now sits *after* affinity, so a hop inserted into the head
enters a list whose order relevance already had a vote on — but its relevance is
`GRAPH_HOP_RELEVANCE` (0.35), so at the shipped δ a hop is taxed 3.25% and the slot
remains the structural thing Phase 3 designs, not an arithmetic accident. Phase 4's
grids inherit the new baseline; anything that reads Phase 0's grid numbers off this
file should read the "two readings" note above first.

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
