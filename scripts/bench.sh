#!/usr/bin/env bash
# Runs the performance benchmark against a throwaway release-image daemon.
#
#   scripts/bench.sh [SEED_COUNT] [QUERY_COUNT]
#
# Uses `docker/Dockerfile` — the artifact users actually get — on its own
# anonymous volume, so it never touches a store you care about and always
# starts from empty. Results land in bench-results/.
#
# Measures cold start and idle RSS here rather than in bench.py, because
# both are properties of the container rather than of the API.
set -euo pipefail

cd "$(dirname "$0")/.."

SEED_COUNT="${1:-100000}"
QUERY_COUNT="${2:-500}"
CONCURRENCY="${CONCURRENCY:-32}"
PORT="${PORT:-7099}"
IMAGE="${IMAGE:-recuerdos-ai:bench}"
CONTAINER="recuerdos-ai-bench"
NETWORK="recuerdos-ai-bench-net"
RESULTS_DIR="bench-results"

mkdir -p "$RESULTS_DIR"
STAMP=$(date +%Y%m%d-%H%M%S)
JSON="$RESULTS_DIR/bench-$SEED_COUNT-$STAMP.json"

cleanup() {
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    docker network rm "$NETWORK" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "==> building the release image"
docker build -q -f docker/Dockerfile -t "$IMAGE" . >/dev/null

echo "==> environment"
docker version --format '    docker {{.Server.Version}}' || true
echo "    host: $(uname -sm)"
echo "    cpus: $(docker info --format '{{.NCPU}}' 2>/dev/null || echo '?') available to docker"

cleanup
docker network create "$NETWORK" >/dev/null

# --- cold start ------------------------------------------------------
#
# From `docker run` returning to /healthz answering. Includes loading the
# ONNX model, which is the part that could plausibly be slow — the target
# exists because a daemon that takes 10s to become ready is one an editor
# plugin cannot spawn on demand.
echo "==> cold start"
START_NS=$(date +%s%N)
docker run -d --name "$CONTAINER" \
    --network "$NETWORK" \
    -p "$PORT:7070" \
    -e RECUERDOS_AI_AUTH__MODE=none \
    -e RECUERDOS_AI_SERVER__HOST=0.0.0.0 \
    "$IMAGE" serve >/dev/null

until curl -sf "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; do
    sleep 0.05
    if ! docker ps -q -f name="$CONTAINER" | grep -q .; then
        echo "the daemon exited during startup:" >&2
        docker logs "$CONTAINER" 2>&1 | tail -20 >&2
        exit 1
    fi
done
COLD_START_MS=$(( ($(date +%s%N) - START_NS) / 1000000 ))
echo "    ${COLD_START_MS}ms to first healthy response"

# --- idle RSS --------------------------------------------------------
#
# Sampled before any load, with the model loaded. `docker stats` reports
# the cgroup's working set, which includes page cache the process could
# give back — so this is an upper bound, not a malloc figure.
sleep 3
IDLE_RSS=$(docker stats --no-stream --format '{{.MemUsage}}' "$CONTAINER" | awk '{print $1}')
echo "==> idle RSS: $IDLE_RSS"

# --- load ------------------------------------------------------------
echo "==> load"
# On the daemon's own network, addressed by container name. Going through
# the published host port instead puts Docker Desktop's userspace
# port-forwarder in the path, which drops connections under sustained
# concurrency — a 100k seed died at ~19k that way — and would tax every
# latency sample with a proxy hop that no real deployment has.
docker run --rm --network "$NETWORK" \
    -v "$PWD/scripts:/scripts:ro" \
    -v "$PWD/$RESULTS_DIR:/results" \
    python:3.12-slim \
    sh -c "pip install -q httpx && python /scripts/bench.py \
        --url http://$CONTAINER:7070 \
        --seed $SEED_COUNT \
        --queries $QUERY_COUNT \
        --concurrency $CONCURRENCY \
        --json /results/$(basename "$JSON")"

# --- RSS after load --------------------------------------------------
LOADED_RSS=$(docker stats --no-stream --format '{{.MemUsage}}' "$CONTAINER" | awk '{print $1}')
echo "==> RSS after load: $LOADED_RSS"

# Fold the container-level measurements into the same file, so one
# artifact describes the whole run.
python3 - "$JSON" "$COLD_START_MS" "$IDLE_RSS" "$LOADED_RSS" <<'PY'
import json, sys
path, cold, idle, loaded = sys.argv[1:5]
with open(path) as handle:
    report = json.load(handle)
report["cold_start_ms"] = int(cold)
report["idle_rss"] = idle
report["rss_after_load"] = loaded
with open(path, "w") as handle:
    json.dump(report, handle, indent=2)
PY

echo
echo "==> $JSON"
