# Retrieval quality

**Status: Phase 4.**

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
| `category-filter`, `tag-filter` | Filters narrow correctly and do not drop valid hits |

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
