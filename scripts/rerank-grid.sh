#!/usr/bin/env bash
# Phase 7.2 rerank tuning grid (reranking-plan.md): one seeded eval corpus
# measured across model × strategy × depth × weight, printing the table §12
# records with Δs against the `off` reference row.
#
# The measurement lives in the binary (`eval --grid`) rather than in this
# script on purpose: every cell must share one store, one embedder session
# and one keyword-index open, so a cell differs from the reference only in
# the second stage being judged. This file is the operator-facing entry
# point that pins how the grid is invoked.
#
# Defaults (the plan's table):
#   strategies  rrf,replace
#   depths      10,20,30,50   (50 clamps to the candidate pool — limit 5
#                              implies a pool of 40 — exactly like production)
#   weights     0.5,1,2,4     (rrf rows only; replace has no weight)
#   models      bge-reranker-base  (the tuned [rerank].model — a bare grid
#                              run measures what ships)
#
# The model axis is opt-in past the default because the larger candidates
# cost ~1.1–2.2 GB on disk and ~1.5 GiB RAM per session (Phase 7 record).
# To compare against the old placeholder on the cheap (147 MB):
#
#   scripts/rerank-grid.sh --grid-models bge-reranker-base,jina-reranker-v1-turbo-en
#
# Paired latency reading (the ship gate's "added latency"): subtract the
# `off` row's p95 from the chosen cell's p95 — same machine, same embedder,
# same run. The `rerank p50/p95` column is the stage boundary alone and is
# what `[rerank].timeout_ms` must contain.
#
# `--json` prints the machine-readable artifact instead of the table;
# everything else passes through to `eval --grid`, including `--config`
# (production-like runs need the provider key in the environment).
#
# Set RA_NATIVE=1 to run against a local toolchain instead of the dev
# container (same flags).
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${RA_NATIVE:-0}" == "1" ]]; then
    exec cargo run -q --bin recuerdos-ai -- eval --grid "$@"
fi

# `-T`: no TTY. The grid streams progress lines; nobody is attached, and a
# pseudo-TTY would interleave them with cargo's progress bar.
exec docker compose run --rm -T dev cargo run -q --bin recuerdos-ai -- eval --grid "$@"
