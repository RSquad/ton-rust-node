#!/bin/sh
#
# Sync test watcher
#
# Runs as a sidecar alongside the TON node. Polls Prometheus metrics
# every 60 seconds and decides whether the node has synced, failed,
# or timed out. Reports the result as a GitHub commit status.
#
# Metrics used (from /metrics on the node's metrics port):
#   ton_node_engine_sync_status          — sync state machine (6 = synced, 8 = db broken)
#   ton_node_engine_last_mc_block_seqno  — latest masterchain block applied
#   ton_node_engine_timediff_seconds     — seconds between now and last MC block
#   ton_node_engine_shards_timediff_seconds — seconds between now and MC block
#                                            last processed by shard client
#
# Sync stages (sync_status values, ordered by normal flow):
#   0 = not_set        Initial state, node just started
#   1 = boot           Downloading init block proof, key blocks
#   2 = load_states    Downloading and applying persistent states (long, no seqno progress)
#   3 = finish_boot    Boot complete, preparing to sync
#   4 = sync_archives  Syncing via archives (bulk download)
#   5 = sync_blocks    Syncing block-by-block from peers
#   6 = synced         Masterchain caught up, shard client within 16 MC blocks
#   7 = checking_db    DB integrity check in progress
#   8 = db_broken      DB corruption detected
#
# Behavior on terminal states:
#   synced   → set GitHub commit status "success", then sleep forever
#   db_broken → set GitHub commit status "failure", then sleep forever
#   timeout  → set GitHub commit status "failure", then sleep forever
#
# On failure the pod stays alive so engineers can inspect logs:
#   kubectl exec -it <pod> -c ton-node -n ton-synctest -- tail -100 /logs/output.log
#
# The next workflow run replaces the pod via `helm upgrade`.
#
# Required env: METRICS_PORT, SYNC_TIMEOUT, NETWORK, GITHUB_TOKEN, GITHUB_SHA, GITHUB_REPO

set -eu

apk add --no-cache curl jq >/dev/null 2>&1

# ---------------------------------------------------------------------------
# Graceful shutdown: if pod is killed before sync completes, report failure.
# ---------------------------------------------------------------------------

FINISHED=false

cleanup() {
  if [ "$FINISHED" = "false" ]; then
    elapsed=$(( $(date +%s) - ${START:-0} ))
    github_status failure "Cancelled after $(fmt $elapsed)"
    echo "[watcher] CANCELLED — pod terminated before sync completed"
  fi
  exit 0
}

trap cleanup TERM INT

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Set commit status on the GitHub commit that triggered this test.
github_status() {
  local state="$1" description="$2"
  curl -sf -X POST \
    "https://api.github.com/repos/${GITHUB_REPO}/statuses/${GITHUB_SHA}" \
    -H "Authorization: token ${GITHUB_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "$(jq -cn --arg s "$state" --arg d "$description" \
      --arg c "sync-test/${NETWORK}" \
      '{state:$s, description:$d, context:$c}')" >/dev/null 2>&1 || true
}

# Format seconds as "Xh Ym".
fmt() {
  local h=$(($1 / 3600)) m=$((($1 % 3600) / 60))
  if [ "$h" -gt 0 ]; then echo "${h}h${m}m"; else echo "${m}m"; fi
}

# Human-readable name for sync_status value.
stage_name() {
  case "$1" in
    0) echo "not_set"     ;; 1) echo "boot"          ;; 2) echo "load_states"  ;;
    3) echo "finish_boot" ;; 4) echo "sync_archives" ;; 5) echo "sync_blocks"  ;;
    6) echo "synced"      ;; 7) echo "checking_db"   ;; 8) echo "db_broken"    ;;
    *) echo "unknown($1)" ;;
  esac
}

# Extract a gauge value from Prometheus text output.
# Handles both "name value" and "name{labels} value" formats.
# Usage: metric "$prometheus_text" "metric_name"
metric() {
  echo "$1" | awk -v m="$2" '$1 ~ "^" m "($|\\{)" && $1 !~ /^#/ { print int($2); exit }'
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

METRICS="http://localhost:${METRICS_PORT}/metrics"
HEALTHZ="http://localhost:${METRICS_PORT}/healthz"
POLL_INTERVAL=60

# Wait for the node's HTTP server to come up.
while ! curl -sf "$HEALTHZ" >/dev/null 2>&1; do sleep 5; done

START=$(date +%s)

while true; do
  # Fetch all metrics in one request.
  prom=$(curl -sf "$METRICS" 2>/dev/null) || { sleep "$POLL_INTERVAL"; continue; }

  status=$(metric "$prom" "ton_node_engine_sync_status")
  seqno=$(metric "$prom" "ton_node_engine_last_mc_block_seqno")
  timediff=$(metric "$prom" "ton_node_engine_timediff_seconds")
  shards_td=$(metric "$prom" "ton_node_engine_shards_timediff_seconds")
  status=${status:-0}
  seqno=${seqno:-0}
  timediff=${timediff:--}
  shards_td=${shards_td:--}
  elapsed=$(( $(date +%s) - START ))

  echo "[watcher] stage=$(stage_name "$status") seqno=$seqno mc_timediff=${timediff}s shards_timediff=${shards_td}s elapsed=$(fmt $elapsed)"

  # --- Terminal states ---

  # sync_status=6: node considers itself synced (MC timediff < 600s,
  # shard client within 16 MC blocks of masterchain).
  if [ "$status" = "6" ]; then
    FINISHED=true
    github_status success "Synced in $(fmt $elapsed) (seqno $seqno, mc_timediff ${timediff}s)"
    echo "[watcher] SUCCESS — node synced"
    exec tail -f /dev/null
  fi

  # sync_status=8: database corruption detected.
  if [ "$status" = "8" ]; then
    FINISHED=true
    github_status failure "DB broken (seqno $seqno, $(fmt $elapsed))"
    echo "[watcher] FAILURE — DB broken, pod stays alive for debugging"
    exec tail -f /dev/null
  fi

  # Timeout: node did not sync within the allowed window.
  if [ "$elapsed" -gt "$SYNC_TIMEOUT" ]; then
    FINISHED=true
    github_status failure "Timeout after $(fmt $elapsed): $(stage_name "$status"), mc_timediff=${timediff}s, shards=${shards_td}s"
    echo "[watcher] FAILURE — timeout after $(fmt $elapsed), pod stays alive for debugging"
    exec tail -f /dev/null
  fi

  sleep "$POLL_INTERVAL" &
  wait $!
done
