#!/usr/bin/env bash
# Phase 0.6 recall tuning grid (recall-quality-plan.md): the *first-stage*
# knobs, each measured in its own process — one row per cell against the
# first row as reference, plus per-kind recall / p@1 / complete@k deltas.
#
# Why a shell loop and not `eval --grid`: the rerank grid runs inside one
# process because swapping the reranker never changes the store, so every cell
# can read the same seeded vectors. These knobs are not like that.
# `graph.enabled = false` stops ingest writing edges, so a graph-off cell has
# to seed its own store the way a deployment with that config would; and the
# §1.3 experiment variants are read from `RA_EXP_*` at the call site, which
# only a fresh process can vary. One process per cell is what makes a row mean
# "the config a user could actually boot", and Task 0.1's determinism
# (hash-derived ages on a settable clock) is what makes the rows comparable.
#
# A cell is `--cell LABEL,ENV=VAL[,ENV=VAL…]`. The assignments go to that
# cell's process only (docker `-e`, or `env` on the native path), so one row's
# knob cannot be read by the next. Env overrides win over the config file, so a
# cell beats whatever `--config` says.
#
# Defaults are the plan's candidate knobs at the values worth arguing about:
#
#   reference      the loaded config (graph on @0.3, 90-day half-life)
#   graph-off      no hop at all — and no edges seeded, which is the point
#   graph-rrf      raw three-leg RRF, the weight before Task 7.3.7 tuned it down
#   graph-quiet    the hop as a whisper, far below the tuned 0.3
#   recency-100y   a century half-life, i.e. no decay: `[retrieval]
#                  .recency_half_life_days = 0` is the documented way to mean
#                  "off", but `validate()` refuses 0 as a typo guard, so the
#                  grid says 36500 — 0.5^(age/36500) is 1.00 for anything a
#                  person has actually said
#   recency-30d    aggressive enough to bury anything a month old
#
# `[rerank].mode` is a legal cell too, just an expensive one: `mode = on` loads
# 1.1 GB of cross-encoder, the same reason rerank-grid.sh keeps the large
# models opt-in —
#
#   scripts/recall-grid.sh --cell rerank-on,RECUERDOS_AI_RERANK__MODE=on
#
# and the same reason it is not a default here. A rerank row is only worth
# reading against a rerank-off row in the same grid.
#
# The §1.3 experiment variants (which need
# `ai-specs/experiments/recall-accuracy-2026-10-17.patch` applied) are what
# `--no-defaults` is for, so a re-run measures the variants plus a reference
# instead of six config cells nobody is arguing about:
#
#   scripts/recall-grid.sh --no-defaults \
#     --cell reference \
#     --cell fuse,RA_EXP_FUSE=linear,RA_EXP_GRAPH_SLOT=1 \
#     --cell triples,RA_EXP_TRIPLE_K=6,RA_EXP_TRIPLE_RANK=1
#
# Everything after `--` passes through to `eval`, including `--config`
# (production-like runs need the provider key in the environment). The latency
# column is informational: these are separate processes, so a busy minute moves
# p95 without moving ranking. Read quality from the quality columns and pair
# any latency claim with the same-machine rerank grid instead.
#
# Each cell's full eval output is kept under a temp directory, and its path is
# printed beside the row so a reader can see *which* cases moved. `--keep`
# keeps the directory and prints it last.
#
# Set RA_NATIVE=1 to run against a local toolchain instead of the dev
# container (same flags).
set -euo pipefail
cd "$(dirname "$0")/.."

usage() { sed -n '2,64p' "$0" | sed 's/^# \{0,1\}//'; }

CELLS=()
PASSTHROUGH=()
NO_DEFAULTS=0
KEEP=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --cell)
            [[ $# -ge 2 ]] || { echo "recall-grid: --cell needs a spec" >&2; exit 2; }
            CELLS+=("$2"); shift 2 ;;
        --no-defaults) NO_DEFAULTS=1; shift ;;
        --keep) KEEP=1; shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; PASSTHROUGH+=("$@"); break ;;
        *) echo "recall-grid: unknown flag $1 (see --help)" >&2; exit 2 ;;
    esac
done

if [[ $NO_DEFAULTS -eq 0 ]]; then
    CELLS=(
        reference
        graph-off,RECUERDOS_AI_GRAPH__ENABLED=false
        graph-rrf,RECUERDOS_AI_GRAPH__RANK_WEIGHT=1.0
        graph-quiet,RECUERDOS_AI_GRAPH__RANK_WEIGHT=0.05
        recency-100y,RECUERDOS_AI_RETRIEVAL__RECENCY_HALF_LIFE_DAYS=36500
        recency-30d,RECUERDOS_AI_RETRIEVAL__RECENCY_HALF_LIFE_DAYS=30
        "${CELLS[@]+"${CELLS[@]}"}"
    )
fi
if [[ ${#CELLS[@]} -eq 0 ]]; then
    echo "recall-grid: --no-defaults with no --cell leaves nothing to measure" >&2
    exit 2
fi

OUT=$(mktemp -d "${TMPDIR:-/tmp}/recall-grid.XXXXXX")
trap '[[ $KEEP -eq 1 ]] || rm -rf "$OUT"' EXIT
FACTS="$OUT/facts"
: >"$FACTS"

fail() {
    echo "recall-grid: $1" >&2
    [[ -n "${2:-}" && -s "${2:-}" ]] || return 1
    echo "── ${2##*/} ──" >&2
    cat "$2" >&2
    exit 1
}

run_cell() {
    # $1 = where to write the run's output, $2… = this cell's assignments.
    local out=$1; shift
    local -a docker_env=() host_env=() assignment
    for assignment in "$@"; do
        docker_env+=(-e "$assignment")
        host_env+=("$assignment")
    done
    if [[ "${RA_NATIVE:-0}" == "1" ]]; then
        # `env` scopes the assignments to the child, exactly as `-e` does for
        # the container path.
        env ${host_env[@]+"${host_env[@]}"} \
            cargo run -q --bin recuerdos-ai -- eval ${PASSTHROUGH[@]+"${PASSTHROUGH[@]}"} \
            >"$out" 2>&1
    else
        # `-T`: no TTY, so cargo's progress bar cannot interleave.
        # A compose container does not inherit the host environment, which is
        # why every assignment is passed with `-e` — dropping one silently
        # would measure the same config once per row and print six identical
        # numbers as if they were a grid.
        docker compose run --rm -T ${docker_env[@]+"${docker_env[@]}"} dev \
            cargo run -q --bin recuerdos-ai -- eval ${PASSTHROUGH[@]+"${PASSTHROUGH[@]}"} \
            >"$out" 2>&1
    fi
}

echo "recall grid: ${#CELLS[@]} cells, one process each · $(date -u '+%H:%M:%SZ')"
echo

index=0
for spec in "${CELLS[@]}"; do
    index=$((index + 1))
    label=${spec%%,*}
    [[ "$label" =~ ^[A-Za-z0-9._-]+$ ]] || fail "cell label \"$label\" must be a single word (it is a table column)"

    assignments=()
    if [[ "$spec" == *,* ]]; then
        IFS=',' read -r -a assignments <<<"${spec#*,}"
    fi

    out="$OUT/$index-$label.out"
    printf '%2d/%d  %-13s %s\n' "$index" "${#CELLS[@]}" "$label" \
        "${assignments[*]:-config as loaded}"
    run_cell "$out" ${assignments[@]+"${assignments[@]}"} || fail "cell $label failed" "$out"

    # Pull the two shapes the eval report prints: the overall summary line and
    # the per-kind lines. Everything else — the per-case table, the missed
    # memories — stays on disk for the reader who asks "which case?".
    cell_facts="$OUT/$index-facts"
    awk -v idx="$index" -v label="$label" '
        function number(value) { gsub(/[%:]/, "", value); return value + 0 }
        # "recall@5:  83.3%   precision@1: 50.0%   MRR: 63.9%   complete@5: …"
        /^recall@[0-9]+:/ && $3 == "precision@1:" {
            k = $1; sub(/^recall@/, "", k); sub(/:$/, "", k)
            printf "L\t%d\t%s\n", idx, label
            printf "S\t%d\tk\t%s\n", idx, k
            printf "S\t%d\trecall\t%.1f\n", idx, number($2)
            printf "S\t%d\tp1\t%.1f\n", idx, number($4)
            printf "S\t%d\tmrr\t%.1f\n", idx, number($6)
            printf "S\t%d\tcomplete\t%.1f\n", idx, number($8)
            printf "S\t%d\tp50\t%.1f\n", idx, number($11)
            printf "S\t%d\tp95\t%.1f\n", idx, number($14)
            next
        }
        # "abstention (negatives returning nothing at the floor): 100.0%"
        $1 == "abstention" { printf "S\t%d\tabstain\t%.1f\n", idx, number($NF); next }
        # "  paraphrase: recall 83.3%   p@1 50.0%   complete@5 80.8%"
        $2 == "recall" && $4 == "p@1" && $6 ~ /^complete@[0-9]+$/ {
            kind = $1; sub(/:$/, "", kind)
            printf "K\t%d\t%s\t%.1f\t%.1f\t%.1f\n", idx, kind, number($3), number($5), number($7)
        }
    ' "$out" >"$cell_facts"
    grep -q "^S	$index	recall	" "$cell_facts" || fail "cell $label printed no summary line" "$out"
    cat "$cell_facts" >>"$FACTS"
done

echo
# The table. Δ columns read against row 1, which is the reference because a
# caller who wants a different one reorders --cell.
awk -F '\t' '
    $1 == "L" { cells = $2; label[$2] = $3; next }
    $1 == "S" { value[$2, $3] = $4; next }
    $1 == "K" {
        if (!seen[$3]++) { kinds++; kind[kinds] = $3 }
        value[$2, $3 "!recall"] = $4
        value[$2, $3 "!p1"] = $5
        value[$2, $3 "!complete"] = $6
        next
    }
    END {
        k = value[1, "k"]
        if (!(k + 0)) { print "recall-grid: no rows to render" > "/dev/stderr"; exit 1 }

        printf "%-13s %7s %6s %8s %6s │ %9s %5s │ %8s %6s %6s %5s\n", \
            "cell", "recall", "p@1", "complete", "MRR", "p50/p95", "abst", \
            "Δrecall", "Δp@1", "Δcomp", "ΔMRR"
        rule = ""
        for (i = 0; i < 92; i++) rule = rule "─"
        printf "%s\n", rule

        for (i = 1; i <= cells; i++) {
            abstain = (i, "abstain") in value ? value[i, "abstain"] : 0
            if (i == 1) {
                deltas = "  reference"
            } else {
                deltas = sprintf("%+8.1f %+6.1f %+6.1f %+5.1f", \
                    value[i, "recall"] - value[1, "recall"], \
                    value[i, "p1"] - value[1, "p1"], \
                    value[i, "complete"] - value[1, "complete"], \
                    value[i, "mrr"] - value[1, "mrr"])
            }
            printf "%-13s %7.1f %6.1f %8.1f %6.1f │ %4.1f/%4.1f %5.1f │ %s\n", \
                label[i], value[i, "recall"], value[i, "p1"], value[i, "complete"], \
                value[i, "mrr"], value[i, "p50"], value[i, "p95"], abstain, deltas
        }

        # Per-kind deltas. The reference column carries its own absolute values —
        # the rest of the block is meaningless without them, and printing a
        # second table just to show one row was not worth the screen.
        title["recall"] = "recall@" k
        title["p1"] = "p@1"
        title["complete"] = "complete@" k

        width = 9
        for (i = 1; i <= cells; i++) if (length(label[i]) + 1 > width) width = length(label[i]) + 1
        kind_width = 16
        for (j = 1; j <= kinds; j++) if (length(kind[j]) + 1 > kind_width) kind_width = length(kind[j]) + 1
        left = "%-" kind_width "s"

        for (m = 1; m <= 3; m++) {
            metric = (m == 1 ? "recall" : (m == 2 ? "p1" : "complete"))
            printf "\n%s by kind (Δ vs %s; the reference column is its own values)\n", \
                title[metric], label[1]
            printf left, "kind"
            for (i = 1; i <= cells; i++) printf "%*s", width, label[i]
            printf "\n"
            for (j = 1; j <= kinds; j++) {
                printf left, kind[j]
                for (i = 1; i <= cells; i++) {
                    here = value[i, kind[j] "!" metric]
                    if (i == 1) printf "%*.1f", width, here
                    else printf "%+*.1f", width, here - value[1, kind[j] "!" metric]
                }
                printf "\n"
            }
        }
    }
' "$FACTS"

if [[ $KEEP -eq 1 ]]; then
    echo
    echo "per-cell output kept in $OUT"
fi
