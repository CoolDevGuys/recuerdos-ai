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

> **Status as of 2026-10-18 — do not start Phase 4.** Phases 0–3 are shipped (`v0.5.0-rc1`) and their
> owed measurements were taken on a deployed store. Phases 4, 6 and 7 are **stopped as written**: the
> recall failures they were designed around were not observed on that store, and the two queries that
> had been cited as their evidence were memories that were never stored. Phase 5 is the one remaining
> candidate — `vocab-gap` still reads 14.3% recall on the corpus — and it is the expensive one, so it
> goes to the frozen-copy corpus before it gets code. Phase 8 stays, as a small release. The knob the
> whole sequence was most sensitive to turned out to be δ, now shipped at **0.5**.
> Numbers, verdicts per phase, and what would reopen one:
> [`recall-quality-status.md`](recall-quality-status.md) and §17 below.

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

Status read 2026-10-18, after the field-host measurements in §17. A phase marked **stopped** is not
deferred work waiting for capacity: its target failure was looked for and not found, so starting it
would tune an instrument on a population it cannot observe. Reopening one needs the evidence named in
[`recall-quality-status.md`](recall-quality-status.md) §5.

| Phase | Deliverable | Branch | Size | Ships on its own? | Status |
|---|---|---|---|---|---|
| 0 | Age-aware, deduplicated eval; `complete@k`; new kinds; real-store `recall audit`; grid script | `chore/recall-eval-fidelity` | M | yes (tooling) | **done**, measured (§17); its Task 0.9 copy arrived 2026-10-18 |
| 1 | Reranker off on the field host; deadline work bounded and cancellable | `fix/rerank-deadline-work` | M | yes | **done**, measured — and it found the fusion problem (M5) |
| 2 | Relevance-aware ordering (`[retrieval].relevance_weight`) | `feat/relevance-affinity` | S | yes | **done**; the field read it timid, default raised to **0.5** |
| 3 | Graph expansion slot (`[graph].expansion_slots`) | `feat/graph-expansion` | S–M | yes | **done**; costs −2.2 MRR here, top-1 unharmed; hand review of the arrivals owed |
| 4 | Per-model embedding prompts, pin fingerprint | `fix/embedding-prompts` | M | yes | **stopped** — no second embedder measured anywhere |
| 5 | Ingest-time question probes | `feat/question-probes` | L | yes (off by default until gated) | **open, unfunded** — the only class still failing on the corpus (`vocab-gap` 14.3%) |
| 6 | Entity resolution (alias edges) | `feat/entity-aliases` | M–L | yes | **stopped** — no query on the store was hurt by a split concept |
| 7 | Temporal cues for "before / previously" questions | `feat/temporal-recall` | S–M | yes (exploratory) | **stopped** — corpus-side only (`temporal` p@1 0.0%); no real temporal question asked |
| 8 | Docs, defaults, release | `docs/recall-quality-release` | S | yes | **pending, small** — defaults now all come from measurements |

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
- [x] Field host on `mode = "off"`; p50/p95 before/after recorded in §17.
      **Met 2026-10-18** on the operator's host: p50 6.7 → 306.2 ms, p95 9.6 → 603.9 ms for the
      stage (same-config noise floor 299 → 302 ms), ~1.6 GiB held with the model loaded against
      a 1.5 GiB prediction. Table in §17 · Field host, where a second reading closed the
      refinement as well: a warm daemon with the stage off holds **198,996 kB** against
      1,668,708 kB with the model loaded, so the stage is ~1.4 GiB resident and ~300 ms of a
      recall that otherwise takes 6 ms.
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
- [x] Grid table (eval and `recall audit`) pasted in §17. Chosen δ and rationale in
      the doc comments. **Met 2026-10-18**, both halves. The eval grid, its gated re-read, the
      chosen δ = 0.05 and the reasoning are in §17, in `DEFAULT_RELEVANCE_WEIGHT`, the config
      field, the example file and `docs/configuration.md`. The `recall audit` half arrived with
      `v0.5.0-rc1` on the operator's host: δ 0 → 0.05 is **p@1 +5.6, MRR +3.5** on 18 real
      queries whose expectations the operator wrote, and the sweep went further than the gate
      asked — δ 0.5 reads **p@1 +27.8, MRR +19.9** over the shipped value. The shipped default
      is not refuted; it looks timid. Table and reading in §17 · Field host.
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

- [x] `just check-native` (and `just sdk-test` if 3.3 lands) passes. — clean; `sdk-test`
      not applicable, 3.3 did not land (deferred to Phase 8, §17's deviations).
- [x] Grid pasted in §17. Chosen `expansion_slots` default with rationale in doc comments.
      — 10 cells, default `1`, rationale in the field docs and the `config.rs` comment.
- [x] §4 gate holds, including `complete@5` for `relational`/`chain` up ≥ 15 points.
      — `relational` +28.6 on the mechanism. `chain` +20.0 **with an asterisk**: the four
      cases this phase inherited produce no graph seeds at all, so the movement is one new
      case the mechanism could reach. §17 says so rather than letting the number stand.
- [ ] `recall audit` on the field host's store: every changed result reviewed by hand.
      — **half met, 2026-10-18**: the audit ran on the deployed store (18 queries, all with
      expectations). The aggregate is in §17 · Field host — top-1 changed 0/18, so G1 held, and
      MRR −2.8 with `complete@5` flat at a store already 88.9% complete inside the window. The
      hand reading is not done, so this box stays open: 0.5 reciprocal-rank units of cost — three
      expected memories pushed 2 → 3, or one pushed out of the window, which the per-query rank
      lines will settle — and only a person can say whether the rows that arrived were
      related-but-useless or vacuous. `⚑` marks nothing here (no top-1 moved); the marker to read
      is `(from N)` in the `after` column.

---

## 11. Phase 4: Per-model embedding prompts

**Goal.** Every embedding model is called the way its model card says it was
trained (E1). A deployment on Ollama or LM Studio models stops silently losing
retrieval quality.

**Branch:** `fix/embedding-prompts`

> **Status 2026-10-18: stopped as written.** The defect is real in principle and unobservable in
> practice: every measurement this project has taken — the corpus and the field host — runs
> `bge-small-en-v1.5`, so no run has ever shown a prompt-less model losing a query. Phase 3 left the
> mechanism in place (`calibrate`, the per-model baseline map) and that is where the value is: a
> deployment on another embedder gets a warning rather than silence. Reopening this phase needs a
> second embedder actually deployed and measured, not another read of the model cards. See
> [`recall-quality-status.md`](recall-quality-status.md) §5.

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

> **Status 2026-10-18: open, and the only later phase with a failing measurement to answer to.** On the
> corpus today `vocab-gap` reads **recall 14.3%, p@1 0.0%** — one of six cases reaches the answer, the
> others do not, which is the class this phase exists for and nothing else touches. What it does not
> have is a *field* target: the 18 real questions contain no vocabulary-gap question, so nobody has
> shown a person losing a memory this way. Given the cost (an LLM at write time, per memory, forever),
> the order is the corpus-copy work first — a real query set with gap-shaped questions in it — and the
> code only if the gap survives contact with a store nobody wrote for the test. See
> [`recall-quality-status.md`](recall-quality-status.md) §5 and §6 item 2.

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

> **Status 2026-10-18: stopped as written.** G3 — the fragmented concept neighbourhood this would
> merge — was looked for on a real store and not found: 280 entities and 61 relations over 182 active
> memories, and none of the 18 owner-written queries lost an answer to a split concept. The reserved
> seat (Phase 3) costs a little on that same graph, which makes *adding* edges to it a change with a
> price and no observed buyer. Reopening needs a query that fails because "Postgres" and "PostgreSQL"
> are two nodes — a shape the field query file does not contain. See
> [`recall-quality-status.md`](recall-quality-status.md) §5.

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

> **Status 2026-10-18: stopped as written, with the weakness kept on the record.** The corpus does show
> the shape — `temporal` reads **p@1 0.0%** over its five cases, and that number is not a scoring
> artifact — but it is a synthetic shape: those cases were authored to be temporally ambiguous, and the
> field host asked no question of that kind, so nothing has been observed to fix. One gap worth naming
> rather than smoothing over: the gated comparison at δ = 0.5 shows no kind losing precision@1, but the
> `temporal` kind is marked informational and sits **outside** that comparison, so this phase's worry is
> unmeasured at the new δ rather than measured and dismissed. Reopening it needs a real "what did we use
> before?" question against a store with superseded rows — the frozen copy has 43 of them, so the
> question is answerable and has not been answered.
> See [`recall-quality-status.md`](recall-quality-status.md) §5.

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

> **Status 2026-10-18: pending, and smaller than written.** The defaults this phase existed to set are
> now set from measurements rather than from a grid's judgement: δ is 0.5 (#40), `[rerank].mode` stays
> `off` with the real-store numbers behind it, and `[graph].expansion_slots` stays 1 with its price
> recorded. What is left is the release itself plus one decision the field work opened and could not
> close — whether the rerank stage should be fused by `replace` or gated on the model's confidence. That
> decision belongs to the frozen-copy corpus, not to this phase; if it is still open when the release is
> cut, ship the defaults above and say so in the notes.
> See [`recall-quality-status.md`](recall-quality-status.md) §4 and §6 item 1.

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
| Phase 2's default was chosen on a corpus that favours relevance by construction | Read on the field host 2026-10-18, and the worry pointed the wrong way: on 18 real queries with owner-authored expectations, δ 0 → 0.05 gained p@1 +5.6 / MRR +3.5, and δ 0.5 over the shipped value gains **+27.8 / +19.9**. The eval understated the gain. So the shipped value is not dangerous, it is timid — and the thing that would argue against a large δ is not "relevance overrides rank evidence" but *supersession*: two near-identical memories where only recency separates them, and the older one is stale. Raising the default is a Phase 8 decision with that check attached, not a quiet edit. §17 · Field host has the table. |
| `scripts/recall-grid.sh`'s absolute columns include the informational cases since Task 0.7, so they no longer match the gated baseline | Phase 0's grid table (83.3 reference) and Phase 2's (75.8) are not the same measurement, and a reader who assumes they are will mis-read both. Phase 2's entry records both readings and the `-- --baseline … --max-drop 100` invocation that produces the gated one; the script itself is a follow-up below. |
| Expansion slot surfaces an irrelevant neighbour at #2 | Only when the query names a known entity, never at #1, only memories that passed filters and the floor. Measured on the field host 2026-10-18: **top-1 changed 0/18** (the "never at #1" property held on real memories), and the cost that did land — MRR −2.8 at `complete@5` flat — is 0.5 reciprocal-rank units, which with no top-1 change means either three queries pushed 2 → 3 or one pushed 2 → out of the window; the per-query `expect …: rank` lines say which, and reading them is the owed review. `complete@5` was already 88.9% with the slot off, so this store has almost no recall headroom for the mechanism to earn against: it paid and did not gain. The hand reading of those arrivals decides whether the right conclusion is "wrong for stores whose recall is already good" or "wrong generally". |
| Probe noise passes the floor | Separate calibrated probe baseline (5.4); negatives in the gate. |
| Probe generation cost / no LLM configured | Off by default; budgeted backfill; client-supplied questions for `:direct`. |
| Wrong aliases merge distinct concepts | Kind compatibility, calibrated τ, optional LLM confirm, `--list/--remove`, rejected pairs remembered. |
| Prompt-table rows go stale as model families evolve | Each row cites its model card. Explicit config override. Unknown models get no prompt and an info log. |
| Eval corpus still small (110 memories) | The §4 "and on a real store" half now exists — 18 owner-authored queries run by `scripts/field-measure.sh` on the deployed store, first reading 2026-10-18 in §17 — and it read the same direction with a much larger effect (δ, where the field reading was ~1.6× the corpus's at the same value). It is still 18 queries on one person's store, so treat the corpus as the gate and the field host as the falsifier, never the reverse. **The falsifier has now fired**, on the rerank fusion rather than on δ, and at a size (44 points) that the corpus cannot absorb by assertion: Task 0.9's copy stops being bookkeeping and becomes the deciding instrument, which means a *frozen* copy with its digest recorded, queried by a file pinned to it. |
| Two `eval` runs need not agree | Measured floor: 0.7 recall points, 1.7 if a whole single-expectation case flips. Equal-BM25 documents (~38 of them match "project" through the category field) reorder with Tantivy segment layout. Below ~2 recall / ~1 p@1 a Δ is one case moving, not a result; re-run twice before gating on it. Root cause and candidate fixes in §17 Phase 0 and the follow-up below. |
| A busy rerank stage looks like a slow one on the wire | Both stay `skipped`/`deadline`, deliberately (invariant 3.5: shipped SDKs parse two reason values, so a third is a breaking change). The distinction lives on the domain trace, in the `warn!` line beside `queued_ms`/`inference_ms`, and in `rerank audit`'s `skipped_busy`. `a_busy_skip_says_exactly_what_a_slow_one_says` pins the transport half so nobody "just adds a field" later. |
| Phase 1's field halves: 1.1 measured, 1.5 still open | 1.1 read on the field host 2026-10-18 — the stage costs p50 6.7 → 306.2 ms and holds 1.6 GiB, for −0.2 MRR and nothing else across 18 real queries; the operator runs `mode = "off"` now, which was the plan's own default anyway. 1.5 has become harder rather than easier: with the stage off on the only field host, the queue has nothing to queue, so either it is measured in the few minutes with `mode = "on"` or R1 stays neither confirmed nor refuted and §17 says so. |
| Phase 3's default is measured on a kind that could not see it | The eval's four inherited `chain` cases produce no seeds, so `chain`'s +20.0 is one case the mechanism could reach, not the kind working. §17 states it, `chain_cases_paraphrase_their_entities_so_no_hop_can_start` pins which cases are reachable, and the DoD's `chain` tick carries the asterisk. The hand review on a real store is the half that could still refute it, and it is owed. |
| `just check-native` in the dev container can be green against a stale build | Measured in Phase 3: a green run clippy'd `recuerdos-ai v0.3.1` while the manifest said `0.4.3`, `cargo fmt --check` said nothing about an unformatted file, and `cargo test` ran a unit-test binary 117 tests behind the tree. Until the recipe is fixed, touch the sources first and check the run against `cargo test -- --list` (1 027 declared = 1 016 passed + 11 ignored on this branch); confirm the log names the tests you just wrote. `test result: ok` alone is not evidence. |

| The remaining phases target recall failures this store may not have | **Checked, and the store has none.** The field host is 16 of 18 complete inside five before Phase 3's mechanism runs, and its two failures are not mis-ranking: quoting both expectations back at the store returns top hits at relevance 0.09 and 0.10, which is what an empty region looks like — the memories were never stored. So on this store the recall job is finished (16 of 16 answerable complete inside five), abstention on the other two is correct behaviour, and Phases 4, 6 and 7 address a failure with no field-observed instance. What remains is ordering: δ, and one fusion that declines a correct judgement. |
| The field host is not a fixed instrument | The same shipped config read 38.9 p@1 where it had read 50.0, because six memories about this measurement work had been stored in the meantime and now outrank the older ones the `expect` substrings name. The system behaved; the ground truth went stale. Any repeated field measurement must carry a store digest, and Task 0.9's copy must be frozen rather than synced — otherwise the drift arrives looking like a regression. |
| The eval corpus and the field host disagree about the fusion, by more than anything Phases 4–8 were written to buy | `replace` reads **−9.6 p@1 worst case on three of four grid models** on `eval/cases.toml` (reranking-plan §12) and **+44.4 on the field store** (§17 · Field host, M5), with the same binary and the same model. Both are internally consistent, so one of them is describing the wrong population: the corpus is paraphrased synthetic cases with near-tied heads, the field store is long fact-shaped memories with repeated project names. The query file may also be needle-shaped, which is what a cross-encoder is best at. This is recorded as a risk rather than resolved because the temptation to ship the big number is exactly the failure mode the gate exists for; the resolution is the frozen real corpus and a rerank grid run against a store copy, and until then the shipped strategy stands. §12's distribution printer calibrates the confidence threshold that would satisfy both. |

**Follow-ups (not in this plan):**
- **~~Raise `[retrieval].relevance_weight` above 0.05~~ — done 2026-10-18: the default is 0.5.** Both
  instruments pointed the same way and neither showed a cost: the eval's own curve reads p@1 +9.6 /
  +13.5 / +17.3 at δ 0.2 / 0.3 / 0.5 with abstention unmoved and no `by_kind` regression at any cell
  (§17, Phase 2), and the field host reads +11.1 p@1 / +7.7 MRR at 0.5 over the shipped value **with the
  stage off** — the configuration that actually serves there — after +27.8 / +19.9 with it on (§17 ·
  Field host). Phase 2 said this is a release note rather than a config edit if the falsifier held, and
  it held: at 0.05, 0.2 and 0.5 on the field host and at every value on the corpus, with **no query
  losing first place in any run**. `eval/baseline.json` was re-recorded, as Phases 2 and 3 did, and the
  recorded `by_kind` map confirms the headline rather than contradicting it.
  - *The one caveat that was checked rather than assumed*: the worry about a large δ was a **stale fact
    winning on relevance**. Retired facts cannot lose that way — the recall path drops a memory whose
    `superseded_by` is set, pinned by `superseded_memories_are_excluded_unless_requested`, and the
    frozen copy confirms 43 such rows sitting outside ranking. What a large δ *can* do is promote a
    stale-but-never-retired memory over a fresher one, because nothing detected the contradiction. That
    is a consolidation-coverage question, not a δ question, and it is worth one measured case before
    anyone calls 0.5 settled for a store with weak reconciliation.
- **Re-run the rerank grid at the new δ.** Every cell in reranking-plan §12 — the measurement that chose
  `strategy = "rrf"` at `weight = 1` — was taken while δ was 0.05. δ is applied before the rerank stage,
  so it changes the head the stage is asked to judge; the fusion decision therefore has an input it no
  longer has. This is not hypothetical: at δ = 0.5 the field store's head is a different five memories
  than the one M5 measured.
- **Was the rerank stage muted rather than useless? Asked, answered: muted, by 44 points.** Query 5 of
  the field files was the exhibit (the model scored the right memory 0.999 and the served first 0.056,
  and rank 4 stood, because `rrf` at `weight = 1` may break a tie against one leg but never overrule
  vector+BM25 agreement); **M5 measured it** and the answer is larger than the hypothesis. On the same
  store with the stage on in every cell: shipped fusion 38.9 p@1 / 54.6 MRR, `weight = 2` 44.4 / 56.9,
  **`strategy = "replace"` 83.3 / 85.2** — eight rank-1 gains, zero losses. The count predicted from the
  model's own scores (15 of 18, i.e. 83.3) is what came back, though on a store that had changed
  underneath the prediction, so it is corroboration rather than proof. So `mode = "off"` stays right *at weight 1* and can
  no longer be cited as a verdict on the stage. What blocks flipping the shipped strategy is the other
  instrument: the eval grid had `replace` losing up to −9.6 p@1 on three of four models, and the same
  run that produced +44 also shows the grid's defect plainly (q17's head scores 0.037/0.031/0.023 —
  the answer in the window, the model rating everything at nothing). Reconciling them is the work now,
  and the shape most likely to survive is not a strategy default but a **confidence gate**: trust the
  model's order where the head's top score clears a threshold, keep the fused order where the head is
  flat, with the threshold calibrated by §12's distribution printer rather than guessed. Until that
  exists on a corpus larger than 18, the default stays. §17 · Field host has the full table.
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
- Make the dev container's `just check-native` unable to pass on a stale build. Today it
  can: `cargo fmt --check` reads file contents cargo thinks are unchanged, `clippy
  --all-targets` can finish in ~1 s, and `cargo test` can run a test binary that predates
  the tests you just added (measured in Phase 3: 117 of the tree's tests were missing from
  a "green" run). Either touch the sources at the head of the recipe, or drop the
  `#[ignore]`d count from the reported total and compare it with `cargo test -- --list`,
  whose 1 027 is what the tree actually declares. Until then §17's checks are only worth
  what their logs' test names are.
- Phase 6 has a concrete target it did not have before: the four `chain` cases whose
  queries confirm no seed (`chain_cases_paraphrase_their_entities_so_no_hop_can_start`
  lists the one that does). A τ-seeded or alias-resolved hop that reaches them is where
  that kind's remaining 60 → 100 lives, and it is falsifiable in one command — three of
  them flip, abstention stays 100.0, and the pinning test is updated to say so. Three, not
  four: `tuning the database the invoices live in` expects a memory that declares no
  entities at all, so it needs extraction or a reclassified case rather than a seed (§17).
  If a τ that reaches the rest costs abstention, §1.3's 100 → 0 is the shape to notice
  first.
- `matched.via` (Task 3.3, deferred to Phase 8 with the rest of the agent-facing surface):
  what a hop came through, as the seed entity's display name. Needs `EntityGraph::neighbours`
  to carry the originating seed (`MIN(depth)` per key, `sqlite_entity_graph.rs`'s recursive
  CTE), a `GraphHit` in its place, a `skip_serializing_if` field on the REST dto, an
  optional field plus a present/absent parsing test in both SDKs, and `just sdk-test`.
  Cheap to do while Phase 8 is deciding what agents see; expensive to do twice.
- `[graph].rank_weight` 0.3 vs 0.15 is an open question, not a settled default: on this
  corpus the two differ by one informational `alias` case, which is under the noise floor
  for a case flip. Either more `alias` cases or the field-host audit can decide it; a
  third grid on the same 73 cases cannot.
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

**Partly run: the store is here, the corpus is not.** The tool was finished long before the data
(`recall audit --config A --compare-config B --queries F [--json]`, with `A = B` for a self-comparison;
purity, refusal and JSON-leak tests pass). On 2026-10-18 the operator handed over the first half:

```
eval/store-2026-10-18.tar.gz
sha256 b20fda44397bfa87eb91f51f970792810935b0225d04ac7966c0385d445bfa17   2,038,835 bytes
```

Verified on arrival rather than assumed: `PRAGMA integrity_check` → `ok`; 225 rows in `memories`, 43 of
them superseded, **182 active**; oldest `2026-07-31`, newest write `2026-10-18T16:23:26Z`; kinds — 94
`preference.coding`, 65 `fact.project`, 17 `decision`, 16 `experience`, 15 `fact.person`, 12 `skill`,
4 `reference`, 2 `preference.personal`. That newest write predates the measurement run in §17 · Field
host, so the copy and those numbers describe the same state, which is what makes it a *frozen* corpus
rather than a snapshot of a moving one. Two facts about reading either: the ids `recall audit` prints
are the **last eight characters** of the memory UUIDs, and the copy is a cold tar of a live SQLite file
(`-wal` included), so it is consistent as a set — extract all three files together or none.

**It is private and it is not in git.** A store copy *is* someone's memories; the measurement bundles
quote them too. `eval/*.tar.gz`, `eval/store-*/` and `eval/field-*/` are ignored, and what is committed
from them is the digest above plus aggregate counts. Anyone reproducing §17's field numbers needs the
file from its owner, not from this repository.

**Still owed, and the reason this is "partly":** a query file of ~40 `expect` pairs written against this
frozen copy, in the voice of the person who stored those memories, plus three of the existing 18 field
queries re-anchored (they encode last week's answers, see §17 · The instrument drifted). With that file
in place the corpus stops being 110 synthetic memories and becomes the instrument that can decide the
fusion question in §16 — which is the only reason it is worth the hour it costs. §4's "real-store
`recall audit` is part of every gate" is now satisfied for measurement-by-tooling and for one 18-question
store; the frozen corpus is what turns that single store's anecdote into a gate.

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

Read on the field host 2026-10-18. **1.1 is met**: p50 6.7 → 306.2 ms, p95 9.6 → 603.9 ms for the
stage, 1.6 GiB resident against the ~1.5 GiB predicted, bought −0.2 MRR and nothing else across 18
real queries, and the host is now on the shipped `mode = "off"` (§17 · Field host). **1.5 is not
met**, and 1.1's outcome is what made it harder — with the stage off there is no session pool to
watch, so the table above needs a host deliberately running the stage for a few minutes. R1 stays
neither confirmed nor refuted until then.

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
  instrument in Task 2.6 and in every §4 gate. **Read 2026-10-18** (§17 · Field host): δ 0 → 0.05
  gained p@1 +5.6 / MRR +3.5 on 18 real queries whose expectations the owner wrote, and the sweep
  continued to 0.5, where it gains +27.8 / +19.9 over the shipped value. So the question below —
  does the eval's curve hold on real memories? — is answered, and it holds with a larger effect.
  One thing the summary could not say was whether §4's last row held; read per query it **did**: one
  rank-1 gain (q2), two improvements short of first (q15 5 → 4, q17 4 → 3), and nothing lost rank 1
  — the other two `⚑` rows changed between two memories that were both wrong. The condition Phase 2
  attached to raising the default is met.
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
  becomes real, and it should be raised in a release note rather than silently. *Both repeats done
  2026-10-18: 0.2 moved no top-1 at all, 0.5 moved seven of eighteen.*
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

**Shipped default: `[graph].expansion_slots = 1`.** Gated reading, against the baseline as
Phase 2 left it: recall@5 83.3 → **87.2** (+3.8), `complete@5` 80.8 → **84.6** (+3.8),
MRR 66.6 → **68.4** (+1.8), precision@1 **53.8 unchanged**, abstention **100.0**
unchanged. The gate's per-kind condition — `complete@5` up ≥ 15 points on `relational`
and `chain` — is met on both: **+28.6** and **+20.0**, with the `chain` half carrying an
asterisk recorded in full below
([Why the gate's `chain` half could not be measured](#the-gates-chain-half-could-not-be-measured-and-the-corpus-was-why)).

#### The anchor first (slots = 0)

`apply_expansion` is called on every recall and returns on `slots == 0` before reading
the list, so "knob at zero, no change" is a code path rather than arithmetic — which is
what the anchor then confirmed. With `RECUERDOS_AI_GRAPH__EXPANSION_SLOTS=0` and every
other knob at its shipped value, `eval --baseline eval/baseline.json` reported
`change: -0.0` on all four gated metrics (83.3 / 53.8 / 66.6 / 80.8). No pre-existing
assertion was edited anywhere in the phase; the only edits to existing test code were
passing the new `MemoryRecaller::new` argument, and the graph tests that predate this
phase pass `0`.

#### The knob grid (Task 3.5)

Ten cells, one `eval` process each (`scripts/recall-grid.sh --no-defaults`, slots through
`RECUERDOS_AI_GRAPH__EXPANSION_SLOTS`), measured **before** the corpus change described
below — so its absolute columns are the bare summary over the then-73 scored cases, not
the gated 59. The reference row is the shipped config with the knob at zero. Noise floor
carried forward from §16: ~0.7 recall, ~1.7 if a whole single-expectation case flips.

| cell | recall | p@1 | complete | MRR | p50/p95 | Δrecall | Δp@1 | Δcomp | ΔMRR |
|---|---|---|---|---|---|---|---|---|---|
| slots-0-rw030 | 75.8 | 45.2 | 72.6 | 58.7 | 8.2/9.5 | reference | — | — | — |
| slots-1-rw030 | 78.5 | 45.2 | 75.3 | 59.6 | 8.2/9.6 | +2.7 | +0.0 | +2.7 | +0.9 |
| slots-2-rw030 | 78.5 | 45.2 | 75.3 | 59.7 | 8.6/11.0 | +2.7 | +0.0 | +2.7 | +1.0 |
| slots-0-rw015 | 75.8 | 46.6 | 72.6 | 59.3 | 8.5/10.2 | +0.0 | +1.4 | +0.0 | +0.6 |
| slots-1-rw015 | 78.5 | 46.6 | 75.3 | 60.4 | 8.3/10.1 | +2.7 | +1.4 | +2.7 | +1.7 |
| slots-2-rw015 | 78.5 | 46.6 | 75.3 | 60.8 | 8.4/10.0 | +2.7 | +1.4 | +2.7 | +2.1 |
| slots-0-delta0 | 75.8 | 42.5 | 72.6 | 56.5 | 8.1/9.9 | +0.0 | −2.7 | +0.0 | −2.2 |
| slots-1-delta0 | 78.5 | 42.5 | 75.3 | 57.4 | 8.5/10.4 | +2.7 | −2.7 | +2.7 | −1.3 |
| slots-1-delta02 | 79.9 | 50.7 | 76.7 | 63.1 | 8.7/10.5 | +4.1 | +5.5 | +4.1 | +4.4 |
| slots-1-delta05 | 80.4 | 57.5 | 76.7 | 67.8 | 8.2/10.0 | +4.6 | +12.3 | +4.1 | +9.1 |

Four readings from it:

1. **The slot's gain is the same at every δ and every graph weight**: +2.7 recall, +2.7
   `complete@5`, +0.9 MRR, and precision@1 exactly ±0.0 in all six slotted cells. The
   p@1 column is not luck — index 0 is never on offer, so no cell can move it.
2. **The slot is indifferent to affinity, as designed.** It reads positions, not scores,
   so the hop's δ tax does not stop it: at δ = 0 it still buys +2.7/+2.7 (the −2.7 p@1 in
   those two rows is δ = 0's own cost — `slots-0-delta0` shows the same number). At
   δ = 0.2 and 0.5 the affinity gain and the slot gain both appear (+4.1, +4.6 recall),
   i.e. they stack rather than compete for the same cases.
3. **`rank_weight` 0.15 is not distinguishable from the shipped 0.3**, at either slot
   count: recall and `complete@5` identical in all three pairs, and the whole +1.4 p@1 is
   *one* `alias` case (alias p@1 20.0 → 40.0 — one of five, which is +1.4 overall and
   under the 1.7 it costs to flip a case). `alias` is one of Task 0.7's informational
   kinds, so on the gated subset the two weights differ by nothing measurable. Task 3.5
   asked whether the RRF term still earns its precision cost; the honest answer is that
   this instrument cannot see either an earning or a cost, so 0.3 stands and the question
   goes to §16 rather than into a config edit.
4. **Latency is flat** — 8.1–8.7 p50 across all ten cells, and the confirmation run below
   reads 8.2/9.8 with the knob at zero against 8.0/9.7 shipped. A slot is at most three
   scans over a ≤50-id leg and the truncated list; nothing in the §4 latency row moved.

The invocation, so the table can be re-made (it was recorded against the corpus *before*
Task 3.5's own case was added, and `expansion_slots` now defaults to `1`, so the zero rows
need saying out loud where they did not then):

```bash
./scripts/recall-grid.sh --no-defaults \
  --cell slots-0,RECUERDOS_AI_GRAPH__EXPANSION_SLOTS=0 \
  --cell slots-1 \
  --cell slots-2,RECUERDOS_AI_GRAPH__EXPANSION_SLOTS=2 \
  --cell rw015,RECUERDOS_AI_GRAPH__RANK_WEIGHT=0.15 \
  --cell slots-1-rw015,RECUERDOS_AI_GRAPH__EXPANSION_SLOTS=1,RECUERDOS_AI_GRAPH__RANK_WEIGHT=0.15 \
  --cell slots-0-delta0,RECUERDOS_AI_GRAPH__EXPANSION_SLOTS=0,RECUERDOS_AI_RETRIEVAL__RELEVANCE_WEIGHT=0 \
  --cell slots-1-delta0,RECUERDOS_AI_RETRIEVAL__RELEVANCE_WEIGHT=0 \
  --cell slots-1-delta02,RECUERDOS_AI_RETRIEVAL__RELEVANCE_WEIGHT=0.2 \
  --cell slots-1-delta05,RECUERDOS_AI_RETRIEVAL__RELEVANCE_WEIGHT=0.5
```

Adding `-- --baseline eval/baseline.json --max-drop 100` puts the *gated* `baseline … now:`
lines in each cell's kept output, which is where the 83.3 → 87.2 table below comes from; the
script's own columns stay the bare summary (§16 has that trap and the fix owed to it).

#### What the shipped cell buys, by kind

The head-to-head after the corpus change (74 scored, 22 informational), bare summary:
knob at 0 reads 75.5 / 45.9 / 71.6 / 59.3, shipped reads **78.8 / 45.9 / 75.7 / 60.2**.
Gated, that is the 83.3 → 87.2 table above. By kind, the two the gate reads, and nothing
else:

| kind | recall@5 | Δ | p@1 | Δ | complete@5 | Δ |
|---|---|---|---|---|---|---|
| `relational` | 57.1 | **+28.6** | 0.0 | +0.0 | 57.1 | **+28.6** |
| `chain` | 70.0 | +10.0 | 100.0 | +0.0 | 40.0 | **+20.0** |
| the other 15 kinds | — | +0.0 | — | +0.0 | — | +0.0 |

Which cases moved is worth naming, because the number is small enough to count.
`relational` is seven cases: with the knob at zero, three read 0% and four read 100%. With
one slot, two of those three go from finding **neither** expectation to finding both —
`relational: who owns a decision about a service` (seeds `billing service`) and
`relational: on-call for an app's dependency` (seeds `mobile app`). Two cases × 1.0 recall
unit over seven cases *is* the +28.6; nothing else moved. The third 0% case,
`relational: where a service's team sits`, seeds `billing service` and still reads 0% —
its answer is not one edge from that seed. So: five of the seven confirm a seed, two of
those five needed no help, two were delivered by the slot, and one is bounded by what the
hop reaches rather than where the slot places. `max_hops` and the edge direction are that
case's ceiling, not this knob.

`relational` p@1 stays 0.0 and *cannot* move: §10's property is that index 0 is never
touched, and a design that never touches index 0 cannot fix "which result is first" — it
fixes "did the pair arrive together". That is the promise this phase made seen from the
side it costs something on, and it is why `chain`'s four unreachable cases below are
Phase 6's problem rather than a reason to widen the slot.

#### Why one slot and not two

`slots-2` equals `slots-1` on recall and `complete@5` in both weight pairs it was run at
(+0.0, with MRR differences of +0.1 and +0.4 — under the noise floor). The reason is structural
rather than a coincidence of this corpus's size: a slot pays for itself only when the hop
reaches a *second* memory that the indexes left outside the window, and this corpus has
one promotable hop per affected query. A second slot therefore costs a visible row on
every recall that promotes, for a gain this instrument cannot see. `2` remains available;
`0` remains the pipeline as v0.4 shipped it.

#### The relocation Task 3.2 asked to re-measure

Task 3.2 moved the step from before the floor (the experiment patch's position) to after
the floor and after the rerank leg, and asked for re-measurement rather than trust. The
measured answer is: **no difference — and no way for this instrument to see one.**
`relational` recall and `complete@5` both read **57.1 → 85.7**, which is §1.3's row for the
expansion slot exactly, and the overall movement is the same size: +3.8 points of gated
recall here against §1.3's +3.9 there (the two corpora's absolute columns are not
comparable — §16). The eval cannot distinguish the two positions for two measurable
reasons: the rerank stage is `mode = "off"` in the eval config, so there is no later
reorder for it to be buried by; and no hop ever falls below the floor on this corpus,
because hop memories carry a constant 0.35 relevance against a 0.25 floor. So the
relocation is justified by what it guarantees (the floor's verdict stands; a reranker
cannot bury the hop) and pinned by a test
(`the_slot_is_applied_after_the_rerank_leg_not_before_it`, a reversing fake reranker plus a
slot), **not** by a measured gain.
Anyone who wants that gain measured needs a corpus where a hop lands below the floor —
which is the same instrument gap §16 already records for the field host.

#### The gate's `chain` half could not be measured, and the corpus was why

The first grid read `chain` `complete@5` at 50.0 → 50.0 in every cell, which is not what
§10 expected the mechanism to do. Reading the corpus rather than guessing explains it: a
hop starts at a name the store confirmed, and **all four `chain` cases Task 0.7 wrote
paraphrase their entities out of the query.**

| case | what its query says | keys the store actually holds | measured |
|---|---|---|---|
| `what a checkout change needs before merge` | "in checkout" | `checkout flow`, `Stripe` | no seed → the leg is silent |
| `who documents the funnel events` | "our funnel events" | `analytics pipeline`, `Priya`, `Kinesis` | no seed; with one, the row it misses is a single edge away |
| `the queue between payments and billing` | "a payment job", "billing" | `payment worker`, `billing service`, `Meridian team` | no seed (it passes on the text legs regardless) |
| `tuning the database the invoices live in` | "the database behind our invoices", "that engine" | `billing service`, `ledger-db`, `Postgres` | no seed — and its answer, the tuning-notes URL, declares **no entities at all**, so no hop reaches it at any threshold |

That last row is not a seeding problem and no phase fixes it by seeding: a memory with no
declared entities has no graph presence, so it is invisible to the hop by construction. It
either becomes an extraction question (ingest should have given it `Postgres`) or it stops
claiming to be a graph case. Recorded in §16 with the other half of this kind's debt.

With no seed, `EntityGraph::seeds` returns nothing, the leg is silent, and no tuning of
the hop or the slot can reach the far end of the chain. The two cases that pass, pass
through the text legs — so the `chain` kind has never measured the graph at all, and the
gate's `chain` half was written against an instrument that could not read it. This is G2
(exact seeds only) working as designed: §1.3 measured what fuzzy seeding costs
(abstention 100 → 0), and Phase 6's alias classes are the sanctioned way to buy it back.

So the corpus gained one case rather than the guard being relaxed:
`chain: the checkout flow named outright` asks the same checkout → Stripe → PCI chain
with the entity named. It is the kind's only seedable instance, and it behaves exactly as
the phase predicts — **50% recall / 0% `complete@5` with no slot** (it finds the bridge
and not the rule, the failure this phase exists for), **100% / 100% with one**. That
single case is where the kind's +10.0 recall / +20.0 `complete@5` above comes from; the
four originals stay at +0.0 each. It is informational like its siblings, so no gated
number from any phase is restated, and the original four cases are untouched.

`chain_cases_paraphrase_their_entities_so_no_hop_can_start` pins the shape instead of
leaving it to prose: it lists which `chain` cases confirm a seed (exactly one) and asserts
`relational` keeps at least five of its seven. Either list moving means a recorded number
stopped meaning what it says, which is worth a failing test to learn.

**Reading of the gate, stated plainly:** `relational` +28.6 passes on the mechanism.
`chain` +20.0 passes as written *only because the corpus gained the case that can exercise
the mechanism*; on the cases the phase inherited it delivered exactly nothing, and the
reason is now measured and pinned. Phase 8's release note should not claim the chain kind
works: four of its five cases still name nothing the store knows, and one of those four
expects a memory that declares no entities at all, which no seed — exact or resolved — can
reach.

#### Tests added (19)

| where | count | what they hold |
|---|---|---|
| `recall_ranker.rs` | 8 | the move; no slot count touches index 0; a candidate sitting exactly *on* the edge stays there; a hop already in the head is left alone; slots fill in graph-leg order and never promote the same hop twice; `slots == 0` and an empty leg return before reading the list; the promoted row keeps its `score` and `match_detail` (so the list is knowingly no longer score-monotone); slot counts wider than the list are harmless |
| `memory_recaller.rs` | 8 | slot 1 puts the connected answer second and leaves first alone; zero slots is the pipeline as it shipped; nothing is promoted when the query names no known entity; a category filter the hop fails is not a bypass; the floor's refusal is not reversed by a slot; `limit = 1` shows exactly what it showed before; the step runs after the rerank leg rather than before it; and the G2 guard end to end — a hop starts only from a name the graph confirms, so a misspelled seed seeds nothing while the correctly-spelled name reaches the answer |
| `config.rs` | 2 | the default is the value the grid chose, read from file and env; out-of-range refuses to boot |
| `eval.rs` | 1 | which cases the graph is allowed to start from (above) |

`just check-native` is clean on this branch — `cargo fmt --check`, `cargo clippy
--all-targets -- -D warnings`, `check-boundaries.sh` ("no violations found"), and
**1 016 tests passing, 11 ignored** (the ONNX ones CI skips) across the ten binaries —
1 027 declared, which is exactly what `cargo test -- --list` counts, and 19 more than the
1 008 this branch started from. `just sdk-test` was not run because nothing reached the
wire: Task 3.3 did not land.

One caveat about that number, because it was nearly wrong. The first `just check-native` of
this phase reported green and **was not valid**: under `docker compose run` the build cache
can answer for a tree the container is not looking at. That run clippy'd
`recuerdos-ai v0.3.1` when the manifest said `0.4.3`, `cargo fmt --check` said nothing about
a file that was not formatted, and `cargo test` ran a unit-test binary 117 tests behind the
tree — none of this phase's tests appeared in its output. Every check quoted here was
re-run after `find src tests -name '*.rs' -exec touch {} +`, and the reading is accepted
only because the log contains this phase's test names and its totals match
`cargo test -- --list`. `test result: ok` by itself is not evidence. §16 carries the
environment fix.

#### Deviations from the task text (all deliberate, none silent)

- **Two guards the plan's arithmetic did not state.** The rule as written promotes the
  first hop whose position `p` satisfies `p >= max(1, limit - slot)`; at `limit = 1` the
  bare subtraction qualifies index 0, so `.max(1)` makes the plan's "never index 0"
  arithmetic rather than a comment. And `p > slot` is required so a slot cannot promote a
  hop that a *previous* slot already moved — without it, slot 2 re-promotes the row slot 1
  placed and the list loses its second seat. Both are pinned by tests.
- **Task 3.3 (`matched.via`) did not land.** The plan marks it optional. Extending the
  recursive CTE to carry the originating seed means changing `EntityGraph::neighbours`,
  the REST dto, and both SDKs — an agent-facing surface decision that Phase 8 owns (the
  same reason §10 refuses to put it in the MCP text: context budget). `matched.graph_rank`
  already tells a client a row came from the hop. Deferred to Phase 8 rather than dropped;
  §16 carries it, and `just sdk-test` is therefore not this phase's gate.
- **The factorial was run as 10 cells, not 24.** The full product of 3 slot counts × 4 δ
  × 2 graph weights is mostly cells that cannot inform a default this phase owns: `rw` is
  measured at both slot counts and at δ = 0.05 and δ = 0, δ is measured at both slot
  counts, and the missing `rw × δ > 0.05` combinations would only re-measure Phase 2's
  already-shipped δ decision. Every axis and both interactions that could change
  `expansion_slots` are covered; each cell is one ~50 s `eval`, and §17's invocation is in
  the grid log header for anyone who wants the rest.
- **The eval corpus gained a case** (see above). A measurement-instrument change, made
  because the instrument could not read the thing the gate asks about; the four inherited
  cases are byte-identical and no recorded number was restated.
- **`recuerdos-ai config` grew a `[graph]` block** (hops, weight, floor, and
  `expansion_slots`, the last printed at zero too). Not in the plan; it is the place an
  operator sees which of these pipelines they are running without opening the file, and
  `retrieval` and `rerank` already argue for exactly that.
- **Commit `f0ad350`'s message overcounts.** It says nine `recall_ranker` tests; the file
  gained eight. `head_with_hop_at` is a fixture that returns the list and its edge, and it
  was counted as a test. The history is left as-is and the correction is recorded here.

#### Not measured (blocked, and deliberately not approximated)

- **`recall audit` on the field host's store, every changed result reviewed by hand** —
  the last box of §10's DoD. **Half read 2026-10-18** (§17 · Field host): the audit ran on the
  deployed store with 18 owner-authored expectations; top-1 changed 0/18, `complete@5` flat at a
  store already 88.9% complete inside the window, MRR −2.8. The hand reading is what is left, and
  the marker to read is **`(from N)` in the `after` column, not `⚑`** — no top-1 moved, so no flag
  fires, while the displaced rows and the seat's occupant are both marked with where they came from.
  The recipe, which writes nothing:

  ```bash
  # The comparison side is the pipeline before this phase. Put the key inside [graph],
  # not at the end of the file — a key landing under the last section is the v0.4.1
  # orphaned-key bug, and the audit would then compare one config with itself.
  awk '{print} /^\[graph\]$/ {print "expansion_slots = 0"}' \
      /etc/recuerdos-ai/recuerdos-ai.toml > /tmp/noslot.toml

  recuerdos-ai recall audit --config /etc/recuerdos-ai/recuerdos-ai.toml \
      --compare-config /tmp/noslot.toml \
      --queries /tmp/questions.txt --user <handle>
  ```

  What to review: each query where the shipped config brings a memory into the top five
  that the zero-slot config did not, and the row it displaced from last. Each arrival
  should be a sentence a person would call related to the question. The failure mode to
  look for is not a wrong memory but a *vacuous* one — a hop that fired on a name the
  query shares with an unrelated memory ("service", "team", a surname), which the eval's
  18 entity keys are too few to represent. That is also the case for raising
  `expansion_slots` above 1 on a real store: this corpus cannot see a second promotable
  hop, a field store with thousands of entities can.

#### What Phase 4 inherits

The gated baseline is re-recorded at the shipped default: **87.2 / 53.8 / 68.4 / 84.6**,
abstention 100.0, p50 8.0 ms / p95 9.4 ms, 59 gated cases (74 scored, 22 informational).
The slot is anchored at zero by code path, so a Phase 4 regression in `relational` is
Phase 4's. Three things this phase leaves as debt rather than success: `relational` p@1 is
0.0 and structurally unreachable by a slot; three of the `chain` kind's four zero-seed cases
are the concrete target for Phase 6's alias/τ work (that kind's remaining 60 → 100 lives
there, and `chain_cases_paraphrase_their_entities_so_no_hop_can_start` pins which rows must
flip — the fourth case expects an entity-less memory, so it is nobody's seeding problem);
and `rank_weight` 0.3 vs 0.15 is an open question this instrument cannot answer.

### Field host — the readings Phases 0–3 owed (v0.5.0-rc1, 2026-10-18)

One machine, one operator, one afternoon. The host is a host install (`lamalamita`, store at
`/home/lamita/.recuerdos-ai/data`), the binary is `v0.5.0-rc1`, and the instrument is
`scripts/field-measure.sh` (branch `chore/field-measure`), which builds each config variant,
asks the binary to read the value back, and refuses to measure when the read-back disagrees —
the v0.4.1 orphaned-key bug made fatal rather than silent. **18 queries, all 18 carrying an
`expect` the operator wrote against their own store**, which was listed first from
`GET /v1/memories/export?format=markdown` so the questions were written from what is actually
there rather than from recall.

Config during the readings: `relevance_weight 0.05`, `expansion_slots 1`, graph on (2 hops,
weight 0.3), rerank **on** at depth 5 / rrf weight 1 / 1200 ms, floor 0.25, recency half-life
90 d. The audits ran the 0.5.0-rc1 binary against the live store file while the daemon still
served 0.4.3 — that is a reading of the new pipeline on real memories, not of the service; the
daemon was restarted onto 0.5.0-rc1 after these runs with `[rerank].mode = "off"`.

The floor is worth naming before the numbers, because it disposes of a worry: the client floor
compares *relevance*, and δ multiplies the *score*, so no δ cell here can move abstention —
which is why the eval's δ grid reported abstention unchanged and this reading does not measure
it. It is not an unmeasured risk; it is a knob that cannot reach that metric.

#### δ — the falsifier Phase 2 owed, and it reads the other way

| comparison | top-1 changed | mean displacement | p@1 | MRR | complete@5 |
|---|---|---|---|---|---|
| δ 0 → 0.05 (what shipped) | 3/18 | 0.20 | 44.4 → **50.0** | 57.6 → **61.1** | 88.9 → 88.9 |
| δ 0.05 → 0.2 | 0/18 | 0.34 | 50.0 → 50.0 | 61.1 → 61.1 | 88.9 → 88.9 |
| δ 0.05 → 0.5 | 7/18 | 0.85 | 50.0 → **77.8** | 61.1 → **81.0** | 88.9 → 88.9 |

Phase 2 shipped δ = 0.05 as "the smallest value that clears the gate while the curve keeps
climbing", and §4's honest caveat was that the eval might reward relevance *by construction*.
On the operator's own memories, with expectations they wrote independently of any pipeline
output, that caveat does not hold: the shipped value leaves **27.8 points of precision@1 and
19.9 of MRR on the table**. The eval understated the gain rather than inventing it.

The shape is explicable. What a 90-day half-life punishes hardest on a personal store is exactly
the material that never goes out of date — hardware specs, tooling preferences, project rules —
and those are the questions in the file (`which GPU does the other inference server use`,
`which Python tools for packaging, linting, testing, typing`, `which repository must contain all
implementation changes`). A 5% relevance tax cannot beat an age advantage; by 50% it does. The
0.2 cell doing nothing while 0.5 moves seven top-1 results is not a contradiction: the pairs the
0.05 flip could afford were already flipped, and the rest need a penalty large enough to beat a
recency term that is still winning by more than 15%.

**Per query, in full, so the arithmetic is not left in the air.** The `expect …: rank N → M`
lines resolve the question the summary could not. At **δ 0 → 0.05**: one query gained rank 1 (q2,
2 → 1), two improved without reaching it (q15 5 → 4, q17 4 → 3), nothing lost it; the other two
`⚑` rows (q11, and q15's top-1) changed between two memories that were *both* wrong, which is why
3 top-1 changes net out at +1. At **δ 0.05 → 0.5**: five queries gained rank 1 (q3 3 → 1, q4 3 → 1,
q6 4 → 1, q16 4 → 1, q17 3 → 1), nothing lost it, and the other two `⚑` rows are q10 and q11
reshuffling noise. So **§4's last row — "no query whose `expect` was at rank 1 loses it" — holds on
the field host at both values**, which is the condition Phase 2 attached to raising the default, and
the eval's own grid never regressed a kind at any δ either.

What *still* argues for caution is narrower than the worry Phase 2 recorded, and it is not about
relevance overrunning rank evidence: a δ that large makes relevance largely win, so the one case it
can lose is two near-identical memories where only recency separates them and the older is stale —
a superseded fact consolidation failed to retire. That is checkable rather than philosophical: the
export marks superseded rows, so `grep -c superseded` over it says whether the failure class even
exists on this store. It is worth noting that the one query a large δ left un-fixed even at 0.5 (q15: `pnpm` served first
when the answer is the `uv`/`ruff`/`pytest` memory) is not that class either — it is a vocabulary
collision, where "package management" pulls the JS preference ahead of the Python one, and the
cross-encoder knew better (rerank 0.371 for pnpm against 0.982 for the answer).

*The unexplained latency cell is explained, and it was never about δ.* The rerank leg's own p50 in
those two files is 306 ms and 232 ms, and the recall latency tracked it exactly (313.6 and 238.8) —
with the embedder and fusion costing ~6 ms, a recall's wall clock *is* the stage's wall clock, and
the stage's p50 varies 232–306 ms run to run on this box. So δ has no measurable latency effect at
all; what looked like a cheaper δ was a faster afternoon for the model. The recall noise floor for
this host is therefore bounded by the stage whenever the stage is on, which is one more reason the
`mode = "off"` decision makes the host easier to reason about.

#### The stage is not useless, it is muted — and this is the finding neither gate was looking for

Query 5 of the δ files, in both cells, is the exhibit. The right memory (`Residential proxies are
mandatory…`) sits at rank 4 with **rerank 0.999**, and the memory served first (`The
linkedin-scrappy subproject is a private Apify Actor…`) carries **rerank 0.056**. The model was
decisive and correct by a factor of eighteen, and the order did not change. That is
`[rerank].strategy = "rrf"` at `weight = 1`, whose documented semantics are exactly this: *lets the
reranker break ties against a single retrieval leg but never overrule vector+BM25 agreement*. Here
the agreement was a lexical artifact — "linkedin-scrappy" appears in the wrong memory's text, and
BM25 is content to agree about it.

The shipped default was chosen on a grid over the eval corpus, which found `replace` losing up to
−9.6 p@1 and `weight = 2` giving back 2 points plus a third of the `needle` kind's, because on that
corpus a cross-encoder's raw-score gaps mean little (expected pairs score 0.006 as often as 0.99)
and the index legs' agreement is nearly always right. That store is paraphrased synthetic cases. A
real store repeats project names across memories, which is the condition under which lexical
agreement is confidently wrong — and then weight 1 spends the stage's judgement entirely. So Phase
1's reading here ("45× the p50 for −0.2 MRR") needs this footnote to be honest: **the stage is not
shown to be wrong, it is shown to be muted**, and its cost buys nothing *as fused*. The test that
would settle whether the latency has a purpose is one config line and the same query file: rerank
`on` with `strategy = "replace"`, and separately with `weight = 2`, each against the shipped fusion.

This does not undo the `mode = "off"` decision. At weight 1 the stage demonstrably does not pay for
300 ms, so off is right as configured; what is open is whether some *other* fusion makes it pay.

#### Scored by itself, the reranker beat the order it was put into

The `after` side of `m2-005-vs-05.txt` carries the model's own score per line, which makes it
possible to ask what the stage's judgement alone would have served. **Its highest-scoring memory
was the expected one in 15 of 18 queries**, and there is no query where it put something at 0.9+
that was not the answer: the two cells it scored 0.000 across the board (q10, q11) are the two with
nothing relevant in the window, and its single disagreement (q17, where the expected memory took
0.031 against 0.037) sits entirely under 0.04 — noise, not a confident dissent. Serving by the
model's argmax would read **83.3 p@1 against the fused 77.8**, gaining q5 and q15 and losing q17.

That is a falsifiable prediction, so M5 (rerank `on`, `strategy = "replace"`, then `weight = 2`)
is a real experiment rather than a fishing trip: if `replace` lands near 83 the reading above is
right and the shipped fusion is the weakest link in the pipeline; if it lands at or below 77.8,
these scores mean less than their ordering suggests and the eval grid's warning about raw-score gaps
wins. **It landed on 83.3** — the run is below. One honest wrinkle: the comparison point in that
sentence (77.8) was the fused p@1 on the *pre-drift* store, so the valid test is against the same
run's `before` column, 38.9, and the predicted absolute happened to survive the drift while the
predicted floor did not. Even a threshold set last night rots on a live store; either way the claim
"the stage buys nothing" is too coarse — what it buys is the most accurate ordering measured on this
store, most of which the fusion declines to use.

#### M5: the fusion is the weakest link in the pipeline, by a wide margin

The run that the read-back bug cost is done, and it is not a small result. Same store, same 18
queries, δ 0.05 throughout, **the stage on for every row** so the only difference is how its
judgement is fused:

| configuration | p@1 | MRR | recall p50 | resident |
|---|---|---|---|---|
| stage on, `rrf` weight 1 — the shipped fusion | 38.9 | 54.6 | 236 ms | 1.6 GiB |
| stage on, `rrf` weight 2 | 44.4 | 56.9 | 233 ms | 1.6 GiB |
| stage on, **`strategy = "replace"`** | **83.3** | **85.2** | 236 ms | 1.6 GiB |
| *stage off, δ 0.05 — what the host runs today* | *38.9* | *55.7* | *6.3 ms* | *194 MiB* |
| *stage off, δ 0.5* | *50.0* | *63.4* | *6.3 ms* | *194 MiB* |

`complete@5` is 88.9 in every cell, as ever: nothing here finds anything new. **Eight queries gained
rank 1 and none lost it** — q1, q2, q3, q4, q5, q6, q15, q16 — eight `⚑` rows, eight gains, net +8,
which is 38.9 → 83.3 with nothing paid for it. The count predicted above from the model's own scores was
15 of 18; the measured count is 15 of 18, on a store that had drifted since the count was taken.

So the honest summary of Phase 1 and Phase 7's default, rewritten: **the cross-encoder is the most
accurate signal in this pipeline on this store, and the shipped fusion throws away 44 points of p@1
of it.** `mode = "off"` was the right call *at weight 1*, where the stage demonstrably changed nothing
and cost 230 ms and 1.4 GiB; it was not a verdict on the stage, and now it cannot be cited as one.

`weight = 2` is the control that makes this credible rather than a wish: raising the reranker's RRF
weight from 1 to 2 bought exactly one query (+5.5 p@1) and moved a second top-1 from one wrong answer
to another. So the effect is not "more reranker influence is good" — it is specifically that *reading
the model's order* works and *voting the model in as another leg* does not, which is what the two
strategies mean arithmetically.

**What the same file says against it, in the same run.** Two of the eighteen heads are flat: q17's five
scores are 0.037, 0.000, 0.031, 0.023, 0.002 — a query whose answer is sitting in the window and which
the model rates as good as nothing — and q10/q11 are all 0.000. That is precisely the defect the eval
grid recorded when `replace` lost up to −9.6 p@1 on three of its four models: *a cross-encoder's
raw-score gaps mean nothing on some corpora, and banded raw scores reorder a near-zero head into
noise.* The field confirms the mechanism and bounds it: `replace` wins big where the head has signal
(16 of 18) and does nothing where it does not (2 of 18), and because the abstention floor compares
*relevance* and not the rerank score, a flat head cannot cause a wrong abstention either way.

The two instruments therefore disagree about `replace` by 44 points on one store and −9.6 on the
corpus, and that gap is now the most important open fact in this project — larger than anything Phases
4 through 8 were written to buy. Three readings, and only measurements separate them:

- the query file is needle-shaped by construction (each `expect` names a memory whose text contains
  the answer phrase), which is the shape a cross-encoder is best at and the shape the eval corpus's
  paraphrased cases were built not to be;
- the eval corpus's `chain`/paraphrase cases genuinely break `replace`, in which case the right object
  is not a strategy default but a **confidence gate** — serve the model's order where the head's top
  score clears a threshold, keep the fused order where the head is flat — and §12's distribution
  printer is the instrument that calibrates such a threshold rather than guessing it;
- or both, which is the most likely answer given that both instruments are internally consistent.

**No default changes on this evidence.** The precedence rule set when the field host was built is that
the corpus gates and the field falsifies, and 18 queries on one person's store — even this store, even
with a prediction that landed to the decimal — is not enough to flip a shipped strategy. What it is
enough for: the frozen real corpus stops being optional bookkeeping and becomes the thing that decides,
and any rerank grid must be run against a real store copy rather than `eval/cases.toml` alone.

#### The seat's cost has a mechanism, and it is visible in these same files

Four lines in each δ file carry **no `rerank` score at all on the side that ran the stage**. A
missing score means the memory was never judged, and exactly one thing joins the list after the
stage does: the graph's reserved slot, which the pipeline pins last (`memory_recaller.rs`, and the
test named `a_reserved_slot_puts_the_connected_answer_second_and_leaves_first_alone` — rank 2 is its
designed position, not an accident). Those four arrivals are `52813f19` at relevance 0.62,
`8ed20c4a` at 0.30, `8ed20c4a` again at 0.16, and `640bd611` at 0.35 — three of them memories from a
different project than the question, sitting above the answer, unjudged and unjudgeable.

That closes Phase 3's reading from "a number with no shape" to a mechanism: **G1 held (0 top-1
changes) and MRR fell 2.8 because the slot inserts an unjudged memory at position two and everything
below shifts once**, which by construction cannot move first place and does cost reciprocal rank in a
store whose ranking was already right. It also says something the Phase 3 text did not anticipate:
enabling the seat and the stage together means the model is never allowed to veto what the seat
inserts. Whatever Phase 8 decides about the seat's default now has both a measurement and a reason.

#### Two of eighteen are not ranking failures at all

q10 and q11 — the two inference-server questions — have **no memory above relevance 0.24** (q11:
0.31) anywhere in the returned head, and the expectation was `absent → absent` in every cell. The
pipeline did not mis-rank an answer; it found nothing on the topic. Two readings, and they are not
the same decision:

- **The memory is not in the store.** Then abstention is *correct* behaviour, these two rows are
  impossible expectations, the honest complete@5 for the answerable queries is 16/16, and the
  client would in fact have abstained — the floor is 0.25 and the whole head sits under it. Worth
  recording that this reading makes the 88.9% ceiling used in the seat argument *more* damning, not
  less: the recall job was already done.
- **It is in the store under different words** — a query saying "Strix Halo" against a memory
  saying "Radeon 8060S Graphics", say. Then this is the first field-observed instance of the failure
  Phases 4 (embedding prompts) and 6 (aliases) were written for, and a better reason to build one of
  them than the synthetic `chain` kind ever was.

`grep -in "radeon\|3090\|strix\|vram" ~/field-private/memories.md` decides which, and it is the
highest-value five seconds available in this whole project: it is the difference between "the plan's
remaining phases solve a problem this store does not have" and "here is one, named, with the query
that fails".


#### The reserved seat — aggregate in, hand reading owed

| slots 0 → 1 (shipped) | value |
|---|---|
| top-1 changed | **0/18** |
| mean rank displacement | 0.58 |
| p@1 | 50.0 → 50.0 |
| MRR | 63.9 → **61.1** |
| complete@5 | 88.9 → 88.9 |
| recall latency | p50 307.8 / p95 634.7 → p50 308.8 / p95 617.9 |

G1 held exactly as designed: a reserved seat never touches the first result, on real memories as
well as in the arithmetic. What it did cost is arithmetic rather than observation — with no top-1
change, −2.8 points of MRR over 18 queries is 0.5 reciprocal-rank units, and the moves that cheap
are **three expected memories pushed from rank 2 to rank 3**, or one pushed from rank 2 out of the
window. The per-query `expect …: rank N → M` lines say which, and those lines are the review: in
either case a row that was not the answer took a place a correct answer had.

Why the eval's benefit is absent comes from the ceiling column: `complete@5` was already **88.9%
with the slot off**. The mechanism earns recall where the answer is *outside* the window, and on a
store this well-populated it almost never is, so there is nothing here for it to gain and a rank it
must pay for. That is the §16 risk row for a personal store arriving as a measurement rather than
as a worry: `relational` +28.6 on 74 curated cases has no counterpart when 16 of 18 answers are
already in the top five.

Hand review still owed on the moved blocks (the marker to read is `(from N)` in the `after`
column, not `⚑`: a rank-2 row whose `from` is larger than 2 is the seat used). What it can still
change is the recommendation, not the arithmetic — if those arrivals are related-but-not-the-answer,
the seat is defensible for stores whose recall is short and wrong for stores whose recall is not;
if they are vacuous, it is wrong generally. `expansion_slots = 0` is meanwhile the honest setting
for a store of this shape, and it is one config line.

#### The second stage — Phase 1's Task 1.1, and a finding nobody gated

| | rerank off | rerank on (deployed) |
|---|---|---|
| recall latency | **p50 6.7 / p95 9.6 ms** | **p50 306.2 / p95 603.9 ms** |
| p@1 / MRR / complete@5 | 50.0 / 61.3 / 88.9 | 50.0 / 61.1 / 88.9 |
| daemon RSS | 31,756 kB (fresh start, nothing served yet) | 1,668,708 kB (after a real search) |
| re-run, stage off (the live config now) | recall p50 6.3 / p95 9.1 ms, self-comparison 6.3 vs 7.5 | — |
| re-run, warm daemon | **198,996 kB** | — |

Eighteen calls, five candidates judged each, `split: not measured` because the replay runs with no
deadline — Phase 1's own reporting, behaving as specified. The stage costs **45× the p50 and about
1.6 GiB resident** and bought −0.2 MRR and nothing else across 18 real queries. The plan's predicted
"~1.5 GiB freed" is confirmed by the process holding 1.59 GiB, and the strictly like-for-like
comparison the first pass could not make is now made: a **warm** daemon with the stage off holds
198,996 kB, so the second stage is roughly 1.4 GiB of resident memory on this box. (The re-run's
quality figures are deliberately not added to the p@1 row above: the store grew between the two runs,
so its absolutes are not comparable — see *The instrument drifted while it was being used*.)

The operator set `mode = "off"` after this reading. That is a config decision about their own
traffic, and it has a consequence for this plan: **Task 1.5's queue cannot be observed on this host
while the stage is off.** Either it gets run in the few minutes it takes with `mode = "on"`, or it
is recorded as what it would then be — a mechanism this deployment does not exercise — and R1 stays
neither confirmed nor refuted.

#### Limits of this reading, stated before anyone quotes it

18 queries on one personal store, expectations authored by the person whose memories they are:
strong for "is this the memory I meant", weak for breadth, and its absolutes (p@1 50.0, complete@5
88.9) describe that store's difficulty and are **not comparable to the eval corpus's 87.2 / 53.8 /
68.4 / 84.6**. Latency is the same rule as `eval`'s latency columns — the embedder is inside both
numbers, so the difference is the finding and the absolute is a property of one machine. Three
single-knob comparisons at n=18 means one flipped query is 5.6 points of p@1, so anything under
about two queries in either direction should be read as nothing, exactly as the eval's 1.7-per-case
noise floor says here.

#### A second reading, with the stage off — the cell that was missing

The host now runs `mode = "off"`, and every δ comparison above had the reranker on *both* sides, so
the absolute at the shipped configuration was unknown even though the differences were valid. Re-running
the sweep closed that, on a store that has in the meantime grown (see the drift section below):

| δ | p@1 | MRR | complete@5 | rank-1 gains | rank-1 losses |
| --- | --- | --- | --- | --- | --- |
| 0 | 27.8 | 47.7 | 83.3 | — | — |
| **0.05 (shipped)** | 38.9 | 55.7 | 88.9 | — | — |
| 0.2 | 44.4 | 59.7 | 88.9 | 1 (q3) | 0 |
| 0.5 | 50.0 | 63.4 | 88.9 | 2 (q3, q17) | 0 |

**The improvement is not an interaction with the reranker**: with the stage off, 0.05 → 0.5 still reads
+11.1 p@1 and +7.7 MRR, monotone in δ, and once again nothing lost first place (the third `⚑` row in
the 0.5 run is q15 changing between two memories that were both wrong). That is the third consecutive
field run where §4's last row held, at four different values of δ.

The ladder also says something new. Between δ 0 and 0.05, `complete@5` itself rose 83.3 → 88.9 — q15's
answer entered the window at all. Because truncation happens *after* the reweighting, δ is not only a
re-ordering knob: a larger δ changes which five memories exist to be seen. That is worth knowing before
anyone reads it purely as a precision dial.

**Task 1.1 is now closed in both directions.** With the stage off the recall p50 is 6.3 ms (p95 9.1,
self-comparison 6.3 vs 7.5), so the ~300 ms measured earlier was the stage entire, and the warm daemon
reports **198,996 kB where with the stage loaded it reported 1,668,708 kB** — the second stage costs
roughly 1.4 GiB of resident memory on this box, for a judgement the shipped fusion declines to use.

#### The seat, measured a second time, and what the floor has to do with it

M1 reproduced itself: **0 of 18 top-1 results changed** (G1 held twice), p@1 flat, MRR −2.2 against the
earlier −2.8 — same sign, same size, independent run. What the second pass adds is a limit on how far
that can be pushed. The audit is deliberately floorless (`floor: clients drop below 0.25, this audit
shows everything`), and `apply_expansion` can only promote a memory that is *already in the list* — its
own contract is "filters and the floor hold". So the striking rank-2 arrivals this run displays
(relevance 0.16, 0.21, 0.22, 0.24) are rows a client would never have been shown: they are dropped
before the slot ever runs, and it is the audit's floorless view that puts them on screen. Two conclusions
survive that and two do not:

- Survives: the slot moves a below-window memory into second place, and in this store that costs
  reciprocal rank without ever touching first — measured twice.
- Survives: enabling the seat and the stage together means the model never judges what the seat places.
- Does not survive: "the seat serves junk". On a client the promoted row has passed the floor, so the
  complaint is that it displaces a better one, not that it is nonsense.
- Does not survive: reading the exact −2.2/−2.8 as a client-facing loss. It is a floorless-audit number.

#### The two GPU failures: the memories are not in the store

Quoting the expectations back at the store, key-free, answers it — and the answer is unambiguous:

```
1. Radeon 8060S Graphics   → 1. Code coverage minimums… rel 0.09   2. MINIO_API_PORT… rel 0.06
2. RTX 3090 24GB           → 1. recall-quality-plan Phase 3 shipped… rel 0.10
```

A stored memory quoted to itself returns near 0.9. These return 0.09 and 0.10, which is what an empty
region of the store looks like. So the vocabulary-mismatch hypothesis is dead: there is nothing to
match. The consequences are load-bearing for the rest of this plan:

- Those two `expect` rows are impossible, abstention is *correct* behaviour for them, and the honest
  denominator is 16 answerable queries — on which the store is **16 of 16 complete inside five**.
- **Phases 4 (embedding prompts), 6 (aliases) and 7 (temporal cues) address recall, and this store has
  no recall failure left.** There is now no field-observed instance of the problem any of them was
  written for, which is a much stronger statement than "the eval corpus did not show improvement".
- What remains is ordering, which is δ's domain, and one ordering failure that is neither: the shipped
  fusion declining to use a cross-encoder judgement that was right.

#### The instrument drifted while it was being used

The same shipped configuration reads 38.9 p@1 today where it read 50.0 last night, and the cause is
visible in the listings: six new memories about this work are now in the store (`fc009ca5`, `ecdb7423`,
`ac027aee`, `f483a47f`, `58887eb2`, `40d9bf52` — the field measurements, the operator server, the
decision to stop the phase sequence). Queries 1, 2 and 3 of this file ask about this project's own
configuration, and the memories written *while doing the work* now outrank the older ones the `expect`
substrings point at. On q1 the new top result is "On 2026-10-18 the field host measured relevance_we…"
where the expectation points at "Phase 2 shipped relevance_weight = 0.0…" one row below.

That is not a retrieval failure; the newer memory is arguably the better answer. It is the ground truth
going stale, and it invalidates three assumptions at once:

- **A live store cannot be re-measured as a fixed instrument.** Every run of this file is a different
  measurement, and nothing in the output said so until the aggregate moved.
- **Task 0.9's copy has to be frozen, and the query file pinned to the store it was written against.**
  A copy that keeps syncing, or expectations written against a store that keeps absorbing the work,
  reproduces exactly this drift — silently, on the way to looking like a regression.
- **Queries 1–3 need widening or retiring.** Their `expect` substrings encode last week's answer.

It is also the second time this week that a missing digest cost time: the script records a store digest
exactly so drift is a changed hash rather than a mystery, and that step never ran because no API key
was entered.

#### M5 did not run: the guard caught a bug of its own

*(Kept as the record of why the first attempt produced nothing. The re-run happened on the same day and
its result is above, under "M5: the fusion is the weakest link"; the bug below is why that re-run was
needed at all, and why the guard now reads inside the section that owns each key.)*

```
!! weight2.toml: asked for weight=2, the binary reads '0.3'.
```

The read-back asked the report for `weight` and got `[graph].weight`, because the report prints key
names unscoped and `weight` exists in two sections — the v0.4.1 failure (a value read from the wrong
section) reproduced inside the check built to catch it. Fixed by scoping every read to the section that
owns the key and reading `[rerank].weight` off the line the report actually folds it into. Worth stating
plainly: the guard refused to measure rather than reporting a difference that was nothing, which is the
right direction for it to fail, but it cost a run, and the fusion question below is still unanswered
because of my code rather than the system's behaviour.

Closed by the two runs: the existence probe (the memories are absent, above), the δ sweep with the
stage off (above), the warm RSS with the stage off (above), and M5 (above), which answered the fusion
question with a bigger number than the hypothesis that asked it. Still owed, in the order that would
change a decision: **a query file against the frozen store copy, and a rerank grid run on that copy** —
the copy arrived on 2026-10-18 and is digested and verified in §17 · Task 0.9, so what is missing is the
~40 owner-written `expect` pairs, not the data. That run is what separates "the corpus describes the
wrong population" from "the field query file is needle-shaped", and it is the only measurement in this
project that can flip a shipped default. Then: the M1 hand review of the seat's moved rows, which the
floorless-audit caveat narrows but does not settle; re-anchoring queries 1–3 of the field file so they
do not encode last week's answer; the same grid re-run at the new δ, because the published fusion choice
was made while δ was 0.05 and the head the stage sees has changed since; and Task 1.5 if the stage ever
goes back on.

One caveat about `complete@5` here, because the ceiling argument above leans on it: the audit
applies no client floor — it prints `floor: clients drop below 0.25, this audit shows everything`.
So 88.9% is what the pipeline *recalled*, not what a client would have been served, and a caller
asking these same eighteen questions could abstain on some of them. That softens "there was nothing
for the seat to gain" into "there was nothing inside the window for the seat to gain, and a row the
floor refuses is unreachable by the slot anyway" — which is still the right design (the slot runs
after the floor, deliberately), but it is a narrower claim than the table reads.

### Phase 4 — embedding prompts

**No measurement taken; the phase is stopped as written.** Nothing this project has measured runs a
non-default embedder — the corpus and the field host both use `bge-small-en-v1.5` — so the with/without
table this section asked for has no left-hand column to compare against. `calibrate` and the per-model
baseline map from Phase 3 are the shipped mitigation. Reopening condition: a second embedder actually
deployed, with a query set it loses. §11's status note, and
[`recall-quality-status.md`](recall-quality-status.md) §5.

### Phase 5 — question probes

**No measurement taken; the phase is open and unfunded.** It is the only later phase whose failure class
is still failing on the corpus — `vocab-gap` reads **recall 14.3% / p@1 0.0%** in the 2026-10-18 eval
run, one of six cases answered — and still has no field observation, because the 18 owner-written
questions contain no vocabulary-gap question. Bench p95, ingest token cost and the field audit are
therefore all still owed, and an LLM at write time should not be bought on a synthetic class. Order:
the frozen-copy query set (§16, status §6 item 2) decides whether the class is real, then this phase.

### Phase 6 — aliases

**No measurement taken; the phase is stopped as written.** No τ was calibrated because no query on the
field store lost an answer to a split concept (280 entities, 61 relations, 182 active memories, §17 ·
Field host), and the one graph mechanism already shipped costs MRR on that same store. Reopening
condition: a query that fails because two spellings of one thing are two nodes. §13's status note.

### Phase 7 — temporal cues

**No measurement taken; the phase is stopped as written, with the weakness recorded.** `temporal` reads
p@1 **0.0%** over its five corpus cases, so the shape is real in the corpus; no real question of that
kind has been asked against a store that has 43 superseded rows. Note for whoever does measure it: that
kind is marked informational, so it is **outside** the gated δ = 0.5 comparison — the affinity change's
effect on temporal ranking is unmeasured, not benign. §14's status note.

### Phase 8 — release

**In progress, and smaller than planned.** Defaults are set from measurements:
`[retrieval].relevance_weight` **0.5** (both instruments, no rank-1 loss at any value, #40);
`[rerank].mode` **off** (the stage changes ~nothing fused at weight 1 and costs 236 ms p50 and 1.67 GB);
`[graph].expansion_slots` **1** (top-1 unharmed, −2.2 MRR on the field store, revisit with a seat-aware
query set). Smoke test still owed. The one open default — `strategy = "rrf"` versus `replace`, or a
confidence gate between them — is explicitly **not** this phase's to guess at; it belongs to the
frozen-copy rerank grid (§16 · follow-ups, [`recall-quality-status.md`](recall-quality-status.md) §4).

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
