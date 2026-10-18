# Recall quality: what is measured, what shipped, what is still open

Snapshot: **2026-10-18**. This page is the state of the work in one read. The spec it implements is
[`recall-quality-plan.md`](recall-quality-plan.md); the raw log behind every number here is that file's
**§17 Results log**, which stays the authority — if this summary and §17 disagree, §17 is right and
this page is stale.

Nothing here is a projection. Every figure was measured on one of two instruments, both named beside
it, and the ones that could not be measured are listed as not measured rather than estimated.

---

## The short version

Two knobs and one stage were tuned on a synthetic corpus, then checked against a real deployed store
of 182 live memories using their owner's own 18 questions.

- **Relevance weight: the corpus understated it.** Shipped `0.05`; the field host read the same
  direction with ~1.6× the effect. **Now `0.5`.** On neither instrument, at any tested value, did
  anything holding first place lose it.
- **The rerank stage is not useless — it is muted by its own fusion.** Off stays right at the shipped
  fusion, which changed almost nothing for 230 ms and 1.4 GiB. Re-fused to read the model's order
  instead of voting it in, the same stage on the same queries reads **83.3 p@1 against 38.9**. That is
  recorded, not shipped: the eval grid says the opposite about that fusion, and the disagreement is now
  the project's central open fact.
- **The graph's reserved seat costs a little here** (−2.2 MRR, top-1 unchanged) for a benefit this
  store does not demonstrate. Default kept, price recorded.
- **The later phases mostly have no field-observed target.** Their two motivating failures turned out
  to be memories that were never stored — an absence, not a ranking error — and every answerable
  question is already complete inside five. Phase 5 (vocabulary gaps at write time) is the exception
  with a live, still-failing corpus number, and it is the expensive one.

---

## 1. Shipped

| Version | Contains | State |
|---|---|---|
| `v0.4.0` | Stopword-free BM25, per-model similarity baseline, `[rerank]` stage **off by default** | released |
| `v0.4.3` | Python SDK publication only | released |
| `v0.5.0-rc1` | Phases 0–3: δ = 0.05, graph reserved seat, bounded rerank deadline | prerelease, published |
| **unreleased** | **δ default `0.05` → `0.5`** | this branch |

The operator-facing summary of each is in [`docs/CHANGELOG.md`](../docs/CHANGELOG.md); the knob-by-knob
reference is [`docs/configuration.md`](../docs/configuration.md).

---

## 2. The numbers, by instrument

### 2a · The eval corpus (`eval/cases.toml`, `bge-small-en-v1.5`)

Synthetic, 74 scored cases of which 22 are marked informational and excluded from the gate; the gated
baseline is the 59 cases recorded in [`eval/baseline.json`](../eval/baseline.json). Its known bias:
expected memories are the topically nearest ones, so it rewards relevance partly by construction. That
bias is why δ was held at 0.05 for two releases, and it is stated here because it is the reason the
field instrument exists at all.

Relevance weight, against the `0.05` baseline on the same build:

| δ | recall@5 | precision@1 | MRR | complete@5 | abstention |
|---|---|---|---|---|---|
| 0 | — | 50.0 | — | — | 100.0 |
| 0.05 | 87.2 | 53.8 | 68.4 | 84.6 | 100.0 |
| 0.2 | — | 63.4 | — | — | 100.0 |
| **0.5** | **87.8** | **67.3** | **75.9** | **84.6** | **100.0** |

δ = 0.5 versus `0.05`, kind by kind: **no kind's precision@1 fell**; four rose — `near-miss`
57.1 → 100.0, `statement` 25.0 → 75.0, `needle` 33.3 → 66.7, `nl-category` 50.0 → 62.5 — and one
kind's recall rose (`nl-category` 66.7 → 70.8). Latency is noise-level (p50 8.0 → 8.5 ms).

Two structural facts, both verified rather than reasoned:

- **δ cannot move abstention.** The floor compares `relevance`; δ multiplies `score`. 100.0 at every
  value, which is why a larger δ carries no risk of a wrong "no match".
- **δ is applied before truncation**, so it moves the top-`limit` *window* as well as the order inside
  it. On both instruments that went towards better answers — never away. Measured, not guaranteed.

### 2b · The field host (`lamalamita`, `v0.5.0-rc1`, one person's real store)

18 questions written by the store's owner, replayed read-only by
[`scripts/field-measure.sh`](../scripts/field-measure.sh) with the rerank stage off, δ as marked:

| δ | precision@1 | MRR | complete@5 |
|---|---|---|---|
| 0 | 27.8 | 47.7 | 83.3 |
| 0.05 (what shipped) | 38.9 | 55.7 | 88.9 |
| 0.2 | 44.4 | 59.7 | 88.9 |
| **0.5** | **50.0** | **63.4** | 88.9 |

Zero rank-1 losses at every step, on both instruments — the load-bearing sentence of the whole change.
The absolute numbers are low because the store repeats project names and the questions are precise
about which project they mean; that is also what makes δ worth so much here.

**Two of the eighteen are not failures of any kind.** The GPU questions (10, 11) ask about memories
that are not in the store: quoted back at the index, `"Radeon 8060S Graphics"` tops out at relevance
0.09 and `"RTX 3090 24GB"` at 0.10, where a stored memory quoted to itself arrives near 0.9. The
honest denominator is 16 answerable questions, and **16 of 16 are complete inside five**. The plan's
canonical "recall failures" were absences.

### 2c · What a recall costs, warm

| configuration | recall p50 | recall p95 | resident |
|---|---|---|---|
| rerank stage **off** | 6.3 ms | 9.1 ms | 199 MB |
| rerank stage **on** | 236 ms | 602 ms | 1.67 GB |

The base-vs-base noise pass ran in the same script and read **6.1 vs 7.4 ms** (6.3 vs 7.5 in the
earlier run), so a difference of a millisecond or so in any number above is noise, not an effect. The
stage is ~37× the latency and ~1.4 GiB — which is why "it changes nothing at weight 1" mattered enough
to be measured twice.

### 2d · The fusion question (M5)

Same store, same 18 queries, **the stage on in every row**, δ = 0.05 — so the only variable is how the
model's judgement enters the order:

| configuration | precision@1 | MRR | complete@5 |
|---|---|---|---|
| `rrf`, weight 1 — **the shipped fusion** | 38.9 | 54.6 | 88.9 |
| `rrf`, weight 2 | 44.4 | 56.9 | 88.9 |
| **`strategy = "replace"`** | **83.3** | **85.2** | 88.9 |

Eight queries gained first place, **none lost it**; `complete@5` never moves, so nothing here finds
new answers — it is purely what happens to an answer already inside the window. On the 16 answerable
questions, `replace` puts the expected memory first **15 times**.

`weight = 2` is the control that keeps this from being a wish: doubling the reranker's voting weight
bought exactly one query and moved another top-1 between two wrong answers. So the finding is not
"more reranker is good" — **reading the model's order works; voting the model in as another leg does
not**, which is what the two strategies mean arithmetically.

The same run argues against itself, and the fact is kept in §17 rather than buried: where the model has
nothing to say, its whole head scores 0.000–0.037 (query 17 has the right memory in the window and
rates everything at nothing). That is exactly the defect the eval grid recorded when `replace` lost up
to **−9.6 p@1 on three of four models**. `replace` wins big where the head has signal (16 of 18
queries) and does nothing where it has none (2 of 18).

---

## 3. Decisions taken

| Decision | Evidence | Worth |
|---|---|---|
| **δ default → 0.5** | Both instruments, four values, zero rank-1 losses | +13.5 p@1 on the corpus, +22.2 on the store, free |
| **`[rerank].mode = "off"` stays** | Fused at weight 1 it moved almost nothing and cost 230 ms + 1.4 GiB | Correct, but no longer a verdict on the stage |
| **`[graph].expansion_slots = 1` stays** | MRR −2.2 on the field store, top-1 unharmed, mechanism understood | Price recorded, Phase 8 revisits |
| **Phase 8 stays a small release** | Nothing in it needs a phase | — |

## 4. Decisions deliberately *not* taken

**`strategy = "replace"` is not the default.** 44 points on one person's store is not enough to flip a
shipped fusion, and that is the entire reason a gate exists when the number is this tempting:

- The eval grid, same binary and same model, says `replace` **loses** up to −9.6 p@1.
- The field query file is needle-shaped by construction — each `expect` names a memory whose text
  contains the answer phrase — which is what a cross-encoder is best at and what the corpus's
  paraphrased cases were deliberately not.
- Both instruments are internally consistent, so one of them is describing the wrong population.

What would settle it, in order: a **frozen copy** of a real store inside the harness (the corpus the
grid can be re-run against — one now exists locally, see §7), a **rerank grid re-run at the new δ**
(the published fusion choice was made at 0.05, so the head the stage sees has since changed), and the
shape most likely to survive both readings is not a strategy default at all but a **confidence gate**:
serve the model's order where the head's top score clears a threshold, keep the fused order where the
head is flat — with the threshold calibrated by the distribution printer in reranking-plan §12 rather
than guessed.

## 5. Phases 4–7: stopped as written, with one live candidate among them

The redirect that ended the phase sequence was *"let measured failures choose the next phase"*. So
each remaining phase is graded against its own target class rather than cancelled as a block — and the
grades differ, which matters more than the tidiness of stopping all four.

| Phase | Target class | Seen in the corpus? | Seen on the field store? | Verdict |
|---|---|---|---|---|
| 4 · embedding prompts | a non-default embedder silently losing quality | not measurable — the corpus runs the default model | no: the host runs `bge-small-en-v1.5` | **stopped.** Needs a second embedder deployed, not a phase |
| 5 · question probes | vocabulary gaps | **yes, still failing: `vocab-gap` recall 14.3%, p@1 0.0%** (1 of 6 cases answered) | untested — the 18 questions contain no vocabulary-gap question | **the one live candidate**, and the expensive one (an LLM at write time) |
| 6 · aliases | fragmented entity neighbourhoods | no case fails for this reason | no: 280 entities / 61 relations, no query hurt by a split concept | **stopped** |
| 7 · temporal cues | "what did we use *before*?" | yes in rank-1 terms (`temporal` p@1 0.0% over 5 cases) | no: no temporal question was asked, so nothing to fix | **stopped pending a real temporal question**; the corpus-side number is a known weakness, not a field failure |

Two of the eighteen field queries that *looked* like recall failures — the two GPU questions — were
**memories that were never stored**, established by quoting them back at the index (rel 0.09 / 0.10,
against ~0.9 for a memory quoted to itself). They had been cited as motivation since §1 of the plan.
That is why "no field target" is the honest reading for most of these phases, and why the plan keeps
their text: if a bigger corpus (Task 0.9) or a second operator shows the failure, the phase reopens
with evidence instead of with a theory.

Phase 8 survives as a small release: the defaults are now all set from measurements, `mode = "off"` is
documented with its real-store numbers, and there is nothing left in it that needs a phase.

## 6. Still open, ordered by how much it could change

1. **The 44-point disagreement about `replace`** — §2d versus the grid. Larger than anything Phases
   4–8 were written to buy. Needs the frozen copy in the harness.
2. **Task 0.9's real corpus.** A store copy now exists (§7) with its digest recorded; what it needs is
   ~40 query/expect pairs and the existing 18 re-anchored — three of them encode last week's answer,
   which is why field absolutes moved between runs. This is also the only way to learn whether the one
   failure class the corpus still shows (`vocab-gap`, recall 14.3%) exists outside synthetic cases,
   which is the whole argument for or against Phase 5.
3. **M1's hand review.** The seat's aggregate says "costs a little"; only reading the arriving rows
   says whether they are sentences a human would want.
4. **Phase 1's Task 1.5** (queue vs refusal in the daemon's own log) — needs `RUST_LOG=debug` for a
   few minutes and the stage turned back on.
5. **The rerank grid at δ = 0.5** — the fusion choice predates this δ change.

## 7. Reproducing this

The field store copy is frozen and identified:

```
eval/store-2026-10-18.tar.gz   sha256 b20fda44397bfa87eb91f51f970792810935b0225d04ac7966c0385d445bfa17
eval/field-20261018-1941.tar.gz  sha256 c7b15e55f3681bc159f130f2ca1aaf90a0bd89b200e486df4d64b97606729f20
```

225 rows, 43 of them superseded, **182 active**, newest write `2026-10-18T16:23:26Z`;
`PRAGMA integrity_check` returns `ok`. The copy postdates the run whose numbers are in §2b, so those
numbers and this file describe the same state.

**These two files are private and are not in git** — a store copy *is* someone's memories, and the
measurement bundles quote them in their `.txt` audits. They are matched by `.gitignore`; what is
committed from them is this digest and the aggregates above. Anyone reproducing §2b needs the copy from
its owner, not from the repository.

To replay a knob change against it locally:

```bash
tar xzf eval/store-2026-10-18.tar.gz -C /tmp/frozen
RECUERDOS_AI_STORAGE__PATH=/tmp/frozen recuerdos-ai recall audit \
    --config a.toml --compare-config b.toml \
    --queries questions.txt --user <handle> --limit 5
```

`recall audit` writes nothing, so it can also be run against a live daemon. Note when reading its
output that the ids it prints are the **last eight characters** of the memory UUIDs.

## 8. What the instrument changes about the method

The plan's §4 gate always read *"and the same comparison on a real store"*, and for two releases that
half was unmeetable because the deployed box predated the knobs. Now it is one script and one question
file. Three things this run should permanently change about how the project tunes:

- **A knob's default is not settled until it has been falsified on data nobody wrote for the test.** δ
  passed that test; the fusion failed it in the other direction, which is the same success.
- **An instrument on a live store drifts while you use it.** Six memories about this work landed in the
  store between runs and moved the shipped config's absolutes from 50.0 to 38.9 p@1. Differences
  measured within a run stay valid; absolutes do not compare across runs, and query files rot — even a
  prediction's comparison threshold rotted mid-project.
- **The tempting number and the gate are both information.** The gate exists precisely for the moment
  when one instrument says +44 and the other says −9.6; the answer is a better instrument, not the
  bigger sign.
