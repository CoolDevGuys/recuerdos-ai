# Configuration

**Status: Phase 2.** Every key below is loaded and validated today.
`[auth]`, `[storage]`, `[embeddings]`, `[retrieval]` and
`[understanding.taxonomy].extra_categories` are enforced; the remaining
`[understanding]` and `[consolidation]` configure the model-backed
features; both degrade to something useful without a provider.

## Precedence

Defaults → `recuerdos-ai.toml` → `RECUERDOS_AI_*` env vars (env wins).
Nested keys use a double underscore: `RECUERDOS_AI_SERVER__PORT=8080` sets
`[server].port`.

The file is read only when you point at it — with `--config PATH`, or by
setting `RECUERDOS_AI_CONFIG=PATH` in the environment. The env var is the
easy way to make **every** command (and the daemon) read the same file
without repeating `--config`; an explicit `--config` still wins over it.
There is no auto-discovery of a `recuerdos-ai.toml` in the working
directory, so a daemon or a `reindex` started with neither uses defaults +
env and ignores a file sitting next to it.

```bash
recuerdos-ai init                 # writes ./recuerdos-ai.toml + creates the data dir
recuerdos-ai serve --config recuerdos-ai.toml
```

`init` refuses to overwrite an existing file, and re-loads what it wrote
before declaring success — if `init` reports success, the file is valid.

## Checking what is in effect

Because env vars override the file, the config a running daemon actually
uses is not always what the file says. `recuerdos-ai config` (or
`make config`) prints the **resolved** result — which embeddings and
understanding provider are selected, their models and endpoints, the MCP
transports, the second-stage reranker, storage path, and auth mode:

```bash
recuerdos-ai config
```

The rerank block prints one line, `mode off`, unless the stage is actually
enabled — the point of the command is to show what a running daemon pays,
and with the stage off it pays nothing. When it is on, the same block shows
the model, depth, strategy, weight and deadline in force, so the two numbers
an operator needs when a recall is slow (which model, how deep) are one
command away and reflect env overrides, not the file you meant to edit.

It prints no secrets. For a provider API key it shows only the env var
holding it and whether that var is set — so a `GEMINI_API_KEY (NOT SET)`
line is the quickest way to spot the misconfiguration that would
otherwise surface as a provider failing on its first request.

## Reference

See [recuerdos-ai.example.toml](../recuerdos-ai.example.toml) for every key
with its default and an explanatory comment. Highlights:

- `[embeddings].provider` defaults to `local` — embeddings work fully
  offline, no external service required.
- `[understanding].provider` defaults to `none` — extraction and
  reconciliation are opt-in, and with no provider configured there is zero
  LLM egress. Submitted content is stored verbatim instead; see
  [api.md](api.md#without-a-provider) for exactly what changes.
- `[consolidation].enabled` defaults to `true`. Expiry and importance
  decay run regardless of provider — both are arithmetic. Merging
  duplicates needs `[understanding].provider`, because deciding whether
  two memories mean the same thing has no offline fallback.
- `[graph].enabled` defaults to `true` — recall runs a third "connected to"
  leg (an entity/relation graph hop) beside the semantic and keyword legs,
  and ingest writes edges. It earns the default: the eval measured
  relational recall@5 climbing 71.4% → 85.7% with no other case kind
  regressing. It only fires when a query names an entity some memory of
  yours declared, so a query that names nothing known gets the exact
  two-leg result. `[graph].extract_relations` (also default `true`) is
  whether ingest asks the model for the edges the hop walks — turn it off,
  or turn `[graph].enabled` off entirely, to restore the pre-graph
  behaviour and its lower token cost. A graph the extraction never filled
  can be backfilled with `recuerdos-ai graph backfill`.
- `[graph].rank_weight` (default `0.3`) and `[graph].unanchored_floor`
  (default `3`) tune how much the hop leg sways ranking. Weighting it below
  `1.0` lets the graph *add* relational recall without *displacing* a memory
  the vector or keyword leg already ranked well — the pair that keeps the
  85.7% relational recall while returning `precision@1` to its pre-graph
  62.5%. The floor caps a memory *only* the graph found so it can't leapfrog
  a strong direct match. Both are inert when a query surfaces no graph hits,
  so non-relational recall is unaffected.
- `[retrieval].relevance_weight` defaults to `0.05` — after the index legs are
  fused, how well a memory matches competes with how new it is, as a bounded
  multiplier `1 − weight × (1 − relevance)`. It earned **+3.8 precision@1** on
  the eval with nothing else moving, and is deliberately near-tie-sized; `0.0`
  gives back ranking by the legs alone. [Retrieval](#retrieval) carries the
  arithmetic of what each part of the accepted range can and cannot overrule.
- `[storage].path` supports a leading `~` for `$HOME` expansion.

## Validation

Invalid config reports every problem at once, not just the first:

```
config error: [server].port is 0; [embeddings].model is empty
```

Each message names the exact TOML path it's complaining about.

### Keys nothing reads

A key the schema does not know is *ignored* rather than fatal — a stray line
in a config a working server has already loaded should not become downtime —
but it is not silent. The daemon warns at startup, and when the same key name
exists in another section it says which one:

```
WARN recuerdos-ai.toml: [rerank].min_relevance is not a known setting and is ignored — did you mean [retrieval].min_relevance?
WARN recuerdos-ai.toml: [rerrank] is not a known section and is ignored
```

The check exists because of a shape that is easy to write by accident. TOML
cannot reopen a section, so a `[retrieval]` key typed after the next `[table]`
header below it defines a key of *that* section instead — and an unknown key of
a real section is the one mistake the loader had no way to mention. During
rerank development the example file itself fell into exactly that hole (both
relevance keys trailed the `[rerank]` block, so `recuerdos-ai init` handed out a
template whose `min_relevance` did nothing), and **v0.4.0 shipped with it**. The
example is repaired and pinned by a test in `v0.4.1`; if you are on `v0.4.0` and
copied the file, move `min_relevance` and `similarity_baseline` up above
`[rerank]` yourself — before this warning existed, editing them there had no
effect at all.

So: every key for a section belongs above the next `[table]` line. If a
setting you changed appears to have no effect, check the startup log for this
warning before anything else.

## Authentication

```toml
[auth]
mode = "api-key"   # api-key | none
```

### `mode = "api-key"` (default)

Every `/v1` route requires a bearer key. Manage keys with the CLI:

```bash
recuerdos-ai user add alex --email alex@example.com
recuerdos-ai key issue --user alex --scopes read,write --name laptop
recuerdos-ai key list --user alex
recuerdos-ai key revoke b99f884a          # by prefix
```

The key is displayed once at issue time. Only an argon2id hash is stored,
so a lost key can be replaced but never recovered.

An unrecognised `mode` value falls back to `api-key`. Failing closed is
the only safe direction: a typo must never silently disable
authentication.

### `mode = "none"`

Disables authentication entirely — every request runs as a built-in user
named `default`. Intended for a single-user deployment bound to
`127.0.0.1`, where the OS is already the access control.

Anyone who can reach the port is the `default` user. The server logs a
warning at startup whenever this is set.

Turning it back on later is safe: `default` is a real persisted user, so
memories written while auth was off keep their owner rather than being
orphaned.

## Embeddings

```toml
[embeddings]
provider  = "local"                    # local | gemini | openai-compat | ollama
model     = "bge-small-en-v1.5"        # or all-minilm-l6-v2
cache_dir = "~/.recuerdos-ai/models"    # local only
```

### `provider = "local"` (default)

Embeddings are computed in-process, on CPU, with no external service —
that is why the daemon works with no API key and no network.

`cache_dir` is where the ~130 MB model lives. The Docker image bakes it
in at `/models`, so containers never download at runtime. On bare metal
the model is fetched on first use; `recuerdos-ai warm-models` does it
ahead of time, which is what you want before taking a host offline. Both
built-in models are 384-dimensional.

### Gemini (`gemini`) — native, task-type-aware

A dedicated Google client rather than the OpenAI-compat shim, because
Gemini's embedding models are **asymmetric**: they embed a document and
the query meant to retrieve it differently, chosen by a `taskType`. This
client sends `RETRIEVAL_DOCUMENT` when storing and `RETRIEVAL_QUERY` when
searching, which the models are trained for and which measurably improves
recall. The OpenAI-compat endpoint can't express `taskType`, so prefer
`gemini` over `openai-compat` for a Gemini key.

```toml
[embeddings]
provider    = "gemini"
model       = "text-embedding-004"        # 768-dim
api_key_env = "GEMINI_API_KEY"            # a Google AI Studio key
# base_url defaults to Google's endpoint
```

Auth is via a header, never a `?key=` URL parameter, so the key stays out
of logs. The key is required.

**For reasoning** (`[understanding]`), `provider = "gemini"` is available
too — but there it is a *preset* over the OpenAI-compat client pointed at
Google's endpoint, not a native client. Gemini's chat API exposes nothing
`taskType`-like that the compatibility endpoint hides, so a second client
would earn nothing. Configure it the same way, with a chat model:

```toml
[understanding]
provider    = "gemini"
model       = "gemini-2.0-flash"
api_key_env = "GEMINI_API_KEY"
# base_url defaults to Google's OpenAI-compatible endpoint
```

### Remote providers (`openai-compat`, `ollama`)

Point embeddings at any other external provider — the same shape as
`[understanding]`: a URL, a model name, and (where the provider needs
one) an API key read from the environment. `openai-compat` also works
against Gemini's compatibility endpoint, but without the task-type
advantage above.

```toml
# OpenAI, or any gateway that copies its /v1/embeddings API
[embeddings]
provider    = "openai-compat"
model       = "text-embedding-3-small"
base_url    = "https://api.openai.com/v1"   # empty = this default
api_key_env = "OPENAI_API_KEY"              # names the env var, not the key
```

```toml
# A local Ollama — no key, it is unauthenticated
[embeddings]
provider = "ollama"
model    = "nomic-embed-text"
base_url = "http://127.0.0.1:11434"         # empty = this default
```

| Key | For |
|---|---|
| `base_url` | The provider's address. Empty means its usual one — set it for an OpenAI-compatible gateway (OpenRouter, Together, a local vLLM, LM Studio) or a non-default Ollama host. |
| `api_key_env` | The **name** of the env var holding the key, never the key itself — a key in a config file gets committed eventually. Leave empty for a keyless local server. Ignored by `ollama`. |
| `max_batch_size` | The most texts one request may carry. `0` (default) sends the whole batch at once. Set it when a provider caps the batch — e.g. **`10` for Alibaba DashScope**, which otherwise fails a `reindex` with `"batch size is invalid"`. Larger inputs are split into sequential requests. Ignored by `local`. |

The model's **dimensionality is discovered at startup** by making one
real embedding request, so there is no dimensions setting to get wrong. A
bad URL, key or model name stops the daemon immediately with a clear
error, rather than surfacing later as recalls that return nothing.

### The model is pinned per collection

Vectors from two different models are not comparable, so changing
`provider` or `model` against an existing database is caught rather than
allowed to return nonsense rankings — a `text-embedding-3-small` vector
(1536-dim) cannot live in a store built for 384-dim `bge-small`.

The pin — the model name **and** its dimensionality — is recorded the
first time you store something; you never set it yourself, and there is no
dimensions option to get wrong (a remote model's width is discovered by a
real request at startup). If the configured model later disagrees with the
pin, **the daemon refuses to start** and tells you exactly what to do:

```
this store was built with embedding model "bge-small-en-v1.5" (384 dimensions)
but the service is configured for "gemini-embedding-001" (3072). … run
`recuerdos-ai reindex` (with the daemon stopped) to re-embed every memory
under the new model in place.
```

This is a fail-fast at startup, not a raw error on your first recall — and
it also catches a swap between two different models that happen to share a
width, which a dimension check alone would miss.

To switch **and keep your memories**, re-embed them in place:

```bash
# 1. stop the daemon
# 2. edit [embeddings] to the new provider/model
recuerdos-ai reindex
# 3. start the daemon
```

`reindex` reads every stored memory's text (which is model-independent),
re-embeds it with the new model, rebuilds the vector index at the new
dimensionality, and updates the pin — all in one transaction, so a failed
run leaves the old store untouched and can simply be retried. Nothing is
ever read but the content, so no memory is at risk. Run it with the
daemon **stopped**: it rebuilds the vector table and holds the write
lock.

The full-text (BM25) index is model-independent and is not touched.

## Retrieval

```toml
[retrieval]
hybrid                 = true
default_limit          = 8
recency_half_life_days = 90
relevance_weight       = 0.05
```

`recency_half_life_days` tunes how much a memory's age discounts it.
Recency is a *bounded* multiplier — it can reorder comparably relevant
results but never bury a far more relevant old memory. See the reasoning
in `src/memories/domain/recall_ranker.rs`.

`relevance_weight` tunes the other side of that sentence: whether *how well a
memory matches* is allowed to compete with how new it is. A recall score is
`(rank evidence) × recency × quality × affinity`, where
`affinity = 1 − relevance_weight × (1 − relevance)`, and `relevance` is the `0..1`
number every result reports — measured from what the indexes actually matched,
which is also what `[retrieval].min_relevance` thresholds.

The default, `0.05`, is what the calibration grid earned: precision@1 50.0 → 53.8
and MRR 63.9 → 66.6 on the eval set, with recall, completeness and abstention
unmoved and no case losing its top hit. That is a near-tie-sized influence: a
maximum relevance gap at `0.05` reverses rank 1 against rank 4 and stops there
(reversing rank 1 against rank 5 needs `0.062`), which is the legs' disagreement
surviving everything except their closest calls. Set `0.0` to switch the stage off
entirely (that is a different code path, not a small weight) and get back the
order recall shipped before this setting existed.

The value is a *spread*, so it is worth knowing what a larger one buys and what
it costs. At `0.2`, a relevance gap of `0.06` is worth 1.2% of a score — enough
to win a pair a week of age apart, and also enough to overrule the two legs
disagreeing by ten ranks (14.8%). Where the line sits is arithmetic rather than
taste, and the accepted range runs to `0.5`, at which relevance overrules any rank
gap a result list contains: ranking by relevance, with the indexes demoted to a
suggestion. The grid measured `0.2` through `0.5` and shipped none of them,
because that corpus favours relevance partly by construction — its expected
memories are the topically nearest ones. Confirm a larger value on your own
memories before trusting it:

```bash
recuerdos-ai recall audit --config tuned.toml --compare-config deployed.toml \
    --queries questions.txt
```

It writes nothing (the daemon may stay running) and prints which results moved and
what the change cost.

`default_limit` applies when a search omits `limit`; requests are capped
at 50 regardless.

There is also an optional second stage, `[rerank]` (documented in
`recuerdos-ai.example.toml`): a local cross-encoder that re-scores the head of
the result list — the candidates that survived `min_relevance`, up to `depth` of
them, never the whole fused list. It is off by default, and
`recuerdos-ai rerank audit` is how you judge it on your own store before turning
it on — a read-only replay that runs *without* `min_relevance`, so it shows you
the near-misses your clients would never see
([`docs/evaluation.md`](evaluation.md)).

### If a model is too big for your box

The default rerank model was chosen on quality, not on size: it earns
**+5.8 precision@1** on the eval set and costs **1.1 GB of weights**
(downloaded into `[embeddings].cache_dir`, or baked into the image) and about
**1.5 GiB of RAM** per concurrent inference. A machine that cannot pay that
should leave `mode = "off"`. The measured alternative is not a graceful
degradation: the small model's whole gain is **+1.9 points**, which the grid
showed to be inside run-to-run noise. There is no quality tier to step down
through — you either want the stage or you do not.

### Language: what the default model actually speaks

`bge-reranker-base` is a **Chinese-and-English** reranker: an XLM-R backbone
whose ranking fine-tuning is English plus Chinese. It will happily return a score
for text in any language — that is the trap — but its judgement of *relevance*
was only ever trained in two. Outside those two, treat its ordering as untested,
not merely a bit worse: our whole +5.8 was measured on English cases, so for,
say, German or Japanese memories there is no number here to lean on, only the
honest statement that nobody measured it.

| Model | Languages it was trained to rank | Weights | Added p95 | Measured gain |
|---|---|---|---|---|
| `bge-reranker-base` (default) | English, Chinese | 1.1 GB | +71 ms | **+5.8 p@1** — the only candidate that clears the ship gate |
| `jina-reranker-v2-base-multilingual` | broad multilingual | 1.1 GB | +76 ms | +3.8 p@1, and only with `strategy = "rrf"` |
| `bge-reranker-v2-m3` | widest of the four (BGE's multilingual line) | 2.2 GB | +157 ms | +1.9 — the same as the 147 MB model, for fifteen times the RAM |
| `jina-reranker-v1-turbo-en` | English only | 147 MB | +20 ms | +1.9 (inside noise) |

So for a store that is not English or Chinese, the multilingual option costs
about what the default costs — the same footprint, a little more latency — and
returns roughly two thirds of a gain that was itself measured in English. That
makes it the default choice for a multilingual store, with the caveat attached,
not hidden: set it, then **shadow it on your own data** and read the audit before
serving it, because the corpus here could not tell you how it behaves in your
language. If you would rather spend no latency on an unproven second opinion,
`mode = "off"` is the correct setting, not a failure to configure.

### Sizing `timeout_ms` for your machine

`[rerank].timeout_ms` (default `250`) is a tail bound, not a cost target. Past
it the stage stops judging, whatever it has ordered is kept, and the request goes
on its way — a second opinion is never worth a failed recall. The default is
about 3.5× the p95 measured during tuning (71 ms at `depth = 10`), and that
tuning box was **idle**, which is the part that does not travel.

The cost is per candidate pair, so what matters is your machine's number:

| Machine | Cost per judged candidate | What `depth = 10` needs | Against a 250 ms bound |
|---|---|---|---|
| the tuning box (nothing else running) | ~7 ms | ~71 ms | comfortable |
| a home server also serving a 9B LLM and a local embedding server | ~34 ms | ~350 ms | ~7 of 10 judged, the tail left as fusion ordered it |
| that same server, after its embedding model was changed | ~120 ms | ~1.2 s | 2 of 10 — reordering two is not a second opinion |

The second row is a real deployment, not a worst case invented for a warning.
Before the head was scored in runs, its `mode = "on"` never contributed once: a
missed deadline abandoned the whole head, every query worth asking had more than
about seven candidates above the floor, and nothing in the response said so until
`v0.4.2` added the `rerank` field. Check yours in one request:

```bash
curl -s $BASE/v1/memories/search \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"query":"what are my programming preferences","limit":8}' | jq '.rerank'
```

```json
{"state": "skipped", "reason": "deadline", "candidates": 10, "took_ms": 251}
```

Sizing by measurement used to come with a catch that bit on exactly the slow
machines this section exists for. The stage used to score the whole head in
**one** call, so the deadline was all-or-nothing and there was no partial result
to take a rate from: a leg reported `skipped`/`deadline` carried a `took_ms`
**equal to the bound you set**, not the cost of the work it did not finish.
Doubling the number in that line produced a larger bound that was also exceeded,
at twice the cost — which is how that deployment ended up at `800` and still
skipping.

Now the head is scored in runs of `[rerank].batch_size` candidates (default 16),
and what a deadline abandons is one run. What finished is applied, and that makes
the measurement one request instead of a search for a query short enough to
complete:

```json
{"state": "applied", "judged": 4, "took_ms": 190}
```

`judged` below `[rerank].depth` is the stage saying it ran out of time part way
through the head — and it is a rate you can use. Four pairs in 190 ms is ~48 ms
a pair, so `depth = 10` on that machine wants ~480 ms, not 250.

Take the rate from the *inference*, and the log splits it for you. Both halves
appear on every leg worth reading:

```
WARN rerank leg failed; falling back to the pre-rerank order … busy=true queued_ms=250 inference_ms=0
WARN rerank leg applied the part of the head it finished before its deadline judged=4 depth=10 rerank_ms=190 queued_ms=120 inference_ms=70
```

The first is a leg that never held a session: the queue ate the budget and the
model was asked for nothing. The second waited 120 ms, then judged four. That
distinction is the whole difference between two settings, and it is one the
client cannot make — the response says `deadline` for both, because the shipped
SDKs parse two reason values and a third would break them:

| The line says | The machine is | Change |
|---|---|---|
| `queued_ms` ≈ the budget, `inference_ms` 0 | contended: no session free | `[rerank].max_concurrency` (each one is another ~1.5 GiB), or a quieter host. `timeout_ms` only makes the recall slower |
| `queued_ms` small, `inference_ms` ≈ the budget | slow at inference | `timeout_ms`, or a shorter `depth` |

Read it with `journalctl -u recuerdos-ai | grep 'rerank leg'`; `rerank audit`
prints the same two numbers as percentiles when a run has them, and says plainly
when it does not (its own replay runs with no deadline, so nothing there ever
waits). A healthy leg logs nothing at all — nothing is wrong — so
`RUST_LOG=debug` is what answers "am I queueing?" on a host that is serving
results: every leg then logs one `rerank_leg` line with both halves.

For the ~34 ms machine above that arithmetic lands on `800`, which turns a
~260 ms recall into ~360 ms — the honest price of reranking on a box that is also
doing something else. If that price is too high, `depth = 5` (~180 ms there) is
the alternative rather than a free lunch: the +5.8 precision@1 was measured at
depth 10, so shortening the head trades away part of the gain.

Two things about that number worth saying out loud. It is a property of the
machine, so re-measure whenever the machine changes: the third row above is the
second row's host measured again after an unrelated model change, and the `800`
this page had recommended for it was, at the time, throwing every head away once
more. And while the stage is spending its bound waiting for a session it will not
get, you are paying the deadline for nothing at all — ~800 ms of waiting to serve
the fused order. In that state `mode = "off"` is not merely the neutral setting,
it is the faster one.

`timeout_ms = 0` is rejected at startup, not treated as "no deadline": a
deadline-free stage on a contended machine is a recall with no bound, and the
fallback this whole path is built around only exists if there is a deadline.

### Recall quality: "results look random"

When recall returns things that have nothing to do with the question, it is
almost never the ranking. Three checks, in this order:

1. **Are the tokens you typed even in the index?** A memory matching the exact
   identifier you searched for is the keyword leg's job. Up to `v0.3.x` that
   leg indexed stopwords too, so "what database do we use" matched every
   memory containing "the". `v0.4.0` fixed it; the fix needs a one-time keyword
   index rebuild, which happens automatically on the first start after the
   upgrade (see [`docs/deployment.md`](deployment.md) if it reports a lock).
2. **Is the relevance baseline right for *your* embedding model?** Relevance
   is a cosine rescaled from the model's own noise floor, and that floor moves
   enormously between models — the same pair of memories scores 0.15 on one
   model and 0.65 on another. Getting it wrong looks exactly like random
   results, because either everything clears the floor or nothing does.
   `recuerdos-ai calibrate` measures it and tells you what is in force;
   `[retrieval].similarity_baseline` sets it by hand. The models in common use
   ship with measured defaults; a model nobody has measured falls back to 0.5
   with a warning at startup — that warning is the second symptom of this
   problem, and the first is the recall you are looking at.
3. **Is the answer in there at all?** `recuerdos-ai eval` runs a corpus of
   known-answer questions; `rerank audit` replays your own queries and shows
   the full candidate list *without* the floor, so a correct answer stuck at
   rank 9 looks different from a correct answer that does not exist.

Turning the reranker on is not a fix for any of these. It reorders what
retrieval already found; it cannot surface a memory the legs missed, and
`depth` is not a knob for "wider" — candidates outside it are never judged.

## Storage

```toml
[storage]
path = "~/.recuerdos-ai/data"
```

The database lives at `<path>/recuerdos-ai.db`, and the per-user keyword
indexes at `<path>/text-index/<user-id>/`. Both are created, along with
any missing parent directories, on first use. Schema migrations run
automatically at startup and are idempotent. Backup is `cp` of the data
directory (stop the daemon first, or copy the WAL files with it).

## Consolidation

```toml
[consolidation]
enabled = true
schedule = "daily"              # hourly | daily | weekly
similarity_threshold = 0.92
```

The nightly tidy-up. Three things happen in one pass, per user:

1. **Expiry.** Memories past their `expires_at` are retired — soft
   deleted, with an audit entry naming the date they hit. `expires_at` is
   a promise that a memory stops being *used*, not that it stops
   existing.
2. **Decay.** `importance` is recomputed from how recently and how often
   each memory was actually recalled, and feeds recall ranking as a
   bounded multiplier. It only ever demotes, and never below a floor well
   above zero: a decision from last year that nobody has looked at since
   must lose ties, not disappear.
3. **Merging.** Near-duplicates within one user's category are grouped
   and replaced by the single memory that says what all of them said. The
   originals are superseded, not deleted, each with a `merge` audit entry
   carrying the model's reasoning.

Only the third needs a provider.

### Running it by hand

```bash
recuerdos-ai consolidate --dry-run   # report what would merge; changes nothing
recuerdos-ai consolidate             # apply
```

`--dry-run` calls no model, so it costs nothing to run. It prints each
cluster's contents rather than its ids, because the point is for a person
to judge whether the grouping is right.

### `schedule`

An interval from process start, not a wall-clock time. A cron expression
would let you pick 3am, which sounds better than it is: Recuerdos AI runs
on laptops, and a laptop is asleep at 3am. The first run is one interval
*after* startup, so a daemon that restarts often does not consolidate —
and pay for it — every time.

### `similarity_threshold`

How alike two memories must be to be *considered* the same thing, as
cosine similarity of their embeddings. High on purpose: it is the only
thing standing between a chain of loosely-related memories and a merge
that loses a fact.

Passing the threshold only earns a memory a place in a cluster. The model
gets the last word and is prompted to decline when unsure — "prefers
pnpm" and "prefers Vitest" sit very close in embedding space and are two
different true things.

## The profile digest

`GET /v1/profile` and the MCP `memory://profile` resource return a
briefing on the user, capped at roughly 1500 tokens.

With `[understanding].provider` set, it is written by a model and cached
until the memories under it change — so it compresses rather than listing
and truncating. Staleness is detected by comparing the memories against
what the digest was built from, so no write path has to remember to
invalidate anything.

Without a provider, it is assembled from the highest-value memories per
category instead. Same route, same media type, same shape.
