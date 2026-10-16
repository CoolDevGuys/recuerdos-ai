# Retrieval quality

**Status: Phase 4; `relational` cases and the graph leg added in 7.3.6–7.3.7.**

The service is a bet that asking a question returns the memory that
answers it. Nothing else in the test suite protects that.

Retrieval quality is not a property of any one component — it emerges
from the embedding model, the BM25 tokenizer, the RRF constant, the
recency multiplier and the candidate depth. A one-line change to any of
them can make recall noticeably worse with every existing test still
green. That already happened once: in Phase 2 a recency floor of `0.5`
quietly made freshness outrank relevance, and the only reason it was
caught was a test written specifically to assert the intent.

`recuerdos-ai eval` is the generalisation of that test.

## Running it

```bash
just eval          # score against the committed baseline
just eval-record   # re-record the baseline after a deliberate change
```

It builds a throwaway instance in a temp directory, seeds
[`eval/cases.toml`](../eval/cases.toml), runs each query through the real
recall path, and prints a table:

```
case                                                   kind  recall  top
────────────────────────────────────────────────────────────────────────
paraphrase: package manager                      paraphrase    100%    ·
exact token: useQuery                           exact-token    100%    ✓
needle: dietary requirement                          needle      0%    ·
    missed: User is vegetarian and does not eat fish
────────────────────────────────────────────────────────────────────────
recall@5: 92.3%   precision@1: 76.9%   (13 cases)
            needle: 66.7%
        paraphrase: 100.0%
```

It runs against the **real** embedding model, not the deterministic fake
the unit tests use. The fake exists to be predictable rather than good,
so scoring against it would measure nothing.

## What is measured

| Metric | Meaning |
|---|---|
| `recall@5` | Of the memories a case says are correct, what fraction reached the top 5 |
| `precision@1` | How often the single best-ranked result was one of the correct ones |

Five, because that is roughly what fits in an agent's context window
alongside an actual conversation. A memory ranked eighth is not wrong; it
is never seen.

Scores are also broken down by `kind`. An overall figure that held steady
while one kind collapsed and another improved is exactly the regression a
single number hides.

| `kind` | The failure it catches |
|---|---|
| `paraphrase` | A query with no words in common with the memory. Keyword search alone cannot answer these |
| `exact-token` | An identifier like `useQuery`. Vector search alone cannot answer these — this is why retrieval is hybrid |
| `current-fact` | The read side of reconciliation: a superseded answer must not come back |
| `needle` | One relevant memory in a corpus of unrelated ones |
| `relational` | The answer lives in a memory that never names the query's subject — reachable only by walking the entity graph a hop or two ("who leads the team that owns the billing service?"). This is the case the graph leg exists for; on the committed corpus it scores 85.7%, up from 71.4% with the graph off |
| `category-filter`, `subcategory-filter`, `tag-filter` | Filters narrow correctly and do not drop valid hits |

## The gate

CI runs `--baseline eval/baseline.json --max-drop 5` on every pull
request and fails if `recall@5` drops more than five points.

A threshold rather than exact equality, deliberately. These scores move a
little with model versions and tokenizer changes, and a gate that fails
on noise gets disabled within a week.

When it fails, there are exactly two possibilities and you have to decide
which:

1. **The change made retrieval worse.** Fix the change. The per-case
   `missed:` lines say which queries broke.
2. **The eval set moved on** — you deliberately changed the embedding
   model, or rewrote a seeded memory. Re-record with `just eval-record`
   and commit the new baseline, so the diff shows the score change as a
   reviewable decision rather than a silent slide.

## Adding cases

This is the point of the file. When a recall goes wrong in real use, the
fix starts by writing the case:

```toml
[[memory]]
content = "User prefers Vitest over Jest for anything new"
category = "preference.coding"
tags = ["testing", "typescript"]

[[case]]
name = "paraphrase: test runner"
kind = "paraphrase"
query = "what should I use to write tests"
expect = ["User prefers Vitest over Jest for anything new"]
```

`expect` names memories by exact content; a unit test checks that every
expectation names something actually seeded, so a typo fails `cargo test`
rather than silently scoring zero forever.

**A case may be committed failing.** One is: "book a restaurant for the
team dinner" does not surface the user's dietary preference, because the
query and the memory share no vocabulary and `bge-small-en-v1.5` does not
place them close enough. It is left in because an eval set containing
only cases that already pass measures nothing, and because it is a real
query — the agent booking dinner is precisely when that memory matters.
It is the concrete target for a better embedding model or a
query-expansion step.

The corpus is deliberately larger than the cases need. With six memories
almost any ranker scores perfectly; the filler exists so a case has
something to be wrong about.

## Judging a second stage on your own memories

`recuerdos-ai eval` answers "did this change break retrieval?" against a
corpus someone wrote. It cannot answer the operator's question — *would
enabling the reranker change what my agent sees, in my store, for queries I
actually ask?* — because 35 seeded memories written by whoever built the
feature are not the eight hundred you accumulated.

`recuerdos-ai rerank audit` is that measurement:

```bash
recuerdos-ai rerank audit --config recuerdos-ai.toml --queries queries.toml
```

```toml
# queries.toml — or one query per line, `#` for comments
[[query]]
text = "which package manager do we use?"
expect = "prefers pnpm"   # a substring of the memory that should come first

[[query]]
text = "who owns the billing service?"
```

Each query runs twice over the real store: once through the pipeline as it
serves today, once with the second stage switched on. Both orderings are
printed with ids, category, relevance, the model's score and where each
result moved, and the run ends with the share of queries whose top result
changed, mean rank displacement, p50/p95 rerank latency, and p@1 / MRR before
vs after for the queries that carried an `expect`.

What it will not do:

* **It changes nothing.** Recall runs untracked, so access counters,
  `last_accessed_at` and importance are byte-identical afterwards — a replay
  that stamped the memories it read would feed importance decay and quietly
  change the ranking it came to measure. A test asserts it.
* **It does not need the daemon stopped.** The keyword index is opened
  read-only, so it never asks for the writer lock a running daemon holds. If
  the store has no keyword index yet it refuses and says to run
  `recuerdos-ai reindex --text` rather than auditing a half-dead pipeline.
* **It is not `rerank all`.** No command writes rerank scores to stored
  memories: a rerank score is a property of a query and the candidate list it
  produced, not of a memory, so storing one would be a cache with no way to
  invalidate it. This command, reading and reporting, is the supported way to
  run the stage over an existing corpus.

Two things it does differently from a live request, both stated in the report
header, because a report that quietly differs from production is how you end
up shipping the wrong conclusion: it applies no relevance floor (near-misses
are what you came to look at, and dropping results before the reranker sees
them would change what it reorders), and no rerank deadline (measuring what
the model costs is half the point — the configured `timeout_ms` is printed
beside the percentiles it has to contain).

And one thing the report now says out loud. Where the stage did not contribute
to a query — the model failed, or `--depth 1` left no head two candidates wide
— the summary opens with how many queries it actually reranked. A run where the
stage did nothing used to look exactly like a run where reranking agreed with
fusion, and "no disagreements found" is precisely the wrong conclusion to draw
from that. On the live surface the same fact is visible per request as
`rerank.state` ([api.md](api.md)).

`--json` emits the same run as ids, ranks and scores, with no memory content —
so a report can be committed or pasted into an issue without carrying anyone's
memories with it.

## Reading the shadow log

`[rerank].mode = "shadow"` is the same judgment on traffic you did not have to
think up: every recall runs the stage, logs the comparison, and serves the
**old** order. One line per recall, `rerank_shadow`, carrying
`query_hash`, `depth`, `rerank_ms`, `top1_before`, `top1_after`, `changed`,
and the two full orderings as `before` / `after`. Ids are the last eight
characters of the UUIDv7 — the head is the creation timestamp and would repeat
across memories saved together. No query text and no memory content appears in
any field.

The number to read out of it is the **disagreement rate** — how often the
second stage would have changed the answer:

```bash
# the default text log: judged recalls, and of those how many moved the head
grep -c 'rerank_shadow' daemon.log
grep 'rerank_shadow' daemon.log | grep -c 'changed=true'
```

or, with `RECUERDOS_AI_LOG=json`:

```bash
jq -s '[.[] | select(.fields.message == "rerank_shadow")]
       | {recalls: length,
          changed: (map(select(.fields.changed)) | length),
          rerank_ms_p95: (map(.fields.rerank_ms) | sort
                          | .[((length * 0.95) | floor)])}' daemon.log
```

Read it as a rate with its denominator, and read it against latency: a 3%
disagreement rate at 400 ms p95 means the stage mostly agrees and costs a
deadline's worth of time — hard to justify. A high rate says the stage has
something to say, which is when the audit above matters: the log tells you how
often the answer would change, and only a human reading the two orderings says
whether that is for the better.
