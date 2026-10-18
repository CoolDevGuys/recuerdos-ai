#!/usr/bin/env bash
# Field measurements owed by recall-quality-plan Phases 0–3.
#
# Run this ON the host that runs recuerdos-ai, after installing v0.5.0-rc1:
#
#   curl -fsSL https://raw.githubusercontent.com/CoolDevGuys/recuerdos-ai/main/install.sh \
#     | RECUERDOS_AI_VERSION=v0.5.0-rc1 sh
#
# The RECUERDOS_AI_VERSION is not optional. `install.sh` with no version asks
# GitHub for /releases/latest, and GitHub excludes prereleases from that — a
# bare install still gives you v0.4.3, which has neither knob this script sets.
# It writes only into $OUT and into no database: every `recall audit` is
# read-only ("the daemon may stay running"), and nothing here stops, restarts
# or reconfigures the running daemon.
#
# What it collects, and which open DoD box each one closes:
#   M1  slot on vs off            → Phase 3's hand review (the last box of §10)
#   M2  δ 0 / 0.05 / 0.2 / 0.5    → Phase 2's owed falsifier for its shipped δ
#   M3  latency + RSS             → Phase 1's Task 1.1
#   M4  (printed, not run)        → Phase 1's Task 1.5, which needs the daemon
#                                   itself logging at debug. Left by hand on
#                                   purpose: a script that restarts your only
#                                   memory server is a bad trade for one number.
#
# Works whether the daemon runs on the host (install.sh) or in a container, and
# says which of the two it found.
set -uo pipefail

# ── EDIT THIS ONE ────────────────────────────────────────────────────────────
USER_HANDLE=alex                  # find it: recuerdos-ai user list
# ─────────────────────────────────────────────────────────────────────────────

# Everything else is discovered, and all of it can be overridden:
#   CONFIG=/path/to/recuerdos-ai.toml  BIN=/usr/local/bin/recuerdos-ai
#   IMAGE=ghcr.io/...  VOLUME=...  LIMIT=8  OUT=/tmp/field
OUT="${OUT:-$HOME/field-measure}"   # stable on purpose: the first run writes a query
                                    # template here for you to fill in, and a
                                    # timestamped directory would lose it on re-run
CONFIG="${CONFIG:-}"
BIN="${BIN:-}"
IMAGE="${IMAGE:-ghcr.io/cooldevguys/recuerdos-ai:0.5.0-rc1}"   # image tags carry no leading "v"
VOLUME="${VOLUME:-recuerdos-ai-data}"
LIMIT="${LIMIT:-5}"                 # the k the plan's gates are written on
WANT_VERSION="${WANT_VERSION:-0.5.0-rc1}"

mkdir -p "$OUT"

# ── which shape is this deployment? ─────────────────────────────────────────
# The knobs this script measures must never arrive from the environment: figment
# merges Env after Toml, so one of these set in a systemd unit would silently win
# over every variant built below, and the audit would compare a config with itself.
is_measured_knob() {
    case "$1" in
        RECUERDOS_AI_CONFIG|\
        RECUERDOS_AI_GRAPH__EXPANSION_SLOTS|\
        RECUERDOS_AI_RETRIEVAL__RELEVANCE_WEIGHT|\
        RECUERDOS_AI_RERANK__MODE) return 0 ;;
        *) return 1 ;;
    esac
}

CONTAINER=$(docker ps --format '{{.Names}}' 2>/dev/null | grep -x 'recuerdos-ai' | head -1 || true)
declare -a ENV_ARGS=()
DAEMON_PID=""
if [ -n "$CONTAINER" ]; then
    IS_CONTAINER=1
    RUN_MODE="container ($CONTAINER)"
    # One container per command: the data volume, plus $OUT read-only as /work so
    # every generated config and the query file live at a path inside it.
    ra() { docker run --rm -v "$VOLUME:/data" -v "$OUT:/work:ro" \
             -e RECUERDOS_AI_CONFIG=/work/"$1" "$IMAGE" "${@:2}"; }
    wp() { printf '/work/%s\n' "$1"; }          # how a file in $OUT looks in there
    rss() { docker stats --no-stream \
              --format '{{.Name}} rss={{.MemUsage}} cpu={{.CPUPerc}}'; }
    [ -n "$CONFIG" ] || CONFIG=$(docker exec "$CONTAINER" sh -c \
        'printf %s "${RECUERDOS_AI_CONFIG:-}"' 2>/dev/null || true)
    if [ -n "$CONFIG" ]; then                   # the host path behind that container path
        SRC=$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "'"$CONFIG"'"}}{{.Source}}{{end}}{{end}}' \
              "$CONTAINER" 2>/dev/null || true)
        [ -n "${SRC:-}" ] && CONFIG="$SRC"
    fi
    RUNNING_IMG=$(docker exec "$CONTAINER" --version 2>/dev/null | sed -n 's/^recuerdos-ai //p' || true)
else
    IS_CONTAINER=0
    RUN_MODE="host"
    BIN=${BIN:-$(command -v recuerdos-ai || true)}
    [ -n "$BIN" ] || { echo "!! no recuerdos-ai on PATH — re-run with BIN=/path/to/recuerdos-ai"; exit 1; }
    ra() { local cfg="$1"; shift
        env "RECUERDOS_AI_CONFIG=$OUT/$cfg" ${ENV_ARGS[@]+"${ENV_ARGS[@]}"} "$BIN" "$@"; }
    wp() { printf '%s\n' "$OUT/$1"; }
    DAEMON_PID=$(pgrep -f 'recuerdos-ai serve' | head -1 || true)
    rss() { if [ -n "$DAEMON_PID" ] && [ -r "/proc/$DAEMON_PID/status" ]; then
                printf 'daemon rss=%s (pid %s)\n' \
                    "$(awk '/^VmRSS:/{print $2 " " $3}' "/proc/$DAEMON_PID/status")" "$DAEMON_PID"
            else
                printf 'daemon rss=not readable (no serve process, or its /proc needs root)\n'
            fi; }
    # Take the daemon's own RECUERDOS_AI_* environment, so the audit resolves the same
    # [storage] path, provider and model cache that the serving process does.
    if [ -n "$DAEMON_PID" ]; then
        if [ -r "/proc/$DAEMON_PID/environ" ]; then
            while IFS= read -r -d '' kv; do
                case "${kv%%=*}" in
                    RECUERDOS_AI_*)
                        if is_measured_knob "${kv%%=*}"; then
                            echo "   note: not inheriting ${kv%%=*} — it would override the variants"
                        else
                            ENV_ARGS+=("$kv")
                        fi ;;
                esac
            done < "/proc/$DAEMON_PID/environ"
        else
            echo "   note: the serve process (pid $DAEMON_PID) runs as someone else, so its"
            echo "   environment is unreadable and this shell's is used instead. Check the"
            echo "   storage path printed in 00-config.txt really is your store."
        fi
    else
        echo "   note: no 'recuerdos-ai serve' running, so the environment below is yours"
        echo "   rather than the daemon's. Fine if this shell already has it."
    fi
fi

# The config file: an explicit $CONFIG wins, else the daemon's RECUERDOS_AI_CONFIG,
# else a --config in its argv. There is no default file name to fall back on —
# AppConfig::load reads a file only when one of those two names it.
if [ -z "$CONFIG" ] && [ -n "$DAEMON_PID" ] && [ -r "/proc/$DAEMON_PID/cmdline" ]; then
    CONFIG=$(tr '\0' '\n' < "/proc/$DAEMON_PID/cmdline" | awk '
        /^--config=/ { sub(/^--config=/, ""); print; exit }
        prev == "--config" { print; exit }
        { prev = $0 }')
fi
[ -n "$CONFIG" ] || { echo "!! could not find your recuerdos-ai.toml. Re-run with CONFIG=/path/to/recuerdos-ai.toml"; exit 1; }
[ -f "$CONFIG" ] || { echo "!! no config at $CONFIG (looked via $RUN_MODE). Set CONFIG and re-run."; exit 1; }
echo "── deployment: $RUN_MODE"
echo "   config   $CONFIG"
cp "$CONFIG" "$OUT/base.toml"

# ── the guard against the bug that bit v0.4.1 ────────────────────────────────
# A key appended under the wrong section is silently orphaned, and the audit then
# compares one config with itself and reports a difference that is nothing. So:
# build the variant, then ask the binary what it thinks the value is, and refuse
# to measure if it disagrees with the file we just wrote.
set_key() { # set_key <section> <key> <value> <dest-name>
  local sec=$1 key=$2 val=$3 dest=$4
  if grep -qE "^[[:space:]]*\[$sec\][[:space:]]*$" "$OUT/base.toml"; then
    # Rewrite the key where it already lives rather than appending a second one: a
    # config copied from the shipped example already carries both knobs, and a
    # duplicate key is something figment has to decide about, silently.
    awk -v sec="$sec" -v key="$key" -v val="$val" '
      function header(l) { return l ~ /^[[:space:]]*\[/ }
      {
        if (header($0)) {
          if (insec && !emitted) print key " = " val      # target section ended without it
          insec = ($0 ~ ("^[[:space:]]*\\[" sec "\][[:space:]]*$"))
          emitted = 0
          print; next
        }
        if (insec && $0 ~ ("^[[:space:]]*" key "[[:space:]]*=")) {
          if (!emitted) { print key " = " val; emitted = 1 }
          next                                            # drop what was there
        }
        print
      }
      END { if (insec && !emitted) print key " = " val }   # target section is last
    ' "$OUT/base.toml" > "$OUT/$dest"
  else
    { cat "$OUT/base.toml"; printf '\n[%s]\n%s = %s\n' "$sec" "$key" "$val"; } > "$OUT/$dest"
  fi
  local n
  n=$(awk -v sec="$sec" -v key="$key" '
    /^[[:space:]]*\[/ { h = $0; gsub(/[[:space:]]/, "", h); insec = (h == "[" sec "]") }
    insec && index($0, key) == 1 && substr($0, length(key) + 1) ~ /^[[:space:]]*=/ { c++ }
    END { print c + 0 }' "$OUT/$dest")
  [ "$n" -eq 1 ] || { echo "!! $dest: [$sec] ends up with $n assignments of '$key'. Edit $CONFIG by hand."; exit 1; }
  local got want
  want=$(printf '%s' "$val" | tr -d '"')
  got=$(ra "$dest" config 2>/dev/null | awk -v k="$key" '$1==k{print $2; exit}')
  if [ "$got" != "$want" ]; then
    echo "!! $dest: asked for $key=$want, the binary reads '${got:-<absent>}'."
    echo "   Absent usually means the section that owns the key is switched off (a graph"
    echo "   block is only reported when [graph].enabled is true), or the key landed under"
    echo "   the wrong section — which is the v0.4.1 bug this check exists to catch."
    exit 1
  fi
  echo "   ok  $dest → $key = $got"
}

# ── 00 · which binary, which store, whose memories ───────────────────────────
{ ra base.toml --version; ra base.toml config; } > "$OUT/00-config.txt" 2>&1
RUNNING=$(sed -n 's/^recuerdos-ai //p' "$OUT/00-config.txt" | head -1)
if [ "$RUNNING" != "$WANT_VERSION" ]; then
  echo "!! the binary that would do this measuring reports '$RUNNING', not $WANT_VERSION."
  echo "   Install the prerelease first (note RECUERDOS_AI_VERSION: /releases/latest never"
  echo "   sees a prerelease, which is why a bare install.sh still gives you v0.4.3):"
  echo "     curl -fsSL https://raw.githubusercontent.com/CoolDevGuys/recuerdos-ai/main/install.sh \\"
  echo "       | RECUERDOS_AI_VERSION=v$WANT_VERSION sh"
  exit 1
fi
STORE=$(awk '/^storage/{s=1;next} /^[a-z]/{s=0} s && $1=="path"{print $2}' "$OUT/00-config.txt" | head -1)
if ! ra base.toml user list 2>/dev/null | grep -qw -- "$USER_HANDLE"; then
  echo "!! user '$USER_HANDLE' is not in the store this config resolves ($STORE)."
  echo "   The audit would be measuring somebody else's database, or an empty one."
  echo "   Re-run with USER_HANDLE=… or CONFIG=… set correctly; this is what it saw:"
  ra base.toml user list 2>&1 | sed -n '1,8p'
  exit 1
fi
echo "   binary   $RUNNING   store $STORE   user $USER_HANDLE ✓"
# The store is read from disk, so an audit measures the new binary whatever the
# service is running. RSS and the queue question are not like that: they are
# properties of the live process, so say out loud when it is still the old one.
if [ "$IS_CONTAINER" = 0 ] && command -v curl >/dev/null 2>&1; then
    LISTEN=$(awk '/^server/{s=1;next} /^[a-z]/{s=0} s && $1=="listen"{print $2}' "$OUT/00-config.txt" | head -1)
    LIVE=""
    [ -n "$LISTEN" ] && LIVE=$(curl -fsS --max-time 3 "http://$LISTEN/version" 2>/dev/null \
        | sed -n 's/.*"version":"\([^"]*\)".*/\1/p' || true)
    if [ -z "$LIVE" ]; then
        echo "   daemon  nothing answering http://${LISTEN:-<no listen line>}/version — it is stopped,"
        echo "           or it is not reachable from this shell. M3's RSS will read as unavailable."
    elif [ "$LIVE" != "$WANT_VERSION" ]; then
        echo "   daemon  still serving $LIVE while the installed binary is $WANT_VERSION."
        echo "           Restart it (systemctl restart recuerdos-ai, or your usual command) before"
        echo "           M3's RSS and M4's queue mean anything; the audits themselves are valid either way."
    else
        echo "   daemon  $LIVE ✓"
    fi
fi
if [ "$IS_CONTAINER" = 1 ] && [ -n "${RUNNING_IMG:-}" ] && [ "$RUNNING_IMG" != "$WANT_VERSION" ]; then
  echo "   note: the *running container* is $RUNNING_IMG while these audits run $WANT_VERSION."
  echo "   That is a valid reading (new binary, live store) but it is not the same fact as"
  echo "   'the service answers this way' — recreate the container when you want that one."
fi
sed -n '/^retrieval/,/^server/p' "$OUT/00-config.txt" | head -22

# ── the query file: the measurement is only as good as these ─────────────────
QUERIES="$OUT/queries.toml"
if [ ! -s "$QUERIES" ]; then
  cat > "$QUERIES" <<'TPL'
# Questions you would really ask your own memory service, in the TOML form so each
# one can say what should come back. `expect` is a SUBSTRING of the memory that
# ought to be first — write it from the memory's wording, not the query's.
# 12–20 is enough to see a pattern; a handful will tell you almost nothing.
# Fill this in, then re-run this script. It refuses to measure an unfilled template.
[[query]]
text = "REPLACE ME: a relational question naming one thing you store — who owns X, what does X run on, what does X require"
expect = "a distinctive substring of the answer"
TPL
  echo "── wrote $QUERIES — fill it in with your own questions, then run this script again."
  exit 2
fi
grep -q "REPLACE ME" "$QUERIES" && { echo "── $QUERIES still holds the template placeholder. Fill it in, then re-run."; exit 2; }

echo "── M1 · Phase 3: does the reserved seat put anything a human wants in the window? ──"
set_key graph expansion_slots 0 noslot.toml
ra noslot.toml recall audit --compare-config "$(wp base.toml)" \
   --queries "$(wp queries.toml)" --user "$USER_HANDLE" --limit "$LIMIT" \
   2>&1 | tee "$OUT/m1-slot.txt"
ra noslot.toml recall audit --compare-config "$(wp base.toml)" \
   --queries "$(wp queries.toml)" --user "$USER_HANDLE" --limit "$LIMIT" \
   --json > "$OUT/m1-slot.json" 2>/dev/null
cat <<'NOTE'
   Read m1-slot.txt (in the output directory) by hand. `before` = the v0.4 ordering with
   no reserved seat; `after` = what ships.
   For each query where `after` shows a memory `before` did not: is it a sentence you
   would call related to the question? Also note the row that left the window — that is
   the price. The failure to look for is not a wrong memory but a vacuous one: a hop
   fired on a name the query shares with something unrelated ("service", "team", a
   surname), or a seat spent on the bridge sentence again.
NOTE

echo "── M2 · Phase 2: is δ = 0.05 too timid on a real store? ──────────────────"
set_key retrieval relevance_weight 0 delta0.toml
set_key retrieval relevance_weight 0.2 delta02.toml
set_key retrieval relevance_weight 0.5 delta05.toml
ra delta0.toml recall audit --compare-config "$(wp base.toml)" \
   --queries "$(wp queries.toml)" --user "$USER_HANDLE" --limit "$LIMIT" \
   2>&1 | tee "$OUT/m2-delta0-vs-005.txt"
ra base.toml recall audit --compare-config "$(wp delta02.toml)" \
   --queries "$(wp queries.toml)" --user "$USER_HANDLE" --limit "$LIMIT" \
   2>&1 | tee "$OUT/m2-005-vs-02.txt"
ra base.toml recall audit --compare-config "$(wp delta05.toml)" \
   --queries "$(wp queries.toml)" --user "$USER_HANDLE" --limit "$LIMIT" \
   2>&1 | tee "$OUT/m2-005-vs-05.txt"
echo "   The eval rewarded relevance partly by construction, so this is the half that can"
echo "   refute it: if 0.2/0.5 promote memories you would not have wanted first, the shipped"
echo "   small δ was right and stays. If they promote better ones, raising it is a release note."

echo "── M3 · Phase 1.1: what a recall costs, and what the stage holds ─────────"
echo "   RSS only means something once the daemon has loaded what it will load: if"
echo "   [rerank].mode is on, ask it a couple of real questions first, or the number is a"
echo "   process that has not paid for its model yet rather than one that does not need it."
rss | tee "$OUT/m3-rss.txt"
MODE=$(awk -F= '/^\[rerank\]/{s=1;next} /^\[/{s=0}
                   s && $1 ~ /^[[:space:]]*mode[[:space:]]*$/ {gsub(/["[:space:]]/,"",$2); print $2; exit}' "$OUT/base.toml")
MODE=${MODE:-off}
ra base.toml recall audit --compare-config "$(wp base.toml)" \
   --queries "$(wp queries.toml)" --user "$USER_HANDLE" --limit "$LIMIT" 2>&1 \
  | sed -n '/latency/Ip' | tee "$OUT/m3-latency-noise-floor.txt"
echo "   (base vs base is not a mistake: it is the noise floor for the latency numbers around it.)"
if [ "$MODE" != "off" ]; then
  set_key rerank mode '"off"' rerankoff.toml
  ra rerankoff.toml recall audit --compare-config "$(wp base.toml)" \
     --queries "$(wp queries.toml)" --user "$USER_HANDLE" --limit "$LIMIT" \
     2>&1 | tee "$OUT/m3-rerank-off-vs-on.txt"
  rss | tee -a "$OUT/m3-rss.txt"
else
  echo "   [rerank].mode is already '$MODE' — Task 1.1's before/after flip is not available here."
  echo "   Recorded instead: mode, the noise-floor pass, and RSS above. That is the honest reading."
fi

echo "── M4 · Phase 1.5: the queue question, by hand ───────────────────────────"
cat <<'RUNBOOK'
This one needs the daemon itself logging at debug, which means restarting it with
RUST_LOG=debug — the config file cannot set the log level. Do it when you can spare
a minute of downtime.

  # systemd:   sudo systemctl edit recuerdos-ai     →  [Service] Environment=RUST_LOG=debug
  #            sudo systemctl daemon-reload && sudo systemctl restart recuerdos-ai
  #            journalctl -u recuerdos-ai -f > /tmp/dbg.log &
  # by hand:   stop it, then  RUST_LOG=debug recuerdos-ai serve 2>&1 | tee /tmp/dbg.log

  # then fire three searches at once (KEY is any read-scope key):
  for i in 1 2 3; do curl -s -X POST localhost:7070/v1/memories/search \
      -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
      -d '{"query":"how should I structure my typescript imports?","limit":5}' -o /dev/null & done; wait
  grep -E "rerank_leg|rerank leg failed" /tmp/dbg.log

  # then set [rerank].max_concurrency = 2 in the config, restart, and fire three again.

What decides it: the caller that got a session should show queued_ms ≈ 0 and
inference_ms ≈ a full head; the two refused ones should show busy=true, queued_ms ≈
the whole budget and inference_ms = 0. With max_concurrency = 2 the third caller's
queued_ms should be roughly one head — that is the queue, not a refusal. If instead a
*refused* caller shows inference_ms above 0, Phase 1 is wrong about its own fix and
that is worth telling me. Restore the usual environment afterwards: RUST_LOG=debug is
chatty on a box serving real traffic.
RUNBOOK

echo "── Task 0.9 (optional): the copy that lets the eval be tuned on real data ─"
echo "   Take it in a quiet moment. I can use the copy plus your expect list for the"
echo "   measurement the eval corpus cannot make; M1–M3 do not need it."
[ -n "$STORE" ] && echo "     tar czf \"\$HOME/store-\$(date +%F).tar.gz\" -C \"$STORE\" ."

echo
TARBALL="$HOME/field-$(date +%Y%m%d-%H%M).tar.gz"
tar czf "$TARBALL" -C "$OUT" .
echo "done. Everything is in $OUT — hand me $(basename "$TARBALL") (or just its .txt files)"
echo "and I will write the numbers into plan §17 and tick only what they support."
