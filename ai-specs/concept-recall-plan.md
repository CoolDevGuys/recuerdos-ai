# Recuerdos AI — Concept Recall: broad questions should reach the whole topic

Opened **2026-10-27**. Owner question: *"I want to improve the memories relationship so recalling memories
improves when searching by concepts or semantics, like 'What is the user preferred tech stack?' — the
service should return memories related to programming or software engineering, testing, etc. If the
problem is the quality or content of the memories, what can we do — discard them, or re-store /
re-embed / re-classify?"*

This plan answers that question in four proposals, **A–D**, ordered cheapest first. **A is implemented
and measured** (§4). B–D are specified so they can be picked up without this conversation.

Companion documents: [`recall-quality-plan.md`](recall-quality-plan.md) (the previous plan; its Phase 4
and Phase 5 touch the same ground and are cross-referenced below) and
[`recall-quality-status.md`](recall-quality-status.md) (its state as of 2026-10-18).

---

## 1. What a "concept question" is, and why the existing numbers missed it

Every question set the project has measured so far — the eval's 74 cases, the field host's 18, the
2026-10-22 audit's 22 — has **one right memory** per question. They measure *ordering*: is that one
memory first? Broad questions are a different task. *"How does the user approach testing?"* has eight
right answers on the real store, and the useful outcome is **how many of them come back, and how much
of the top 5 is on-topic** — not which one is first.

So this plan adds a third question set (§4.2): **15 broad questions**, each with its expected *set* of
memories, scored as:

- **on-topic@5** — the share of the top 5 that belongs to the expected set (is what comes back about
  the topic?);
- **recall@k** — the share of the expected set reached within the top *k* (does the topic come back?).

## 2. Diagnosis: why broad questions underperform

Live recall on 2026-10-27 already surfaced the right *family* for three broad questions, with noise.
Reading the code and the store explains the gaps:

| # | Cause | Evidence | Fixed by |
|---|---|---|---|
| D1 | **Only the sentence was embedded.** A memory's category, sub-label and tags reached the BM25 leg (tantivy indexes them) but never the vector leg — the one that matches on meaning. | `DirectMemorySaver` embedded `memory.content()` only | **A** |
| D2 | **No query instruction.** bge-small v1.5 is trained with an optional query prefix; the local and OpenAI-compat embedders ignore `EmbeddingTask`, so queries and memories were embedded identically. | `fastembed_embedder.rs`, `remote_embedder.rs` | **A** (measured: not worth enabling for bge-small) |
| D3 | **The graph has no concept layer.** 183 entities, 58 relations for 179 active memories; the busiest entities are generic words (`code`, `tests`, `python`). Nothing says *pnpm → package manager → tooling → software engineering*. | 2026-10-22 audit | **B**, **C** |
| D4 | **Topic vocabulary gaps.** *"version control and code review"* never reaches memories filed under `git` — under any variant measured (§4.3). The model does not bridge the topic's name to its members' words. | §4.3 per-question table | **C** |
| D5 | **Multi-topic blobs and hub memories.** A few long `fact.project` memories cover four or five subjects in one vector and surface as weak noise for many questions; one short, generically-phrased preference ranks first for most "the user prefers…" questions. | §4.3 inspection | **D** |
| D6 | **The graph seat promotes generic arrivals.** Seat #2 holds a graph-only hit at relevance ≈0.2 in every variant measured. | §4.4 | config (`expansion_slots = 0`) |

## 3. Should the memories be discarded?

**No.** The memory *content* is the only source of truth; everything recall uses beyond it is derived and
rebuildable from the `memories` table, and the project already has the commands:

| Derived data | Rebuild with | Model calls | Daemon |
|---|---|---|---|
| Vectors | `recuerdos-ai reindex` (one transaction; re-pins) | embeddings | stopped |
| Keyword index | `recuerdos-ai reindex --text` | none | stopped |
| Graph: memory → entity edges | `recuerdos-ai graph backfill --entities` | none | running |
| Graph: relations | `recuerdos-ai graph backfill --relations` (budgeted, resumable) | LLM, one per memory without relations | running |
| Duplicates | `recuerdos-ai consolidate` | LLM per merge | running |
| Topics (proposal C) | `recuerdos-ai topics backfill` (new) | LLM, one per memory | running |

Discarding would lose the one thing that cannot be rebuilt. The fixes below re-derive; only **D**
touches content, and only for specific memories (split a blob, merge a duplicate).

---

## 4. Proposal A — embed the labels with the sentence (**implemented**)

### 4.1 What shipped

Branch `feat/enriched-embeddings`.

- **`[embeddings].document_text = "content" | "enriched"`** (default `content`).
  `memories::domain::document_text::DocumentText` renders the embedded text:

  ```text
  content:   Testing follows TDD methodology: …
  enriched:  Coding preference (testing, tdd). Testing follows TDD methodology: …
  ```

  Header = the category as words (`preference.coding` → *Coding preference*), then sub-label and tags in
  parentheses. Used by the saver, the updater, `reindex` and `calibrate`. Consolidation keeps comparing
  bare content on purpose: it clusters *within* one category, where a shared header would only inflate
  every pair's similarity and shift the merge threshold's meaning.
- **The format is part of the collection pin.** `enriched` pins `<model>+enriched-v1`; `content` keeps the
  bare model id, so every existing store opens unchanged. Switching is a `reindex` — the same path and
  the same startup refusal as a model switch. The suffix is versioned: a future header change is a new
  pin, never a silent mix.
- **Label edits re-embed under `enriched`.** A tag-only or category-only edit changes the embedded text,
  so `MemoryUpdater` rebuilds the vector; under `content` it still does not.
- **Its own similarity baseline.** The header lifts every cosine by ≈0.02 (noise ceiling 0.611 → 0.631,
  floor-gated answers 0.528 → 0.551). At the bare model's 0.5 one `negative` eval case cleared the 0.25
  floor (noise corrected to 0.263). `SIMILARITY_BASELINES` gained `bge-small-en-v1.5+enriched-v1 → 0.53`
  (noise corrects to 0.215, the same margin `content` has). The resolver looks up by pin, then falls back
  to the bare model's row with a `warn!`. **An explicit `[retrieval].similarity_baseline` still wins** —
  a config that pins `0.5` must move to `0.53` when it switches.
- **`[embeddings].query_instruction`** (default empty): `InstructedEmbedder` prepends it to `Query`
  embeddings only, for any provider. No pin change, no reindex. This is the query half of
  [`recall-quality-plan.md`](recall-quality-plan.md) Phase 4, now measured on bge-small (below).
- **`recall audit --compare-config` refuses a differing `document_text` or `query_instruction`.** Both
  columns share the `--config` side's embedder; the second instruction would have been silently ignored.
- **`calibrate` embeds the corpus as the saver would**, so its ceiling and floor are read on the text
  recall compares against.

### 4.2 How it was measured

- **Store:** the frozen copy of the field host's store (`eval/store-frozen/`, 222 memories, 179 active),
  copied twice into a scratch directory — never committed.
- **Embedder:** both copies re-embedded with the **local** bge-small (`reindex`), so the two columns
  differ in one thing only. Absolute numbers therefore differ slightly from the field host, which serves
  bge-small through an OpenAI-compatible server; relative differences are the finding.
- **Ranking:** the field host's own `base.toml` — δ 0.5, `expansion_slots` 1, rerank off — at
  `similarity_baseline` 0.5 for `content` and 0.53 for `enriched`.
- **Instrument:** `recall audit --limit 10 --json` (read-only, floorless), one run per config per set;
  scored offline by joining hit ids to the store.
- **Question sets:**
  - **concept** — 15 broad questions, 103 expected memories (§4.3), written for this plan;
  - **prefs** — the 2026-10-22 audit's 22 single-answer preference questions;
  - **field** — the owner's 18 single-answer questions (`eval/store-frozen/queries.toml`; 2 of them ask
    for memories that were never stored and always miss);
  - **eval** — `recuerdos-ai eval --baseline eval/baseline.json` (seeds its own store).

### 4.3 Results

| Configuration | concept on-topic@5 | concept recall@5 | concept recall@10 | prefs p@1 / MRR | field p@1 / MRR | p50 |
|---|---|---|---|---|---|---|
| `content` (today) | 36.0 | 25.1 | 44.7 | 54.5 / 72.5 | 50.0 / 63.4 | ≈8–10 ms |
| `content` + instruction | 34.7 | 24.1 | 40.9 | 59.1 / 75.0 | 55.6 / 66.2 | ≈9–12 ms |
| `enriched` @ 0.50 | 50.7 | 36.0 | 57.2 | 63.6 / 79.2 | 61.1 / 71.8 | ≈8–10 ms |
| **`enriched` @ 0.53** | **50.7** | **36.0** | **57.2** | **63.6 / 79.2** | **66.7 / 74.5** | ≈8–11 ms |
| `enriched` @ 0.53 + instruction | 49.3 | 35.4 | 58.1 | 63.6 / 79.2 | 66.7 / 74.5 | ≈9–12 ms |

Eval set (gate: `--max-drop 5`), against the committed baseline `67.3 p@1 / 75.9 MRR / 87.8 recall@5 /
84.6 complete@5`:

| Configuration | recall@5 | p@1 | MRR | complete@5 | abstention |
|---|---|---|---|---|---|
| `content` | 87.8 | 67.3 | 75.9 | 84.6 | 100% |
| `content` + instruction | 87.8 | 69.2 | 77.5 | 84.6 | 100% |
| `enriched` @ 0.50 | 88.5 | 71.2 | 79.2 | 86.5 | **85.7%** (one negative leaks) |
| **`enriched` @ 0.53** | **89.1** | **73.1** | **80.6** | **86.5** | **100%** |
| `enriched` @ 0.53 + instruction | 87.8 | 73.1 | 80.0 | 84.6 | 100% |

Single-answer movement, `content` → `enriched` @ 0.53: prefs **+2 first places, 0 lost, 4 improved,
0 worse** (the branch-naming question that missed in every 2026-10-22 config moves rank 8 → 2); field
**+3 first places, 0 lost, 4 improved, 0 worse**. On the eval, `nl-category` p@1 62.5 → 87.5 and
`vocab-gap` recall 14.3 → 28.6.

Per concept question (`content` → `enriched` @ 0.53):

| Question | on-topic in top 5 | expected reached in top 10 |
|---|---|---|
| What is the user's preferred tech stack? | 3 → **1** | 3/7 → **2/7** |
| How does the user approach software testing? | 3 → 3 | 6/8 → 7/8 |
| What are the user's version control and code review habits? | 0 → 0 | 0/6 → 1/6 |
| Which security practices does the user insist on? | 3 → 4 | 6/9 → 7/9 |
| What are the user's UI and accessibility standards? | 2 → 4 | 4/8 → 7/8 |
| How does the user like backend services to be structured? | 2 → 3 | 3/10 → 3/10 |
| What domain-driven design conventions does the user follow? | 2 → 4 | 4/8 → 4/8 |
| How should frontend code be organized for this user? | 2 → 2 | 2/6 → 3/6 |
| What Python tooling and conventions does the user use? | 1 → 3 | 2/7 → 5/7 |
| What is the user's professional background? | 2 → 4 | 3/6 → 5/6 |
| When does the user consider a change finished? | 0 → 1 | 1/5 → 1/5 |
| What naming conventions does the user follow in code? | 2 → 3 | 4/7 → 5/7 |
| What does the user think about code cleanliness? | 4 → 4 | 5/6 → 5/6 |
| How strict is the user about static typing? | 0 → 1 | 2/4 → 2/4 |
| What does the user know about PHP tooling and Symfony? | 1 → 1 | 2/6 → 3/6 |

### 4.4 What the results say

1. **`enriched` is the win, and it is free at recall time.** +14.7 on-topic@5, +12.5 recall@10 on broad
   questions; +9.1 / +16.7 p@1 on the two real single-answer sets; +5.8 p@1 on the eval; no question
   ranked worse anywhere; latency unchanged (the header is embedded once, at save).
2. **The query instruction is not worth enabling for bge-small.** It helps bare `content` vectors a
   little, adds nothing on top of `enriched` for single answers, and lowers broad-question on-topic@5
   and the eval's recall. The knob stays, off, for models that require one (E5's `query: `).
3. **The baseline row matters.** At 0.5, `enriched` leaks one negative eval case and reads 5.6 points
   lower p@1 on the field set than at 0.53.
4. **One question got worse: "tech stack".** Under `enriched`, the *Coding preference* header pulls
   every coding preference toward a question phrased "the user's preferred…", crowding out the two
   *Fact about a person* memories that actually list languages and infrastructure. This is the limit of
   A: it makes a memory's *filing* visible, and "tech stack" is a concept that cuts across filings.
   That is C's job, not a header-wording tweak (tuning the header on one question would be fitting the
   test).
5. **The graph seat is still neutral-to-negative.** With seat 0 under `enriched` @ 0.53: concept
   recall@10 57.2 → 58.3, prefs MRR 79.2 → 79.5, field MRR 74.5 → 76.9, top-1 unchanged everywhere.
   Same direction as the 2026-10-22 audit.
6. **What A cannot do** is the "version control" row: zero on-topic in every variant. The memories are
   tagged `git`; the question says "version control and code review". No embedding of the memory's own
   words bridges that — a topic with a description does (C).

### 4.5 Rolling A out on the field host

```bash
# 1. stop the daemon
# 2. in recuerdos-ai.toml:
#      [embeddings]   document_text = "enriched"
#      [retrieval]    similarity_baseline = 0.53   # the field config pins 0.5 explicitly
#      [graph]        expansion_slots = 0           # optional, see 4.4 item 5
recuerdos-ai reindex          # re-embeds every memory; one transaction; ~seconds at this size
# 3. start the daemon; `recuerdos-ai config` must print `document_text enriched` and
#    `similarity_baseline 0.53` — a build without A prints neither line. Then re-read the field set:
scripts/field-measure.sh      # expect field p@1 ≈ 66.7 (local-embedder reading; ±1 question)
```

Rollback is the same three steps with `content` and `0.5`.

**Default.** `content` stays the default in this branch: flipping it makes every existing store refuse to
start until `reindex`, which is a release-note decision, not a code one. Recommendation: flip it for
`v0.5.0` with that note, after the field host confirms §4.3 on its own embedder.

### 4.6 Definition of Done (A)

- [x] `DocumentText` with `content` / `enriched`; saver, updater, reindex and calibrate use it.
- [x] Format in the pin; mismatched store refuses to start and names `reindex`.
- [x] Enriched baseline row measured with `calibrate`; resolver looks up by pin.
- [x] `query_instruction` wrapper; audit refuses mismatched embedder knobs.
- [x] Unit tests: rendering, pin, parse, saver vector, updater re-embed rules, reindex pin + vector,
      instruction wrapper, config validation, baseline resolution, audit refusals. Full suite green.
- [x] Eval: no gated metric drops; abstention 100%.
- [x] Docs: `docs/configuration.md`, `recuerdos-ai.example.toml`, CHANGELOG.
- [x] Field host: deployed (v0.5.0-rc3 build, 2026-10-28) and `field-measure.sh` recorded in §9: p@1 55.6 → 61.1, MRR 65.3 → 69.4, nothing worse.
- [ ] Decide the `v0.5.0` default.

---

## 5. Proposal B — fill in the missing relations (no code)

**Goal.** Give the graph something to traverse: 58 relations for 179 memories means most memories are
islands.

**What exists.** `recuerdos-ai graph backfill --relations` re-extracts relations for memories that have
none, one LLM call each, bounded by `[graph].backfill_budget`, resuming from a per-user watermark;
relations are anchored to the memory's stored entities.

**Tasks.**

- **B.1** Dry run on the frozen copy: `graph backfill --relations --dry-run` → how many memories, how many
  calls.
- **B.2** Run it on the frozen copy with the field host's extraction model; record relations before/after
  and the degree distribution of entities.
- **B.3** Re-measure the three store sets with `expansion_slots` 0 and 1. The seat is only worth keeping
  if a denser graph turns its arrivals on-topic.
- **B.4** If generic hubs (`code`, `tests`) still dominate: down-weight a seed entity by its degree
  (IDF-style) in `sqlite_entity_graph.rs` before expansion — a separate, measured change.

**Gate.** Keep the backfilled graph if concept recall@10 or field MRR rises with the seat on; otherwise
the finding is "relations don't help this store" and `expansion_slots = 0` becomes the recommendation.

**Expectation, stated honestly:** B is cheap to try and unlikely to fix D4. Extracted relations are
between *named things* in one memory (`project uses tool`); they do not create the cross-memory concept
links a broad question needs.

---

## 6. Proposal C — a topic layer (the real fix for concept questions)

**Goal.** A broad question names a *topic* ("testing", "version control", "tech stack"). Give memories
topics, give topics descriptions, and let recall reach a topic's members even when no member shares the
question's words.

### 6.1 Design

- **A small, curated topic tree** — shipped as data, extensible by config like
  `[understanding.taxonomy].extra_categories`:

  ```text
  software-engineering
    ├─ languages-and-stack     "programming languages, frameworks, tech stack, runtimes"
    ├─ testing                 "tests, TDD, coverage, mocks, fakes, test naming"
    ├─ version-control         "git, branches, commits, pull requests, code review, merging, CI"
    ├─ architecture            "DDD, hexagonal, layers, bounded contexts, use cases, folders"
    ├─ security                "authentication, authorization, tokens, secrets, hashing, input validation"
    ├─ frontend-and-ui         "UI, accessibility, CSS, design tokens, forms, components, state"
    ├─ tooling                 "package managers, linters, formatters, type checkers, build tools"
    └─ code-quality            "naming, dead code, small changes, clarity, typing strictness"
  career                       "jobs, employers, education, skills, experience"
  personal                     "family, location, communication style, preferences outside work"
  projects/<name>              derived from project entities, one node per project
  ```

  Each node has an id, a parent, and a one-line description. The description is what makes D4 solvable:
  *"version control"* matches the description of `version-control`, whose members are the `git`
  memories.
- **Assignment at ingest.** The extraction call (already one LLM call per save) returns 1–3 topic ids
  from the tree, validated against it. No extra call.
- **Storage.** Migration `V10__memory_topics.sql`: `topics(id, parent_id, description)` and
  `memory_topics(memory_id, topic_id)`. Topic descriptions are embedded once into a small vector table
  (a dozen rows — exact KNN, microseconds).
- **Recall.**
  1. Embed the query (already done).
  2. Match it against topic-description vectors; a topic is *hit* when its relevance clears a calibrated
     threshold (measured with `calibrate`, like the memory baseline).
  3. **Expand:** add the hit topic's members (and its children's) as candidates, ranked within the
     topic by their own relevance — a third RRF leg, `rrf_topic`, alongside vector and BM25.
  4. Cap the leg's contribution so a single-answer question is never displaced (the δ affinity and the
     relevance floor already guard this; measure it).
- **Optional, later:** put the topic names into A's header (`Coding preference · testing (tdd)`) — a
  `+enriched-v2` pin — once topics exist.

### 6.2 Tasks

- **C.0 Instrument first.** `recall audit` query files accept `expect_any = [...]` (a list); the report
  gains on-topic@k and recall@k next to p@1/MRR. Commit the *question texts* of §4.3's concept set as
  `eval/concept-queries.example.toml` (expectations stay private, with the store). Without this, C cannot
  be gated.
- **C.1 Topic tree.** `memories::domain::topic` (id, parent, description), default tree, config
  extension, validation (unique ids, acyclic, known parents).
- **C.2 Schema + repository.** V10 migration; `TopicRepository` port + SQLite adapter; export/import
  carry topics.
- **C.3 Ingest.** Extraction prompt + candidate schema gain `topics`; the reconciler validates them
  against the tree; `memory_save` (direct) accepts optional `topics`.
- **C.4 Backfill.** `recuerdos-ai topics backfill` — budgeted, resumable, `--dry-run`, same shape as
  `graph backfill --relations` (reuse `ConsolidationBudget` and the watermark state).
- **C.5 Recall leg.** Topic-description vectors, topic hit threshold (calibrated), `rrf_topic` leg in
  `RecallRanker`, `[retrieval].topic_weight` (0 = off, the shipped default until gated).
- **C.6 Measure** on the frozen copy: concept set (on-topic@5, recall@10), prefs, field, eval.

### 6.3 Gate

Ship `topic_weight > 0` as default only if, against `enriched` @ 0.53 (§4.3):
concept on-topic@5 **+10 or more**; the "version control" and "tech stack" rows each gain;
prefs and field p@1 **do not drop** (one question = ±4.5 / ±5.6 points — any drop is a named question
to read); eval gate passes; recall p50 rises **< 2 ms**.

### 6.4 Relation to the previous plan

- [`recall-quality-plan.md`](recall-quality-plan.md) **Phase 5 (question probes)** attacks vocabulary
  gaps one memory at a time (3 generated questions per memory, a second vector index). C attacks them one
  *topic* at a time (one description per topic, a dozen vectors). C is cheaper per memory and targets
  broad questions; probes target specific ones. Do C first; reopen Phase 5 only if specific-question
  vocab gaps survive it.
- **Phase 6 (entity aliases)** stays stopped: no measured question was hurt by a split entity.

### 6.5 Risks

- **Topic drift.** An LLM labelling at ingest will be inconsistent. Mitigations: a closed tree with
  descriptions in the prompt; topics validated against it; `topics backfill --rebuild` to relabel
  everything after a tree change.
- **Over-expansion.** A hit topic with 70 members would flood the list. The leg is rank-fused and capped
  (C.5); measure with on-topic@5, which falls if expansion floods.
- **Tree fit.** The default tree is written from one person's store. Extensible by config; measured on
  that store only — say so in the docs.

---

## 7. Proposal D — curate the content that hurts recall

Cheap, manual or one LLM pass, and the only proposal that edits memories.

- **D.1 Split multi-topic blobs.** Memories over ~400 characters covering several subjects (the long
  `fact.project` "gotchas"/"audit" summaries) become one memory per subject. A one-shot LLM job proposes
  the split, the owner approves, the originals are superseded (not deleted), so history is kept.
- **D.2 Rephrase hub memories.** Very short, generically phrased memories ("The user prefers X") match
  every "the user's preferred…" question. Make them specific ("Uses pnpm, not npm or yarn, for
  JavaScript package management") — `PATCH /v1/memories/{id}` re-embeds them.
- **D.3 Merge duplicates.** `recuerdos-ai consolidate --dry-run`, then run it.
- **D.4 Expire stale project facts.** Project status snapshots older than their project's last decision
  get `expires_at` or are forgotten.

**Gate.** Re-run the three store sets after each step; keep a step only if no set drops.

---

## 8. Order of work

| Step | What | Cost | Status |
|---|---|---|---|
| 1 | **A** — enriched vectors (+ instruction knob) | S | **done**, measured (§4) |
| 2 | Field host: deploy A, `expansion_slots = 0`, record field reading | config + reindex | **A done** (§9); seat still 1 |
| 3 | **C.0** — multi-expectation audit + committed concept question texts | S | next |
| 4 | **B** — relation backfill on the frozen copy, re-measure the seat | no code, LLM calls | next |
| 5 | **C.1–C.6** — topic layer | M–L | after 3 |
| 6 | **D** — curation passes | manual / small | any time; re-measure after |

---

## 9. Results log

| Date | What | Numbers | Where |
|---|---|---|---|
| 2026-10-27 | A measured on the frozen store copy, local bge-small, 4 configs × 3 sets + eval | §4.3 | this file |
| 2026-10-27 | Enriched baseline calibrated: noise ceiling 0.631, floor-gated signal 0.551 → 0.53 | §4.1 | `recall_ranker.rs` `SIMILARITY_BASELINES` |
| 2026-10-27 | Field host, first attempt: config set `document_text = "enriched"`, `similarity_baseline = 0.53`, but the binary was the published **`0.5.0-rc1`**, which predates A and ignores the key — so this reads content vectors at the enriched baseline, **not A**. Field set: p@1 55.6, MRR 69.0 seat off / 65.3 seat on; rerank on 50.0 / 61.1 at 299 ms p50. Live MCP showed the signature: same order as before, every relevance ≈0.02–0.03 lower, one broad question falling under the floor. | not a measurement of A | `eval/field-20261027-0100/` (not committed) |
| 2026-10-28 | **Field host after A** — rc3 build (reports `0.5.0-rc1`: `Cargo.toml` was not bumped; `config` prints `document_text enriched`, `similarity_baseline 0.53 (explicit)`). Same 18 questions vs the 2026-10-27 run (old code, same baseline): seat on **p@1 55.6 → 61.1, MRR 65.3 → 69.4**; seat off 55.6 → 61.1, 69.0 → 72.7; complete@5 88.9 both (the two misses are memories never stored). Moves: Python-tools question 3 → 1, linkedin auth 4 → 3; none worse. Recall p50 6.4 ms, RSS 78 MB. One question short of the frozen-copy forecast (66.7 / 74.5): the host serves bge-small through llama.cpp, and the store has grown since the copy. | A confirmed on the field host | `eval/field-20261028-0239/` (not committed) |
| 2026-10-28 | Graph seat on the enriched store: top-1 unchanged, MRR 72.7 → 69.4 with it on — four answers pushed down one place for a graph arrival. Fourth reading in the same direction. | recommend `expansion_slots = 0` | same bundle, `m1-slot` |
| 2026-10-28 | Rerank on the enriched store, depth 10: shipped fusion (`rrf`, weight 1) p@1 **77.8** / MRR 81.5 vs off 61.1 / 69.4; `replace` 83.3 / 85.2 (was 66.7 vs 83.3 on content vectors). Cost p50 ≈600 ms, p95 ≈1.2 s — at the 1200 ms timeout. Recorded, not shipped. | open | same bundle, `m5-*` |
| 2026-10-28 | Bundle hygiene: `m3-rerank-off-vs-on.txt` in both field folders is the 2026-10-18 file (byte-identical; M3 is skipped with rerank off). `field-measure.sh` now clears old results first. | — | `scripts/field-measure.sh` |
