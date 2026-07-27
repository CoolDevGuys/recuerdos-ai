#!/usr/bin/env bash
# Seeds a user's memories and measures recall latency percentiles.
#
# Usage: scripts/bench-recall.sh [SEED_COUNT] [QUERY_COUNT]
#
# Measures the full request — auth, embedding, both index queries, fusion
# — because that is what a client actually waits for. The server's own
# `took_ms` (which excludes auth) is reported alongside, so the two halves
# can be told apart.
#
# Assumes a daemon on localhost:7070 and the `recuerdos-ai` CLI reachable
# via `docker compose run --rm dev`.
set -euo pipefail

SEED_COUNT="${1:-2000}"
QUERY_COUNT="${2:-200}"
BASE_URL="${BASE_URL:-http://localhost:7070}"
USER_HANDLE="bench-$(date +%s)"

echo "==> waiting for $BASE_URL"
for _ in $(seq 1 60); do
    curl -sf "$BASE_URL/healthz" >/dev/null 2>&1 && break
    sleep 2
done

echo "==> creating user $USER_HANDLE"
docker compose run --rm dev cargo run -q --bin recuerdos-ai -- \
    user add "$USER_HANDLE" >/dev/null 2>&1
KEY=$(docker compose run --rm dev cargo run -q --bin recuerdos-ai -- \
    key issue --user "$USER_HANDLE" --scopes read,write 2>/dev/null \
    | grep -o 'ra_live_[a-f0-9]*')

if [ -z "$KEY" ]; then
    echo "failed to issue an API key" >&2
    exit 1
fi

# Vocabulary that produces realistic overlap between memories, so the
# keyword leg has something to disagree with the vector leg about.
TOPICS=(pnpm typescript hetzner sqlite tantivy rust axum docker argon2 tokio)

echo "==> seeding $SEED_COUNT memories"
SEED_START=$(date +%s%N)
for i in $(seq 1 "$SEED_COUNT"); do
    topic="${TOPICS[$((i % ${#TOPICS[@]}))]}"
    curl -s -o /dev/null -X POST "$BASE_URL/v1/memories:direct" \
        -H "Authorization: Bearer $KEY" \
        -H 'Content-Type: application/json' \
        -d "{\"content\":\"memory $i: the project uses $topic for part $((i % 50)) of the stack\"}"
    if [ $((i % 500)) -eq 0 ]; then echo "    $i/$SEED_COUNT"; fi
done
SEED_END=$(date +%s%N)
echo "    ingest: $(( (SEED_END - SEED_START) / SEED_COUNT / 1000000 ))ms per memory"

echo "==> running $QUERY_COUNT queries"
LATENCIES=$(mktemp)
SERVER_TIMES=$(mktemp)
trap 'rm -f "$LATENCIES" "$SERVER_TIMES"' EXIT

for i in $(seq 1 "$QUERY_COUNT"); do
    topic="${TOPICS[$((i % ${#TOPICS[@]}))]}"
    START=$(date +%s%N)
    BODY=$(curl -s -X POST "$BASE_URL/v1/memories/search" \
        -H "Authorization: Bearer $KEY" \
        -H 'Content-Type: application/json' \
        -d "{\"query\":\"what do we use $topic for in the stack\",\"limit\":8}")
    END=$(date +%s%N)
    echo $(( (END - START) / 1000000 )) >> "$LATENCIES"
    echo "$BODY" | grep -o '"took_ms":[0-9]*' | cut -d: -f2 >> "$SERVER_TIMES"
done

percentile() {
    local file=$1 p=$2 count line
    count=$(wc -l < "$file")
    line=$(( (count * p + 99) / 100 ))
    [ "$line" -lt 1 ] && line=1
    sort -n "$file" | sed -n "${line}p"
}

echo
echo "==> results ($SEED_COUNT memories, $QUERY_COUNT queries)"
printf '    end-to-end   p50 %sms  p95 %sms  p99 %sms\n' \
    "$(percentile "$LATENCIES" 50)" \
    "$(percentile "$LATENCIES" 95)" \
    "$(percentile "$LATENCIES" 99)"
printf '    server-side  p50 %sms  p95 %sms  p99 %sms  (excludes auth)\n' \
    "$(percentile "$SERVER_TIMES" 50)" \
    "$(percentile "$SERVER_TIMES" 95)" \
    "$(percentile "$SERVER_TIMES" 99)"
