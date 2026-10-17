# Recall accuracy without the speed cost — findings and proposal

**2026-10-17 · against `main` @ `bfe1d57` (v0.4.3) · companion to [reranking-plan.md](reranking-plan.md)**

Line references below are to `main` at that commit.

## TL;DR

- **The reranker works as specified, but it is the wrong tool for this system's errors.**
  On the real store it changed **0 of 13** top results (p@1 72.73 → 72.73, MRR +1.06 —
  one query), for 400–1200 ms per recall (reranking-plan §12, "first audit against a
  real store"). Recall without it is ~8–25 ms. Keep `[rerank].mode = "off"`.
- **Two zero-latency changes, measured on the same eval set the reranker was tuned on,
  beat it on MRR and recall:**

  | eval (108 memories, 59 cases, bge-small) | recall@5 | p@1 | MRR | relational recall | p95 |
  |---|---|---|---|---|---|
  | today | 83.0 | 65.4 | 74.6 | 57.1 | 9.2 ms |
  | reranker, Phase 7 winner (idle box) | 83.3 | **71.2** | 77.2 | — | +70.6 ms |
  | **relevance tie-break + graph expansion slot** | **86.9** | 69.2 | **78.4** | **85.7** | 9.1 ms |

  Abstention stays 100%, and no kind's recall drops. The one top-1 loss is a case today's
  build wins by id order (§2, experiment 3).

  > **Caveat, found while planning:** the eval seeds every memory within milliseconds,
  > so the recency band is the same for all of them and symmetric rank disagreements
  > become *exact* ties. In a real store, ages differ and recency usually settles those
  > pairs first. The tie-break's p@1/MRR gain is therefore an upper bound until it is
  > re-measured on an age-aware eval. The graph slot's gain is structural and does not
  > depend on ages. The implementation plan,
  > [recall-quality-plan.md](recall-quality-plan.md), handles this in its Phase 0
  > (age-aware eval) and Phase 2 (a bounded relevance multiplier instead of a pure
  > tie-break).
- **Defects found:** embedders ignore query/document mode for every provider except
  Gemini (E1). A timed-out rerank keeps running and blocks the next one (R1). The graph
  leg cannot rank a relational answer into the top 5 by construction (G1).

---

## 1. Is the current implementation correct?

| # | Area | Verdict | Evidence |
|---|---|---|---|
| R0 | Reranker mechanics | **Correct as specified.** Aligned scores, sigmoid squash, deadline fallback, trace. | Reviewed through Phases 4–8; the field fixes (v0.4.1–0.4.3) were reporting gaps, not logic errors. |
| R1 | Rerank deadline | **Defect.** On timeout the scoring thread is detached and keeps running (`memory_recaller.rs:630,639`). With the shipped `max_concurrency = 1` (`config.rs:555`) it keeps holding the only session, and the next call parks on it (`fastembed_reranker.rs:213`). So `took_ms` includes queueing behind work nobody will read, and the CPU spends that work on a box that also serves the LLM and embedder. Agents issue parallel recalls (Claude Code does), which compounds it. | Plausible contributor to the unexplained 34 → 120 ms/pair drift (plan §12, part three), **not proven**. Test: fire 3 concurrent recalls with `mode = "on"` and log session-acquire wait separately from inference. |
| R2 | Reranker usefulness | **Structurally limited**, independent of R1. (a) It only reorders the post-floor head (4–5 items live). (b) `rrf` weight 1 cannot overrule vector+BM25 agreement, and weights ≥ 2 measured worse. (c) bge-reranker-base is trained on question→passage; **27 of 52** correct pairs score at or below the negative-noise ceiling (Phase 7.4). (d) 278M params on CPU: ~7 ms/pair idle, 20–120 ms contended. | Real-store audit: `top-1 changed 0/13`. |
| E1 | Query vs document embedding | **Defect.** `EmbeddingTask` is ignored by `fastembed_embedder.rs:78` and `remote_embedder.rs:147`. The trait doc's premise that the local and OpenAI-compatible models are "symmetric" (`embedder.rs:16`) is wrong for most models served that way. EmbeddingGemma prescribes `task: search result \| query: …` / `title: none \| text: …`, nomic-embed `search_query:` / `search_document:`, e5 `query:` / `passage:`. The field host ran `embeddinggemma-300M` without them. | Model cards (sources). **For bge-small, the documented query instruction measured *negative* here** (p@1 −1.9; experiment 1). It is meant for short-query → long-passage retrieval, and these memories are one sentence. Do not apply it to bge-small. |
| F1 | Fusion | **Design limit.** RRF uses ranks only. When vector and BM25 disagree symmetrically (v1/k2 vs v2/k1), the two candidates get *identical* scores, and recency/id decides (`recall_ranker.rs:582–590`). The calibrated match strength computed right after fusion (`recall_ranker.rs:779`) never takes part. | 3 of 52 eval top-1 misses are exact ties where the correct memory has the higher relevance (0.673 vs 0.659, 0.708 vs 0.649, 0.742 vs 0.725). |
| G1 | Graph leg ("concept relationships") | **Design limit.** Seeds are exact canonical-name matches of query word-windows (`sqlite_entity_graph.rs:282`). Hits are ordered by hop distance then edge count (`:387`), and damped (`rank_weight 0.3`, `unanchored_floor 3`, `config.rs:424–425`). A graph-only answer is found by one leg, so its RRF score is at most 1/61 ≈ 0.0164, which loses to **any** memory both vector and BM25 returned in their top ~60. A hop-2 answer ("Nadia is the tech lead of the Meridian team" for "who owns decisions about the billing service") shares no words with the query, so it can never reach the top 5. | Relational recall 57.1%. Turning the graph *up* lowers precision (experiment 4), so this needs structure, not weight. |
| G2 | Graph-only relevance | **Latent hazard.** Graph-only hits get a constant relevance of 0.35 (`recall_ranker.rs:630`), above the 0.25 floor. That is safe only while seeds are exact names: any fuzzy or semantic seeding without a threshold puts noise past the floor. | Measured: unthresholded triple seeding took abstention from 100% to 0% (experiment 5). |
| V1 | Eval set | **Hygiene.** `eval/cases.toml` defines 4 memories twice (108 rows, 104 distinct), so duplicates take top-5 slots. Relational `p@1` also scores the wrong thing for multi-hop questions: the bridge fact *should* rank first and the answer next to it. | `pnpm`, barrel files, table-driven Go tests, Vitest. |

## 2. What was measured

All runs use `eval` on `main` @ `bfe1d57`, default config (bge-small-en-v1.5, the same embedder the
field host runs now), no reranker, in the dev container. Every variant is env-gated in
[`experiments/recall-accuracy-2026-10-17.patch`](experiments/recall-accuracy-2026-10-17.patch),
so the control run is byte-identical to `main`. The control reproduces the committed
baseline exactly.

| # | Experiment | recall@5 | p@1 | MRR | abst. | Verdict |
|---|---|---|---|---|---|---|
| 0 | control | 83.0 | 65.4 | 74.6 | 100 | — |
| 1 | bge query instruction (`RA_EXP_QUERY_PREFIX`) | 83.3 | 63.5 | 73.1 | 100 | ✗ needle −33, relational −14 |
| 2 | contextual embedding: category/tags/entities prepended (`RA_EXP_CONTEXT=full`) | 81.7 | 63.5 | 72.3 | 85.7 | ✗ a shared prefix makes one-sentence memories look alike; a negative leaks |
| 3 | **RRF ties broken by relevance** (`RA_EXP_FUSE=tiebreak`) | 83.0 | **69.2** | **76.5** | 100 | ✓ nl-question p@1 42.9 → 71.4, statement 50 → 75; relational −14.3 (one case, below) |
| 3b | relevance blended into the score, λ ∈ {0.001 … 0.03} | 82.4–83.0 | 67.3–69.2 | 74.6–76.4 | 100 | ≈ / worse than 3: the signal is "decide ties", not "re-weight" |
| 4 | 3 + graph `rank_weight` 0.6 / 1.0, `unanchored_floor` 0 / 3 | 83.0–84.9 | 67.3–69.2 | 75.5–76.2 | 100 | ✗ relational recall ↑ (71) but p@1 → 0: more weight promotes the bridge memories, not the answers |
| 5 | 3 + HippoRAG-style query-to-triple seeds, unthresholded | 83.0–83.7 | 55.8–63.5 | 70.1–73.9 | **0** | ✗ every negative leaks (G2) |
| 5b | 5 with a 0.72 similarity threshold + hits ordered by best-matching relation | 83.0–84.9 | 63.5–69.2 | 74.6–76.5 | 100 | ≈ 3: seeding was not the bottleneck; ranking (G1) is |
| 6 | **3 + graph expansion slot** (`RA_EXP_GRAPH_SLOT=1`) | **86.9** | **69.2** | **78.4** | 100 | ✓ relational recall 57.1 → 85.7; no kind's recall drops; p95 9.1 ms |
| 6b | 6 with query-aware graph ordering / triple seeds | 86.9 | 67.3–69.2 | 77.1–78.4 | 100 | no gain over 6. Keep the simple one |

Experiment 6 vs control, top-1 changes: **+3** (`auth review`, `the failed migration`,
`hand-run migration`) and **−1** (`what a pipeline ran on before`). In that case the Kafka
(*before*) and Kinesis (*now*) memories tie exactly, and today's build wins only because the
id ordering happens to favour it. Relevance cannot encode "previously". The bi-temporal graph
can (follow-up in §3, step 3).

Reproduce:

```bash
git worktree add ../ra-exp bfe1d57 && cd ../ra-exp
git apply ../recuerdos-ai/ai-specs/experiments/recall-accuracy-2026-10-17.patch
touch .env   # compose's env_file; COMPOSE_PROJECT_NAME reuses the warm cargo/model volumes
COMPOSE_PROJECT_NAME=recuerdos-ai docker compose run --rm \
  -e RA_EXP_FUSE=tiebreak -e RA_EXP_GRAPH_SLOT=1 dev just eval-native
```

Not measured here: real-store effect (needs the field host), production embedders other than
bge-small, and §3 steps 4–6.

## 3. Proposal

Ordered by measured value per unit of risk. Every step adds **no model call at query
time**. Each is gated the way Phase 7 was (p@1 ≥ control, MRR no drop, recall@5 within 2,
no `by_kind` recall drop > 5, all negatives pass), plus a stricter latency budget:
**≤ +5 ms p95**.

### Step 1: stop paying for the reranker (config, today)

`[rerank].mode = "off"` and drop the `bake-rerank` image. That gives back 400–1200 ms per
recall and ~1.5 GiB RSS on the field host. The code stays compiled in. If anyone re-enables
it, fix R1 first: try-acquire a session and report `skipped{reason: "busy"}` instead of
queueing, and score in micro-batches so a deadline applies the part that finished rather
than discarding all of it.

### Step 2: relevance-aware tie-breaking in fusion (S, 0 ms)

After relevance is attached (`memory_recaller.rs:417–428`, before the floor), order by
`score`, then **relevance**, then the existing recency/id tie-break. Measured: **p@1 +3.8,
MRR +1.9**, recall unchanged. Document it in `recall_ranker.rs` in the calibrated style
(invariant I-3). It only decides exact ties, so it cannot reorder anything RRF already
separates. Anything heavier (convex-combination fusion, [Bruch et al.](https://arxiv.org/abs/2210.11934))
should go through the grid as its own experiment: the blends tried here did worse than the
pure tie-break.

### Step 3: graph as expansion, not as a competing ranker (S–M, 0 ms)

When the graph leg has seeds, take its best hit that fusion left outside the client's top
`limit`, and place it directly after result #1. Make it `[graph].expansion_slots = 1`
(0 = today's behaviour, byte-identical). This is the "concept relationships" fix: the bridge
fact ranks first, the related answer sits next to it, and an agent reads the chain. Measured
with step 2: **recall@5 +3.9, MRR +3.8, relational recall +28.6**. Do with it:

- Score relational cases by **chain recall** (bridge + answer in the top k), not p@1.
- Cue temporal phrasings ("before", "previously", "used to") to the graph's existing
  closed-interval edges. That is the one case step 2 loses.
- Keep G2 in mind: any future fuzzy seeding must use a threshold, or derive graph-only
  relevance from the path rather than a constant 0.35.

### Step 4: per-model embedding prompts (S, 0 ms; correctness)

Add an `EMBEDDING_PROMPTS` table keyed by `Embedder::model_id()`, mirroring
`SIMILARITY_BASELINES`, applied by both the fastembed and the remote embedder: EmbeddingGemma,
nomic-embed, e5 and Qwen3-Embedding get their documented prompts, while bge-small and OpenAI
get none (bge-small measured worse with its instruction). Fix the trait doc at `embedder.rs:16`.
A document-side prompt changes stored vectors, so the release notes need `reindex` plus
`calibrate`, and the baseline map needs re-measuring per model. This mostly matters for
deployments on Ollama, LM Studio or llama.cpp models, which is how the field host started.

### Step 5: ingest-time question probes (M, ~0–2 ms)

These target the misses no reorder can reach, because the right memory is never in the head:
`dietary requirement` → "vegetarian", `installing project dependencies` → pnpm, abstract
category queries. Extraction (already an LLM call at ingest) also emits 2–3 questions each
memory answers. They are embedded into a new `vec_memory_probes` vec0 table, with
vector-leg score = max(memory, its probes), and added to BM25 as a `questions` field. This is
the HyPE idea ([arXiv 2607.29402](https://arxiv.org/pdf/2607.29402)): it moves the
question/statement asymmetry, which the cross-encoder was paying to bridge per query, to
write time. The query-time cost is a KNN over ~3× rows, which stays flat to about 20k memories
(`docs/performance.md`). It is unavailable with `[understanding].provider = "none"`. **Not
measured here**: generating probes by hand would have leaked the eval's own queries. It needs
an LLM run, gated on `needle`, `nl-question` and `nl-category`.

### Step 6: entity resolution (M, 0 ms)

"Postgres" and "PostgreSQL" are separate nodes today (`entity_key.rs` lowercases and trims,
nothing more). At ingest, or in nightly consolidation, add `same_as` edges between entity
names whose embeddings exceed a per-model threshold (HippoRAG 2 used 0.8), optionally
confirmed by the understanding LLM, as Graphiti does. Add an `alias` eval kind first.

### Not recommended

| Option | Why not |
|---|---|
| A larger or different cross-encoder | Wrong failure class (R2). Phase 7 measured four models; the best gave +5.8 offline and 0 on the real store. |
| HyDE, LLM query rewriting, LLM reranking | An LLM call per recall: hundreds of ms to seconds. Contradicts the speed goal. |
| Personalized PageRank over the graph (HippoRAG 2) | Its gains come from query-to-triple seeding and graph-scored ranking. Here, seeding (exp. 5b) and graph weight (exp. 4) did not move precision, because RRF outvotes one-leg answers (G1). Revisit only if step 3 leaves relational chains incomplete. |
| Swapping the embedder first | Possibly useful, but it moves every calibrated number (baseline, floor, eval). Use `eval --config` and `calibrate` after steps 2–4, not before. |

## Sources

- BGE v1.5 query instruction and when it applies — [model card](https://huggingface.co/BAAI/bge-small-en-v1.5)
- EmbeddingGemma prompts — [model card](https://ai.google.dev/gemma/docs/embeddinggemma/model_card)
- Convex combination vs RRF — [Bruch, Gai, Ingber, *An Analysis of Fusion Functions for Hybrid Retrieval*](https://arxiv.org/abs/2210.11934)
- HippoRAG 2 (query-to-triple linking, PPR, ablations) — [arXiv 2502.14802](https://arxiv.org/html/2502.14802v2)
- Hypothetical Prompt Embeddings — [arXiv 2607.29402](https://arxiv.org/pdf/2607.29402)
- Graphiti hybrid search and rerankers — [Zep docs](https://help.getzep.com/graphiti/working-with-data/searching)
- Contextual retrieval (prepending context; 35–49% fewer failed retrievals on long chunks) — [summary](https://getcoai.com/news/anthropic-introduces-contextual-retrieval-to-boost-accuracy-of-rag-systems). It measured *negative* here on one-sentence memories (experiment 2).
