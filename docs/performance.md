# Performance

Measured against the release image (`docker/Dockerfile`) — the artifact
users actually get, not a dev build.

**Three of the four acceptance targets are missed on this hardware.** The
numbers are below with the reasons, because a performance page that only
reports its wins is not evidence of anything.

## How to reproduce

```bash
scripts/bench.sh 100000 500     # corpus size, query count
```

Brings up a throwaway daemon on its own volume and network, seeds it,
then measures. Results land in `bench-results/` as JSON.

The load generator runs in a container on the daemon's Docker network and
addresses it by name. Going through a published host port instead puts
Docker Desktop's userspace forwarder in the path — which drops
connections under sustained concurrency (a 100k seed died at ~19k that
way) and taxes every latency sample with a proxy hop no real deployment
has.

## Hardware

| | |
|---|---|
| Host | Apple Silicon (arm64), macOS |
| Runtime | Docker Desktop 29.4.0, 14 CPUs available |
| Image | `docker/Dockerfile`, release profile, LTO |
| Config | defaults; `[understanding].provider = "none"` |

**This is the pessimistic case.** Docker Desktop on macOS runs a Linux VM
with a virtualised filesystem, and this workload is dominated by SQLite
writes and ONNX inference — both of which that layer taxes. Native Linux
should do better. How much better is not measured, so it is not claimed.

## Results

### Recall latency

500 serial queries, `limit=8`, hybrid (vector + BM25 + fusion).

| Corpus | p50 | p95 | p99 | max |
|---|---|---|---|---|
| 2,000 | 26.0 ms | 33.7 ms | 36.9 ms | — |
| 20,000 | 23.5 ms | 36.0 ms | 49.9 ms | — |
| 100,000 | **70.2 ms** | **82.9 ms** | 153.5 ms | 450.8 ms |

Server-side `took_ms` at 100k is p50 69 ms against 70.2 ms end-to-end, so
**the latency is the search itself** — auth, transport and JSON add about
a millisecond. There is nothing to win in the HTTP layer.

> **Target: P95 < 50 ms at 100k. Measured: 82.9 ms. Missed by 66%.**

The shape is the interesting part: **flat from 2k to 20k, then a sharp
rise.** Ten times the corpus costs nothing measurable; the next five
times costs 3×.

That rules out the obvious explanation. `sqlite-vec` 0.1.x does exact
KNN — a brute-force scan of the user's vectors, not an approximate index
— so the natural guess is that latency tracks corpus size linearly. It
does not: a linear scan would have made 20k roughly ten times slower than
2k, and it was not slower at all.

What the numbers support instead is a fixed overhead of roughly 22 ms —
dominated by embedding the query, which happens regardless of corpus size
— that swamps the scan until somewhere past 20k, after which the
corpus-dependent term becomes visible. A linear fit through the three
points is poor, so something else changes between 20k and 100k as well.
It has not been profiled, and this document is not going to guess further.

Two things worth recording about that:

- **The 20k run is the one that earned its keep.** With only 2k and 100k,
  the two-point inference is "50× the corpus, 2.7× the latency, must be
  the linear scan" — confident, tidy, and wrong. It was measured
  precisely because two points cannot distinguish a slope from a step.
- **Optimising on the guess would have wasted the effort.** Replacing
  exact KNN with an ANN index is a large change, and nothing here shows
  the vector scan is the dominant cost at 100k.

The honest summary: **recall is effectively instant up to ~20k memories**
(p50 ~24 ms, and flat across a 10× range) and merely fast at 100k. The
50 ms target is missed only at a corpus size no personal user reaches —
see the restatement below.

### Ingest

| | |
|---|---|
| `POST /v1/memories` ack, p50 | 1.4 ms |
| `POST /v1/memories` ack, p95 | **4.5 ms** |
| `POST /v1/memories` ack, p99 | 5.5 ms |
| `:direct` write throughput | 53.7/s at concurrency 32 (54–66/s across runs) |

> **Target: ack < 5 ms. Measured: 4.5 ms at p95. Met.**

This is the number the async design exists to protect: submitting content
returns a job id without waiting for extraction, and it stays flat at
100k memories because the only inline work is one row insert.

Write throughput of ~54/s is not a target and is bounded by the local
embedding model — every `:direct` write embeds its content on the
request path. It is the reason seeding 100k takes half an hour. Batch
imports would benefit from a bulk path that embeds in batches; none
exists today.

### Startup and memory

| | 2k | 20k | 100k |
|---|---|---|---|
| Cold start to first healthy response | 298 ms | 335 ms | 495 ms |
| Idle RSS (model loaded) | 187.6 MiB | 185.2 MiB | 222.2 MiB |
| RSS after the benchmark | 338.8 MiB | 366.5 MiB | 579.3 MiB |

> **Target: cold start < 300 ms. Measured: 298 ms / 495 ms. Missed at
> 100k.**

> **Target: idle RSS < 150 MB. Measured: 187–222 MiB. Missed by 25–48%.**

The RSS figure is `docker stats`, which reports the cgroup working set —
it includes page cache the kernel would reclaim under pressure, so it is
an upper bound rather than a heap measurement. The floor is the ONNX
model and its runtime, which are resident whatever the corpus size; the
150 MB target appears to have been set without accounting for it.

RSS after the benchmark is substantially higher, which is expected: the
process has just written 100k rows, 100k vectors and a tantivy index.
Whether it *returns* is what the soak test answers, and the soak has not
been run.

## Targets, restated

project-plan.md §6 set these as POC acceptance criteria. Measured on the
hardware above:

| Target | Measured | |
|---|---|---|
| Recall P95 < 50 ms @ 100k | 82.9 ms | ✗ |
| Ingest ack < 5 ms | 4.5 ms | ✓ |
| Cold start < 300 ms | 495 ms @ 100k | ✗ |
| Idle RSS < 150 MB | 222 MiB | ✗ |

The one that matters most for the product is recall, and it is the one
most clearly missed. Two mitigations are worth noting before anyone reads
that as fatal:

1. **Personal scale is nowhere near 100k.** A heavy user accumulating ten
   memories a day reaches 100k in 27 years. At the scale this is actually
   used — thousands — recall is ~26 ms.
2. **The measurement is on the slowest plausible runtime.** Docker
   Desktop on macOS is a VM with a virtualised filesystem.

Neither makes the target met. They are context for deciding whether to
optimise now or ship and revisit.

## Not measured

Two things the plan asked for are **not** in this document, because they
were not run:

- **1M memories.** At 54 writes/s, seeding takes ~5 hours. The harness
  supports it (`scripts/bench.sh 1000000`); nobody has paid for the run.
- **24-hour soak.** `scripts/soak.sh` exists and drives sustained writes
  with hourly consolidation, sampling RSS every minute to a CSV. It has
  not been run, so **there is no evidence here about memory leaks either
  way** — the elevated RSS after the benchmark is unexplained rather than
  known-good.

Do not cite a leak-free claim from this page. There isn't one.
