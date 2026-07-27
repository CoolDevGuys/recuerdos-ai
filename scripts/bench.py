#!/usr/bin/env python3
"""Load generator for the performance numbers in docs/performance.md.

Driven by `scripts/bench.sh`, which brings up an isolated daemon first —
running this against a store you care about would fill it with tens of
thousands of synthetic memories.

    python3 scripts/bench.py --url http://127.0.0.1:7070 \\
        --seed 100000 --queries 500 --concurrency 32

Measures three things, because they fail differently:

- **Ingest throughput** — `:direct` writes/second. Bounded by the local
  embedding model, so this is really "how fast can this machine embed".
- **Ingest ack** — how long `POST /v1/memories` takes to return a job id.
  The work happens off the request path, so this should stay flat however
  large the store gets. If it does not, something is doing work inline
  that should not be.
- **Recall latency** — end-to-end, and the server's own `took_ms`
  alongside. The gap between them is auth plus transport, which on a
  local daemon should be small; when it is not, that is worth knowing
  before optimising the wrong half.
"""

from __future__ import annotations

import argparse
import concurrent.futures as futures
import json
import statistics
import sys
import time
from typing import Any

import httpx

# Enough overlap between memories that the keyword leg has something to
# disagree with the vector leg about. A corpus of unique nonsense would
# make hybrid search look better than it is, because nothing competes.
TOPICS = [
    "pnpm",
    "typescript",
    "hetzner",
    "sqlite",
    "tantivy",
    "rust",
    "axum",
    "docker",
    "argon2",
    "tokio",
]


def percentiles(samples: list[float]) -> dict[str, float]:
    if not samples:
        return {}
    ordered = sorted(samples)

    def at(p: float) -> float:
        # Nearest-rank. With a few hundred samples the difference from
        # interpolation is noise, and this cannot report a latency that
        # was never actually observed.
        index = max(0, min(len(ordered) - 1, round(p / 100 * len(ordered)) - 1))
        return ordered[index]

    return {
        "p50": at(50),
        "p95": at(95),
        "p99": at(99),
        "max": ordered[-1],
        "mean": statistics.fmean(ordered),
    }


def seed(url: str, key: str | None, count: int, concurrency: int) -> dict[str, Any]:
    """Fills the store, and reports how fast it went."""
    headers = {"Authorization": f"Bearer {key}"} if key else {}
    # Contiguous slices rather than `count // concurrency` per worker:
    # integer division silently drops the remainder, so a run asking for
    # 100000 would quietly seed 99968 and report a corpus size that was
    # never in the store.
    bounds = [count * i // concurrency for i in range(concurrency + 1)]

    def worker(index: int) -> int:
        written = 0
        with httpx.Client(base_url=url, headers=headers, timeout=120) as client:
            for n in range(bounds[index], bounds[index + 1]):
                topic = TOPICS[n % len(TOPICS)]
                body = {
                    "content": (
                        f"memory {n}: the project uses {topic} for part "
                        f"{n % 50} of the stack"
                    ),
                    "category": "fact.project",
                }

                # Retried, because a seed of this size runs for tens of
                # minutes and one dropped connection should not throw the
                # run away. Deliberately not counted as a latency sample:
                # seeding measures throughput, and the latency numbers
                # come from the recall phase where nothing is retried.
                for attempt in range(3):
                    try:
                        response = client.post("/v1/memories:direct", json=body)
                        if response.status_code == 201:
                            written += 1
                        break
                    except httpx.HTTPError:
                        if attempt == 2:
                            raise
                        time.sleep(0.5 * (attempt + 1))
        return written

    started = time.perf_counter()
    with futures.ThreadPoolExecutor(concurrency) as pool:
        written = sum(pool.map(worker, range(concurrency)))
    elapsed = time.perf_counter() - started

    return {
        "requested": count,
        "written": written,
        "seconds": round(elapsed, 1),
        "per_second": round(written / elapsed, 1) if elapsed else 0,
    }


def measure_recall(
    url: str, key: str | None, queries: int, limit: int
) -> dict[str, Any]:
    """Serial queries: percentiles of a contended server measure the load
    generator as much as the server, and the target is a single client's
    experience."""
    headers = {"Authorization": f"Bearer {key}"} if key else {}
    end_to_end: list[float] = []
    server_side: list[float] = []

    with httpx.Client(base_url=url, headers=headers, timeout=120) as client:
        for i in range(queries):
            topic = TOPICS[i % len(TOPICS)]
            started = time.perf_counter()
            response = client.post(
                "/v1/memories/search",
                json={
                    "query": f"what do we use {topic} for in the stack",
                    "limit": limit,
                },
            )
            end_to_end.append((time.perf_counter() - started) * 1000)
            if response.status_code == 200:
                server_side.append(float(response.json().get("took_ms", 0)))

    return {
        "queries": queries,
        "end_to_end_ms": percentiles(end_to_end),
        "server_ms": percentiles(server_side),
    }


def measure_ingest_ack(url: str, key: str | None, count: int) -> dict[str, Any]:
    """How long `POST /v1/memories` takes to hand back a job id.

    Should be flat regardless of store size: the pipeline runs in a
    worker, and the only inline work is one row insert.
    """
    headers = {"Authorization": f"Bearer {key}"} if key else {}
    samples: list[float] = []

    with httpx.Client(base_url=url, headers=headers, timeout=120) as client:
        for i in range(count):
            started = time.perf_counter()
            client.post(
                "/v1/memories",
                json={"content": f"ack probe {i}: we moved logging to Loki"},
            )
            samples.append((time.perf_counter() - started) * 1000)

    return {"requests": count, "ms": percentiles(samples)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:7070")
    parser.add_argument("--api-key", default=None)
    parser.add_argument("--seed", type=int, default=100_000)
    parser.add_argument("--queries", type=int, default=500)
    parser.add_argument("--ack-probes", type=int, default=200)
    parser.add_argument("--concurrency", type=int, default=32)
    parser.add_argument("--limit", type=int, default=8)
    parser.add_argument("--json", dest="json_out", default=None)
    parser.add_argument(
        "--skip-seed",
        action="store_true",
        help="Measure against a store that is already populated.",
    )
    args = parser.parse_args()

    with httpx.Client(base_url=args.url, timeout=30) as client:
        for _ in range(120):
            try:
                if client.get("/healthz").status_code == 200:
                    break
            except httpx.HTTPError:
                pass
            time.sleep(1)
        else:
            print(f"no daemon at {args.url}", file=sys.stderr)
            return 1

    report: dict[str, Any] = {"url": args.url, "corpus": args.seed}

    if not args.skip_seed:
        print(f"==> seeding {args.seed} memories (concurrency {args.concurrency})")
        report["ingest"] = seed(args.url, args.api_key, args.seed, args.concurrency)
        print(
            f"    {report['ingest']['written']} written in "
            f"{report['ingest']['seconds']}s "
            f"= {report['ingest']['per_second']}/s"
        )

    print(f"==> {args.ack_probes} ingest acks")
    report["ingest_ack"] = measure_ingest_ack(args.url, args.api_key, args.ack_probes)
    print(f"    p95 {report['ingest_ack']['ms']['p95']:.1f}ms")

    print(f"==> {args.queries} recall queries")
    report["recall"] = measure_recall(args.url, args.api_key, args.queries, args.limit)

    e2e = report["recall"]["end_to_end_ms"]
    srv = report["recall"]["server_ms"]
    print(
        f"    end-to-end  p50 {e2e['p50']:.1f}ms  "
        f"p95 {e2e['p95']:.1f}ms  p99 {e2e['p99']:.1f}ms"
    )
    print(
        f"    server-side p50 {srv['p50']:.1f}ms  "
        f"p95 {srv['p95']:.1f}ms  p99 {srv['p99']:.1f}ms"
    )

    if args.json_out:
        with open(args.json_out, "w") as handle:
            json.dump(report, handle, indent=2)
        print(f"==> wrote {args.json_out}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
