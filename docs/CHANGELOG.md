# Changelog

Format loosely follows [Keep a Changelog](https://keepachangelog.com/).
One entry per phase, backfilled as phases land.

## v0.4.0 — Recall quality, and an optional cross-encoder reranker

Two measured recall defects are fixed, and a second stage that had been
built and rejected once before is added — this time with the measurements
that justify it.

A cross-encoder reads the query and one candidate *together*, which no
retrieval leg ever does. It was tried during the graph work and reverted:
neutral on a small corpus, a second gigabyte on the hot path. Before
trusting it again we fixed what made the fused list wrong in the first
place, grew the eval set to 108 memories and 59 cases, and ran 81 grid
cells — four models, two strategies, three depths, four weights. It earned
**+5.8 precision@1**. It also costs 1.1 GB of weights and +71 ms at p95, so
it ships **off by default**, with the tools to judge it against your own
memories before you spend either.

- **The keyword leg stopped matching stopwords.** "what do I use for data
  storage" matched any memory containing *the*, *do* or *use* — BM25 was
  indexing question words as if they carried meaning, which inflated noise
  into the top ranks. The analyzer now drops the Lucene English list plus
  question words, auxiliaries and pronouns, and deliberately keeps `use`,
  `user` and `prefer`, which are words people search for in a technical
  store. Measured on the committed corpus: `precision@1` **62.5% → 65.4%**,
  and abstention on questions the store cannot answer **25% → 75%** — a
  memory service that always answers something is one you stop trusting.
  The keyword index is versioned, so the first start after the upgrade
  rebuilds it from the `memories` table: no model calls, seconds at personal
  scale, and `reindex --text` does the same by hand
  ([deployment.md](deployment.md#6-upgrade)).
- **Relevance is calibrated per embedding model.** Relevance subtracts the
  cosine at which a model says nothing before rescaling, and that floor is a
  property of the model — it differs by more than **0.15** between the two
  shipped embedders. With one constant (0.5, measured against the default
  `bge-small-en-v1.5`), switching `[embeddings].model` silently miscalibrated
  every relevance number: either noise sailed past the floor or real answers
  were subtracted away. `[retrieval].similarity_baseline` now defaults from a
  measured per-model map (`gemini-embedding-2` → 0.46); a model with no
  measured entry falls back to 0.5 **with a warning at startup**, and
  `recuerdos-ai calibrate` produces the number. *Behaviour change if you use a
  hosted embedder: the relevance numbers move.*
- **`[rerank]` — an optional second stage, off by default.** `mode = "off" |
  "shadow" | "on"`. `shadow` scores every real recall and logs what *would*
  have changed (ids and ranks, never content) while serving the old order —
  how the stage is judged on traffic you didn't have to invent; `on` serves
  the reranked order. The tuned default (`bge-reranker-base`, fused as an
  RRF leg at weight 1.0, depth 10) measures **+5.8 precision@1, +2.7 MRR,
  +0.3 recall@5** with no case kind regressing, for **+71 ms p95** added to a
  recall, 1.1 GB of weights and ~1.5 GiB RAM per inference. `off` is a hard
  no-op: nothing loads, nothing downloads, the release image grows by nothing,
  and recall is byte-identical to a build without the feature. The model is
  English; the two multilingual candidates are measured and costed in
  [configuration.md](configuration.md#if-a-model-is-too-big-for-your-box).
- **`rerank audit` judges it on your store, not on our corpus.** It replays
  your queries over your real memories twice — with and without the stage —
  and prints both orderings, how often the head moved and what it cost. It
  writes nothing, so the daemon can stay running, and it is the only
  supported way to run the stage over an existing corpus: there is
  deliberately no "rerank everything" command, because a rerank score is a
  property of a query against a candidate list, not of a stored memory
  ([evaluation.md](evaluation.md)).
- **A slow reranker can cost latency, never a request.** Past `timeout_ms`
  (default 250, ~3.5× the tuned model's measured p95) the stage is dropped
  and the fused order is served with a warning. `max_concurrency` (default 1)
  bounds how many copies of the model your box holds, because each inference
  is another resident copy of the weights.
- **`matched.rerank_rank` and `rerank_score`** appear in a recall result only
  when the stage is on, so you can see which memories the second opinion
  promoted. Nothing a client has to send changed; the Python SDK gained the
  two optional fields.
- **Fixed: Gemini embedding batches larger than the API's cap.**
  `batchEmbedContents` accepts at most 100 inputs, so a `reindex` or a
  consolidation pass over a store of any size failed with HTTP 400. The
  adapter now chunks the input and preserves order.
- **Getting the weights where they need to be.** `warm-models` downloads the
  rerank model too when the config enables one, and the image can bake it in:
  `docker build --build-arg RERANK_MODEL=bge-reranker-base`. The default image
  carries no rerank model, since the default does not use one.

## Unreleased — the field host answered the questions rc1 asked, and one answer changes a default

Deployed measurements are in. [`recall-quality-plan.md`](../ai-specs/recall-quality-plan.md) §17 now
carries the first readings taken against a live personal store running `v0.5.0-rc1`: 18 questions
written by the person whose memories they are, replayed over that store by
[`scripts/field-measure.sh`](../scripts/field-measure.sh) with the rerank stage off, at four values of
one knob. Three of rc1's owed questions have answers, and one of them is a shipped default.

- **`[retrieval].relevance_weight` now defaults to `0.5`, not `0.05`.** rc1 asked whether 0.05 was
  right or too timid; it was timid. Against the baseline it replaces, same build: **precision@1
  53.8 → 67.3**, **MRR 68.4 → 75.9**, recall@5 87.2 → 87.8, `complete@5` and abstention unchanged, and
  **no case kind's precision@1 fell** — four rose (`near-miss` 57.1 → 100.0, `statement` 25.0 → 75.0,
  `needle` 33.3 → 66.7, `nl-category` 50.0 → 62.5). Against `0.0` on that corpus the ladder reads
  +17.3 precision@1 and climbs monotonically through 0.05 / 0.2 / 0.3 / 0.5. On the real store:
  precision@1 **27.8 → 50.0** and MRR **47.7 → 63.4** across δ = 0 / 0.05 / 0.2 / 0.5. On neither
  instrument, at no value, did anything holding first place lose it — which is the sentence that
  mattered, because "the eval rewards relevance by construction" was the only reason 0.05 shipped.
  *Behaviour change on upgrade*: the order you get back changes. `relevance_weight = 0.05` restores
  what rc1 shipped, `0.0` restores the pre-Phase-2 order, and the accepted range is unchanged.
- **The reserved graph seat costs a little here, and the mechanism is visible in the audit.** With
  the seat on, precision@1 held (38.9 → 38.9) while MRR fell 57.9 → 55.7, twice, because an arrival
  the graph alone found enters at rank 2 by design and the second stage never judges it. Not a
  regression against rc1 — a measured price on a store whose entity graph is denser than the eval's.
  The default stays where rc1 put it, with the number recorded next to it.
- **`[rerank].mode = "off"` was the right default for a reason we had not published.** Fused as
  shipped (`strategy = "rrf"`, `weight = 1`) the stage changed almost nothing while costing 236 ms p50
  and ~1.4 GiB resident. Re-fused as `strategy = "replace"` on the same 18 questions it reads
  **83.3 precision@1 / 85.2 MRR** against the shipped fusion's 38.9 / 54.6 — eight queries gained
  first place, none lost it. That is a finding, not a shipped default: the eval grid has `replace`
  *losing* up to −9.6 precision@1 on three of four models, and the same field run shows how both can
  be true (where the model has nothing to say, its head scores 0.000 and banded raw scores reorder
  noise). What separates those two readings is a frozen copy of a real store inside the harness, and
  that is now the project's next measurement rather than a new phase.

Nothing else moves: no migrations, no wire changes, no new downloads. Upgrading from `v0.5.0-rc1` to
a build containing this is a config-file question — the store is untouched, and both knobs above stay
reversible without another deploy.

## v0.5.0-rc1 — Three ordering changes, and the measurements they still owe

A **prerelease**, deliberately: `latest` is not moved, the Python SDK is not
published, and what this tag is for is measurement rather than adoption.

[recall-quality-plan.md](../ai-specs/recall-quality-plan.md) Phases 0–3 are merged.
Each one shipped with a §4 gate whose second half is *"and the same comparison on a
real store"* — and every one of those halves has been blocked on the same thing: the
deployed server runs `v0.4.3`, which predates all three, so it has neither
`recall audit --compare-config` nor the two knobs the audits are meant to compare.
This build puts them on the same box as the store so the owed numbers can be read
off it. Until they are, treat what follows as *measured on the eval corpus, not yet
on your memories*.

- **`[retrieval].relevance_weight` = 0.05** (Phase 2, `v0.4.3..main`). After the
  index legs are fused, how well a memory matches the question now competes, a
  bounded amount, with how new it is: `score ×= 1 − δ·(1 − relevance)`. Before this,
  a memory matching the question *better* lost every close call to one three days
  newer. Eval: **precision@1 50.0 → 53.8**, MRR 63.9 → 66.6, recall@5 unchanged,
  abstention unchanged. `0.0` restores the old ordering exactly.
  [configuration.md](configuration.md#retrieval) explains why the shipped value is
  the smallest that clears the gate when the curve keeps climbing.
- **`[graph].expansion_slots` = 1** (Phase 3). A memory reached only by walking an
  entity relation brings one fusion term where a direct match brings two, so it
  cannot win a rank at any weight — now it reserves a *seat* instead: the
  best-connected memory you would otherwise not see is shown second, and your top
  result is untouched. Eval: **relational `complete@5` 57.1 → 85.7**, gated recall@5
  83.3 → 87.2, precision@1 unmoved. `0` gives back the `v0.4` pipeline, and nothing
  happens at all when a query names no entity the store knows — which, because seeds
  are exact, is most queries. One consequence to know about before it surprises you:
  a promoted row keeps its own `score`, so **the returned list is no longer sorted
  by score**, and a client that re-sorts by `score` undoes the feature.
- **A missed rerank deadline costs one chunk, never the next request** (Phase 1).
  The stage's model session used to be held by work that had already run out of
  budget, so one slow query could make the next one skip; the queue and the
  inference are now separately visible in the debug log and in `rerank audit`.

**Upgrade is ordinary and reversible.** No migrations landed since `v0.4.3`, so the
store file is untouched; pull the tag, recreate the container, and the volume stays
as it is. Rolling back is pulling `v0.4.3` again. If you want `v0.4`'s ordering while
keeping the tooling, set `relevance_weight = 0.0` and `expansion_slots = 0` — both
changes are config-reversible without another deploy.

What this tag is owed, and what your numbers will decide: whether 0.05 is the right
weight or too timid (the eval rewards relevance partly by construction, so a real
store is the falsifier); whether one reserved seat is the right seat count on a store
with thousands of entities rather than the eval's eighteen; and whether a hop ever
places something a human would call unrelated. The commands are in
[plan §17](../ai-specs/recall-quality-plan.md#17-results-log), and the findings go back
into that log before Phase 4 starts.

> **Answered on 2026-10-18**, by the box this tag was cut for: 0.05 was timid and the default is now
> `0.5`; one seat is affordable but not free; and the rerank stage turned out to be muted by its
> fusion rather than wrong. See the Unreleased entry above and §17 for the numbers.

## v0.4.3 — A release to publish the Python SDK

Nothing server-side changed. This cut exists because `release.yml` publishes the
Python client when a tag goes out, and the SDK's `0.4.0` — merged after
`v0.4.2` — would otherwise have sat unreleased on `main`, unnoticed by anyone
installing from PyPI.

**The Python SDK goes to 0.4.0.** `search_report()` returns what a search
response says *above* the results: the hits, the server's own `took_ms`, the
relevance floor that produced the list, and what the optional rerank stage did —
`applied`, `shadow`, `not_needed`, or `skipped` with a reason, absent when no
stage is configured (including any server older than `v0.4.2`). `search()` is
now `search_report().hits`, unchanged for callers who only want memories. Two
gaps turned up while doing it: the client had no `min_relevance` parameter at
all, though `docs/api.md` tells callers to read the floor and retry at `0.0`
([sdk-python.md](sdk-python.md)); and `recuerdos_ai.__version__` reported
`0.2.0` for two releases whose packaging said `0.3.0`, now pinned against the
installed distribution so it cannot drift again.

The crate is bumped to `0.4.3` so the binaries built from this tag report the
version they were tagged with. On the server side `v0.4.2..v0.4.3` is that one
line and nothing else.

## v0.4.2 — The second stage now says whether it ran

The same deployment that surfaced the v0.4.1 template defect turned up a second
one, and this one was not a config mistake: with `[rerank].mode = "on"`, its
reranker had never once contributed to a recall, and nothing a client could see
said so.

The stage misses its `[rerank].timeout_ms` by design and serves the pre-rerank
order — a second opinion is never worth a failed request — and the only trace
was a `WARN` in the journal. From the client's side that is indistinguishable
from `mode = "off"`. On that host, which also serves a 9B model and a local
embedding server, judging one candidate costs ~34 ms rather than the ~7 ms the
idle benchmark box managed, so a `depth = 10` head needs ~350 ms against a
250 ms deadline and every query worth asking lost the stage. `rerank audit`
aggravated it: measured deadline-free, it reported the stage as running while
the live path threw its answer away.

- **A recall that has a second stage now reports what it did**, as a top-level
  `rerank` object on `POST /v1/memories/search`: `applied` with how many
  candidates it judged and in how long, `shadow` with whether it *would* have
  moved the top result, `not_needed` when fewer than two cleared the floor, or
  `skipped` with `reason = "deadline"` or `"model"`. The field is **absent**
  when `mode = "off"`, so a deployment without a reranker gets the response it
  has always got, byte for byte. SDK models already tolerate unknown fields.
- **`rerank audit` says how many queries the stage actually contributed to**,
  and opens with that when it is not all of them. A run where the stage did
  nothing used to be indistinguishable from a run where reranking agreed with
  fusion, and "no disagreements found" is the wrong conclusion to draw from it
  ([evaluation.md](evaluation.md#judging-a-second-stage-on-your-own-memories)).
- **An eval whose stage was skipped says so once**, on stderr, instead of
  reporting reranking's measured gain as 0.0 and its cost as nothing.
- **`timeout_ms` is documented as what it is**: a tail bound measured on an idle
  machine, sized in pairs of milliseconds per candidate, with the two machines
  above side by side and a recipe — read `rerank.took_ms` across your own
  queries and set the deadline near twice that
  ([configuration.md](configuration.md#sizing-timeout_ms-for-your-machine)).

**For operators who turned reranking on and saw no difference:** send one search
and read `.rerank`. If it says `skipped`/`deadline`, raise
`[rerank].timeout_ms` to roughly 2× the `took_ms` you measure, or lower
`[rerank].depth` — understanding that the +5.8 precision@1 was measured at
depth 10, so a shorter head trades away part of the gain rather than getting it
cheaper.

## v0.4.1 — Two relevance settings that were being dropped

A review of a real deployment's `recuerdos-ai.toml` found that the example file
v0.4.0 shipped wrote `min_relevance` and `similarity_baseline` *after* the
`[rerank]` header. TOML cannot reopen a section, so both defined unknown
`[rerank]` keys instead of retrieval ones, and the loader ignored them without
complaint. `recuerdos-ai init` hands out that same file, so a copied template had
a relevance floor and a baseline that did nothing — including for the operator
whose config was being reviewed when it turned up.

- **The example is repaired.** Both keys are back under `[retrieval]`, with a
  note that a key written after the next `[table]` line belongs to that section.
  Pinned by a test that loads the shipped file, so the template cannot drift back
  into this. **If you copied the v0.4.0 example or ran `init`, move the two keys
  up yourself** — while they sat below `[rerank]`, changing them had no effect,
  whatever the file said.
- **A key nothing reads is now named at startup**, with the section it probably
  meant: `[rerank].min_relevance is not a known setting and is ignored — did you
  mean [retrieval].min_relevance?` Unknown keys stay non-fatal — a stray line in
  a config a working server has loaded should not become downtime — but they are
  no longer invisible, which is the only reason the example could carry this
  through review ([configuration.md](configuration.md#keys-nothing-reads)).
- **A first rerank-enabled boot explains its own pause.** The model load names
  the model and the cache directory *before* it begins, so a log that stops there
  is a ~1.1 GB download in progress, not a hang. `docs/deployment.md` also covers
  the bare-binary case, where `DynamicUser=` or `ProtectHome=` can leave the
  cache directory unwritable and the download has nowhere to land
  ([deployment.md](deployment.md#optional-second-stage-reranking)).

## v0.2.1

- **`[embeddings].max_batch_size`.** A remote embedding provider that caps
  its batch size — Alibaba DashScope rejects a batch larger than 10 — failed
  a `reindex` or a consolidation pass, which embed many texts at once, with
  `"batch size is invalid"`. The remote embedder now splits a large input
  into sequential requests of at most `max_batch_size` (default `0` = no cap,
  the previous send-it-all behaviour), preserving order. Set it to your
  provider's limit; the local provider ignores it.

## v0.2.0 — Graph (Strategy B): relational recall

Recall gains a third leg. Beside the semantic (vector) and keyword (BM25)
legs, a hop over an entity/relation graph reaches the memory that answers a
question it shares no words with — "who owns the billing service's data?"
surfaces the memory naming `ledger-db` by walking `billing service →
ledger-db`. **On by default**: the eval measured relational recall@5 at
**85.7%**, up from **71.4%** without the graph, with no other case kind
regressing.

- **A bi-temporal entity/relation graph.** Ingest extracts relations
  (`subject —predicate→ object`) alongside the entities it already pulled,
  and projects them into SQLite edge tables. Edges carry two clocks: *valid*
  time (when the fact was true in the world) distinct from the memory's
  *transaction* time (when we learned it). A superseding memory closes the
  old edge's validity interval instead of deleting it, so history is
  walkable, not lost.
- **Graph-hop retrieval as a third RRF leg.** Recall scans the query for
  entity mentions, keeps the ones some memory actually declared as seeds,
  and walks up to `[graph].max_hops` edges. It is additive and
  self-silencing: a query naming no known entity yields the exact two-leg
  result, so non-relational recall cannot regress — a property the eval and
  an empty-leg identity test both hold.
- **Tuned so recall's gain isn't precision's loss.** At full weight the hop
  let a relation "bridge" outrank the direct answer on one case, costing
  `precision@1`. `[graph].rank_weight` (default `0.3`) down-weights the hop's
  fusion term so it *adds* relational recall (85.7%) without *displacing* a
  strong direct match — `precision@1` stays at its pre-graph 62.5%.
  `[graph].unanchored_floor` (default `3`) additionally caps a memory only
  the graph found, so it can't leapfrog a direct hit while still reaching
  the top-k.
- **Time-travel recall.** `as_of` (REST search and the `memory_recall` MCP
  tool) reads the hop in valid time — "who was on-call before the reorg?" —
  traversing edges that were live then. `graph_rank` in a result's `matched`
  object says when a hop, rather than a semantic or keyword match, is why a
  result appeared.
- **Budgeted, resumable backfill.** `recuerdos-ai graph backfill
  --entities` (zero model calls) and `--relations` (bounded by
  `[graph].backfill_budget`, resuming where it stopped) give edges to a
  corpus that predates the graph; `--dry-run` spends nothing. See
  [deployment.md](deployment.md).
- **Off is a clean switch.** `[graph].enabled = false` restores the
  pre-graph behaviour exactly — no hop, no edges written, no
  relation-extraction tokens — for a deployment that wants neither.

## v0.1.1

- **`[server].mcp.allowed_hosts`.** The streamable-HTTP `/mcp` endpoint's
  DNS-rebinding guard accepted only loopback `Host` headers, so reaching it
  over the network needed a reverse proxy that rewrites `Host`. This option
  lets an operator allow the hostname(s) clients connect with directly
  (added to the loopback defaults, so local access still works); a single
  `"*"` disables the guard for a trusted private network. Empty — the
  default — is unchanged.
- **Docs & install hardening** since v0.1.0: reverse-proxy setups must
  rewrite `Host` for `/mcp` (Caddy `header_up Host localhost`); the Linux
  binary's glibc-2.38 floor (imposed by ONNX Runtime's prebuilt) is
  documented and `install.sh` now checks it up front and points to Docker
  below it; the GHCR image name is lowercased; and the README gained
  Updating and Uninstalling sections plus a comparison and use cases.

## v0.1.0 — Phase 6: SDK, docs & release

The first release candidate. Mostly about making everything from Phases
0–5 installable, documented, and usable from something other than curl —
plus the provider choice and operational features a self-hosted
deployment needs.

- **Python SDK** (`pip install recuerdos-ai`): a thin typed client over
  the REST API — one method per endpoint, pydantic models, and no
  caching or retry opinions of its own. The two things it does add are
  the two a raw `httpx` call makes awkward: errors keyed off the API's
  stable error `code` so callers branch on a type, and job polling.
  Ships a LangChain `BaseRetriever` behind a `[langchain]` extra.
- **Integration recipes** for the client types without one: Hermes,
  LangChain/LangGraph, and a plain-REST recipe for anything else. Every
  request and response in them was captured from a running daemon.
- **Benchmarks** in `docs/performance.md`, measured against the release
  image rather than a dev build, with the methodology and the hardware
  written down beside the numbers — including the targets that were
  missed.
- **`install.sh`**, which downloads a binary, verifies its checksum, and
  refuses to install anything unverified. Tested against a tampered
  tarball and a missing checksum, not just the happy path.
- **Release workflow**: native builds per architecture (linux
  x86_64/aarch64, macOS arm64), multi-arch image to GHCR, drafted
  release notes.
- **Apache-2.0**, and a recorded name-availability check.
- **README** rewritten as a launch page.

And the deployment-facing features that landed alongside the release work:

- **External embedding providers.** Beyond the built-in local ONNX model,
  embeddings can come from a **native Gemini client** (task-type-aware —
  `RETRIEVAL_DOCUMENT` on store, `RETRIEVAL_QUERY` on search, for better
  recall), any **OpenAI-compatible** endpoint, or **Ollama**. Chosen with
  `[embeddings].provider`; the model's dimensionality is discovered by a
  real request at startup, so there is no width to configure.
- **`recuerdos-ai reindex`.** Change the embedding model or provider and
  re-embed every memory in place — in one transaction — instead of
  needing a fresh data directory. The store pins its model, and the daemon
  now **refuses to start on a mismatch** with a message naming the fix,
  rather than failing later on the first recall.
- **Streamable-HTTP MCP transport** at `/mcp` (previously deferred, see
  Phase 3). A client connects over HTTP with a bearer token — no local
  binary, no `docker exec` shim — sharing the same four tools and the same
  per-user auth as the stdio path. This is the natural way to reach a
  daemon running on another host.
- **Gemini as a reasoning provider** for `[understanding]` — a preset over
  the OpenAI-compatible client, since Gemini's chat API needs no native
  client the way its embeddings do.
- **`recuerdos-ai config`** prints the effective, secret-free configuration
  (which providers, models and transports are actually in force), and
  `RECUERDOS_AI_CONFIG` points the whole CLI at one file without repeating
  `--config`.

## v0.5.0-alpha — Phase 5: Consolidation

Memory that stays clean over time. Until now the store only grew; this
phase is everything that makes it shrink again.

- **`POST /v1/sessions/distill`** and the **`session_distill` MCP tool**
  reduce a finished session to the few things that outlive it. Same
  pipeline as ordinary ingestion, asked a stricter question — *what is
  still true after this session ends?* — so the conventions and root
  causes survive and the task chatter does not. Most sessions distil to
  nothing, which is the correct answer.
- **A nightly consolidation job**, also runnable as
  `recuerdos-ai consolidate [--dry-run]`. Near-duplicates within a user's
  category are grouped by similarity, and the model writes the one memory
  that says what all of them said. Five phrasings of one preference
  become one active memory and five superseded ones, each with a `merge`
  audit entry carrying the reasoning.
- **The threshold proposes, the model disposes.** Similarity gets
  memories into a cluster but cannot tell whether they mean the same
  thing — "prefers pnpm" and "prefers Vitest" are very close in embedding
  space and are two facts. The merge prompt argues *against* merging, and
  every unusable answer parses as keep-separate: a duplicate surviving
  one more night is cheap, a merged-away fact is not.
- **TTL expiry.** Memories past their `expires_at` are retired with an
  audit entry naming the date. Retired, not erased — `expires_at` is a
  promise a memory stops being used, not that it stops existing.
- **Importance decay.** Memories are rescored from how recently and how
  often they were actually recalled, feeding recall ranking. It only ever
  demotes, and never below a floor well above zero: an architecture
  decision nobody has read this year has to lose ties, not vanish.
- **An LLM-written `memory://profile`.** Generated per half (how they
  work / about them) and cached until the memories under it change, so it
  compresses forty preferences into a paragraph instead of truncating at
  eight. Staleness is derived from the memories themselves rather than a
  dirty flag every write path has to remember to set.
- **Graceful degradation throughout.** Expiry and decay need no provider
  and run everywhere. Merging and the written digest need one and are
  simply skipped without it — the profile falls back to assembly, and the
  resource contract does not change. Distillation is the single exception:
  it refuses rather than storing a transcript whole, because that memory
  would be unrecallable and would cost a context window on every match.

## v0.4.0-alpha — Phase 4: Understanding

The differentiator: raw text in, understood memories out. A memory store
stops being a log.

- **`POST /v1/memories`** accepts raw content and returns `202` with a job
  id; **`GET /v1/jobs/{id}`** reports what became of it. The work is a
  model call that takes seconds — holding the request open for it would
  make a client's timeout our problem and lose the work on a disconnect.
- **Extraction** splits content into atomic, separately-recallable
  memories with a category, tags and entities. One sentence about moving
  hosts *and* a testing preference becomes two memories, not one blob.
- **Reconciliation** decides ADD / UPDATE / DELETE / NOOP against the
  memories most similar to each candidate. A contradiction supersedes
  what it replaces instead of accumulating beside it, so recall stops
  returning last quarter's answer alongside this quarter's. Superseded
  memories are retained for audit and reachable with
  `include_superseded=true`.
- **Three providers** — Anthropic, any OpenAI-compatible endpoint, and
  Ollama — behind one `ChatModel` contract, with retry, timeout and
  malformed-JSON recovery shared across all of them so behaviour cannot
  drift between them.
- **A durable job queue and worker pool.** Jobs survive a restart, a
  crashed worker's job is reclaimed rather than lost, and a poison job
  dead-letters with its error instead of retrying forever.
- **Still zero-egress by default.** `[understanding].provider` defaults to
  `none`, and in that mode the same endpoints store content verbatim,
  inferring only the category from unambiguous phrasing. Turning a
  provider on requires no client changes.
- **MCP `memory_save` runs the pipeline**, so an agent's save is
  reconciled against what is already stored — and the tool says so
  honestly, including when nothing was stored because the store already
  knew it.
- **`recuerdos-ai eval`** scores retrieval quality against a committed
  baseline, and CI fails a PR that drops recall@5 by more than five
  points. Nothing else in the suite would notice a ranking regression.
  See [evaluation.md](evaluation.md).

Known limitation: one eval case is committed failing — a query with no
vocabulary in common with its memory ("book a restaurant" → "user is
vegetarian") does not reach the top 5. It is kept as a real target rather
than removed to flatter the score.

## v0.3.0-alpha — Phase 3: MCP server

Claude Code, opencode and any other MCP client can now read and write the
same memory store.

- **Three tools** — `memory_save`, `memory_recall`, `memory_forget` — and
  the **`memory://profile` resource**, served by `recuerdos-ai mcp`.
- **The stdio server is a shim**, forwarding to the daemon over the same
  authenticated REST API any client uses. Four editor windows would
  otherwise mean four resident embedding models and four writers on one
  SQLite file.
- **Tool descriptions carry the trigger logic.** A model decides from the
  description alone whether to save something, so they name concrete
  phrasings ("I prefer", "we decided") *and* say when not to call —
  without which a memory store fills with transient task chatter.
- **`memory_forget` is two-step and the server enforces it**: ids without
  `confirm: true` delete nothing, and the candidate listing states
  plainly that nothing has been deleted yet.
- **`GET /v1/profile`** exposes the same digest over REST, which is what
  the shim reads and what a Claude Code SessionStart hook can curl.
- Integration recipes for [Claude Code](integrations/claude-code.md)
  (including SessionStart and PreCompact hooks) and
  [opencode](integrations/opencode.md).

Deferred at the time: the streamable-HTTP MCP transport. rmcp's session
factory cannot see request headers, so per-user auth would need a per-call
path that complicates keeping tool definitions identical across transports
— and every target client then spoke stdio. **Shipped later in v0.1.0**
(see above): the HTTP handler forwards the request's bearer to the same
per-user REST auth, so both transports share one set of tool definitions.

308 tests.

## v0.2.0-alpha — Phase 2: Memories

Store memories and search them semantically and lexically, fully offline,
strictly per user.

- **Hybrid recall.** A vector leg (sqlite-vec) and a keyword leg
  (tantivy BM25) run on every search, fused by reciprocal rank. The
  keyword leg is what finds `useQuery` as a literal token; the vector leg
  is what answers a question sharing no words with the memory.
- **Local embeddings**, in-process ONNX (bge-small-en-v1.5, 384d), baked
  into the Docker image so a container never downloads at runtime and an
  air-gapped host works. `recuerdos-ai warm-models` pre-populates a cache.
- **Taxonomy**: eight built-in categories, extensible via config. Unknown
  names are rejected rather than silently created.
- **REST**: `POST /v1/memories:direct`, `POST /v1/memories/search`,
  `GET/PATCH/DELETE /v1/memories/{id}`, `GET /v1/memories/export`,
  `GET /v1/audit`.
- **Export** as markdown (grouped by category, one memory per line, so it
  stays greppable) or JSON — your memories are yours and portable.
- **Soft delete and supersede** both retain the row, so the audit trail
  stays truthful.
- Isolation extends to memories: `user_id` is a vec0 *partition key*
  (a query never scans another user's vectors), each user has their own
  tantivy index directory, and the cross-tenant suite now tests recall,
  by-id reads, edits, deletes, export and audit.

**A 12x latency fix, found by measuring rather than assuming.** The first
benchmark showed 251ms end-to-end searches against 8ms of actual work:
argon2id was running on every request and accounted for 96% of the
response. `VerifiedKeyCache` caches the verification (SHA-256 digest of
the presented secret, constant-time compared) while still reading the key
row every time, so revocation and scope changes take effect immediately.

Measured after, at 2,000 memories: **p95 29ms end-to-end**, 16ms
server-side, 55ms per ingest.

Known limitations, stated rather than buried:

- The plan's 100k-memory target is unproven. sqlite-vec 0.1.x has no ANN
  index — KNN is a brute-force scan — so recall latency grows linearly
  with corpus size. `scripts/bench-recall.sh` takes the scale as an
  argument.
- Search filters are applied after the indexes answer, so a very
  selective filter over a large corpus can return fewer results than
  asked for.
- No generated OpenAPI spec yet; `docs/api.md` is hand-written. The spec
  belongs with Phase 6's SDK, which is what consumes it.
- Ingest commits the text index synchronously (read-after-write beats
  throughput for an agent that just saved something), which is most of
  the 55ms.

The release image grows from ~112 MB to ~287 MB: the embedding model is
baked in (~130 MB) so a container never downloads at runtime, plus the
C++ runtime libraries ONNX needs. Both build stages now pin the same
Debian release — `rust:1-slim` had drifted to trixie while the runtime
stage was still bookworm, which produced a binary the runtime's glibc
could not load.

297 tests.

## v0.1.0-alpha.1 — Phase 1: Identity

Multi-user authentication. Every `/v1` route now resolves to exactly one
user, and the isolation guarantee is enforced by the type system.

- **Users and API keys.** `recuerdos-ai user add|list`,
  `key issue|list|revoke`. Keys are `ra_live_<prefix><secret>`; only an
  argon2id hash is stored, and the key is displayed once at issue time.
- **Scopes**: `read`, `write`, `admin`. `write` does not imply `read`, so
  a write-only ingestion key is expressible. `admin` implies everything.
- **`UserContext`**, the capability token every later context will require.
  Its constructors are `pub(in crate::identity)`, so no other context can
  invent one for a user it did not authenticate — cross-tenant access is a
  compile error. `check-boundaries.sh` fails if that visibility is widened.
- **Auth middleware**: `Authenticated`, `ReadAccess`, `WriteAccess`
  extractors. The argon2 verify runs on `spawn_blocking`; `last_used_at`
  is recorded off the request path.
- **Uniform rejections**: missing, malformed, unknown, wrong-secret and
  revoked keys are byte-identical 401s, and the verify runs before the
  revocation check so timing doesn't leak revocation either.
- **Error envelope** `{"error":{"code","message"}}` for all contexts;
  `internal` responses never carry their detail.
- **`[auth].mode = "none"`** for single-user deployments: a real persisted
  `default` user (so data isn't orphaned if auth is re-enabled), a loud
  startup warning, and fail-closed parsing of the mode value.
- **SQLite storage**: WAL, enforced foreign keys, refinery migrations run
  at startup.
- `tests/identity_isolation.rs`, the cross-tenant suite later phases
  extend. Its sharpest case splices one user's public prefix onto
  another's secret and asserts 401.
- Docs: `api.md`, `security.md` (isolation + threat model),
  `configuration.md` auth/storage sections, `architecture.md` worked
  example.

130 tests.

## v0.1.0-alpha.0 — Phase 0: Foundation

- Screaming-architecture skeleton (identity/memories/understanding/
  providers/consolidation, each domain/application/infrastructure) plus
  the shared kernel (`RaError`, UUIDv7 ids, `Clock`).
- `scripts/check-boundaries.sh` boundary check, wired into `just check`.
- Docker-only dev flow: `Dockerfile.dev` (non-root, host-UID-matched),
  `docker-compose.yml`, and a multi-stage release `Dockerfile` (~112 MB,
  non-root, healthchecked).
- `AppConfig`: TOML + `RECUERDOS_AI_*` env overrides via figment,
  aggregated validation errors, `recuerdos-ai init`.
- axum HTTP skeleton: `/healthz`, `/version`, graceful shutdown on
  SIGINT/SIGTERM, tracing (plain or JSON via `RECUERDOS_AI_LOG=json`).
- GitHub Actions CI: check / test / docker-build-and-smoke-test.
