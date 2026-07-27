#!/usr/bin/env bash
# 24-hour soak: sustained load with consolidation firing, watching RSS.
#
#   scripts/soak.sh [HOURS]
#
# Answers one question the short benchmark cannot: does anything leak.
# A memory service is a long-lived daemon holding an ONNX model, a SQLite
# connection, per-user tantivy indexes and a job queue — and the nightly
# consolidation job walks every user's whole corpus. If RSS climbs
# steadily across a day, that is where it will show.
#
# `[consolidation].schedule = "hourly"` rather than daily, so a 24-hour
# run exercises the job ~24 times instead of once. Consolidation only
# merges with a provider configured, but expiry and decay — the parts
# that walk and rewrite every memory — run regardless, and those are the
# ones that touch enough memory to leak.
#
# Results stream to soak-results/ as they are sampled, so a run that dies
# at hour 19 still leaves 19 hours of evidence.
set -euo pipefail

cd "$(dirname "$0")/.."

HOURS="${1:-24}"
PORT="${PORT:-7098}"
IMAGE="${IMAGE:-recuerdos-ai:soak}"
CONTAINER="recuerdos-ai-soak"
RESULTS_DIR="soak-results"
# Sampled every minute: frequent enough to see a sawtooth, sparse enough
# that a day of samples is still a readable file.
SAMPLE_INTERVAL="${SAMPLE_INTERVAL:-60}"
# Deliberately gentle. This is a duration test, not a throughput test —
# saturating the daemon would measure the same thing bench.sh already
# does, and would make a leak harder to see against the noise.
WRITES_PER_MINUTE="${WRITES_PER_MINUTE:-60}"

mkdir -p "$RESULTS_DIR"
STAMP=$(date +%Y%m%d-%H%M%S)
SAMPLES="$RESULTS_DIR/soak-$STAMP.csv"

cleanup() {
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    kill %1 2>/dev/null || true
}
trap cleanup EXIT

echo "==> building the release image"
docker build -q -f docker/Dockerfile -t "$IMAGE" . >/dev/null

docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" \
    -p "$PORT:7070" \
    -e RECUERDOS_AI_AUTH__MODE=none \
    -e RECUERDOS_AI_SERVER__HOST=0.0.0.0 \
    -e RECUERDOS_AI_CONSOLIDATION__SCHEDULE=hourly \
    "$IMAGE" serve >/dev/null

echo "==> waiting for the daemon"
until curl -sf "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; do sleep 0.5; done

echo "elapsed_minutes,rss_bytes,memories,recall_p50_ms,http_errors" > "$SAMPLES"
echo "==> soaking for ${HOURS}h; samples -> $SAMPLES"

# --- background writer -----------------------------------------------
(
    i=0
    while true; do
        for _ in $(seq 1 "$WRITES_PER_MINUTE"); do
            curl -s -o /dev/null -X POST "http://127.0.0.1:$PORT/v1/memories:direct" \
                -H 'Content-Type: application/json' \
                -d "{\"content\":\"soak memory $i: the service uses component $((i % 40)) of the stack\"}" || true
            i=$((i + 1))
        done
        sleep 60
    done
) &

# --- sampler ---------------------------------------------------------
END=$(( $(date +%s) + HOURS * 3600 ))
ERRORS=0

while [ "$(date +%s)" -lt "$END" ]; do
    sleep "$SAMPLE_INTERVAL"

    ELAPSED_MIN=$(( (SAMPLE_INTERVAL + $(date +%s) - (END - HOURS * 3600)) / 60 ))

    # Bytes, not the human-readable form: this column gets plotted, and
    # "1.2GiB" does not sort against "987MiB".
    RSS=$(docker stats --no-stream --format '{{.MemUsage}}' "$CONTAINER" 2>/dev/null \
        | awk '{print $1}' \
        | awk '/GiB/{gsub("GiB","");print $1*1024*1024*1024; next}
               /MiB/{gsub("MiB","");print $1*1024*1024; next}
               /KiB/{gsub("KiB","");print $1*1024; next}
               {print 0}')

    START_NS=$(date +%s%N)
    BODY=$(curl -sf -X POST "http://127.0.0.1:$PORT/v1/memories/search" \
        -H 'Content-Type: application/json' \
        -d '{"query":"what does the service use for the stack","limit":8}' 2>/dev/null) || ERRORS=$((ERRORS + 1))
    RECALL_MS=$(( ($(date +%s%N) - START_NS) / 1000000 ))

    COUNT=$(curl -sf "http://127.0.0.1:$PORT/v1/memories/export?format=json" 2>/dev/null \
        | grep -o '"id"' | wc -l | tr -d ' ')

    echo "$ELAPSED_MIN,${RSS:-0},${COUNT:-0},$RECALL_MS,$ERRORS" >> "$SAMPLES"
done

echo
echo "==> done. $SAMPLES"
echo "==> RSS first and last samples:"
head -2 "$SAMPLES" | tail -1
tail -1 "$SAMPLES"
echo
echo "A flat RSS column is the pass condition. A steady climb across the"
echo "run — rather than a rise that plateaus as caches fill — is a leak."
