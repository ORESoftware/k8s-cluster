#!/usr/bin/env bash
# Bring up a 3-node BEAM cluster on localhost. Each node binds to a
# different HTTP port but all share the same Erlang cookie and form a
# fully-connected distributed cluster via EPMD.
#
# Usage:
#   ./scripts/cluster-local.sh up      # start node-0, node-1, node-2
#   ./scripts/cluster-local.sh down    # kill everything we started
#   ./scripts/cluster-local.sh tail    # tail all three logs at once
#
# The script writes pid files + logs into ./.cluster-local/ so re-runs are
# idempotent (down won't kill someone else's beam.smp processes).
#
# After bringing the cluster up:
#   python3 scripts/demo.py --bases http://localhost:8181 http://localhost:8182 http://localhost:8183

set -euo pipefail

cd "$(dirname "$0")/.."

DATA_DIR=".cluster-local"
COOKIE="cluster_local_cookie"
NODES=(0 1 2)
PORTS=(8181 8182 8183)

mkdir -p "$DATA_DIR/logs" "$DATA_DIR/pids"

build_release() {
  echo "==> building gleam release"
  gleam build >/dev/null
}

erl_dist_args() {
  local idx="$1"
  local epmd_port=4369
  local dist_port=$((9100 + idx))
  echo "-name presence$idx@127.0.0.1 -setcookie $COOKIE -kernel inet_dist_listen_min $dist_port -kernel inet_dist_listen_max $dist_port"
}

start_node() {
  local idx="$1"
  local port="${PORTS[$idx]}"
  local pid_file="$DATA_DIR/pids/presence$idx.pid"
  local log_file="$DATA_DIR/logs/presence$idx.log"

  if [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
    echo "==> node $idx already running (pid $(cat "$pid_file"))"
    return
  fi

  local dist_port=$((9100 + idx))
  local node_name="presence$idx@127.0.0.1"

  echo "==> starting node $idx on http :$port  (erlang $node_name, dist :$dist_port)"

  # Build the static-peer list = every node *other than* this one.
  local peers=""
  for j in "${NODES[@]}"; do
    if [[ "$j" != "$idx" ]]; then
      peers="$peers${peers:+,}presence$j@127.0.0.1"
    fi
  done

  PORT="$port" \
  ERL_AFLAGS="-name $node_name -setcookie $COOKIE -kernel inet_dist_listen_min $dist_port -kernel inet_dist_listen_max $dist_port" \
  CLUSTER_PEERS="$peers" \
  CLUSTER_DISCOVERY_INTERVAL_MS="1000" \
  KUBERNETES_SERVICE_HOST="" \
    nohup gleam run >"$log_file" 2>&1 &

  echo $! > "$pid_file"
  sleep 0.5
}

wait_for_cluster() {
  # The in-process cluster module reads CLUSTER_PEERS and tries to connect
  # every second. Give it ~3 seconds to converge before reporting status.
  echo "==> waiting for cluster mesh to form via in-process CLUSTER_PEERS loop"
  sleep 3.5
}

cluster_status() {
  for idx in "${NODES[@]}"; do
    local port="${PORTS[$idx]}"
    echo
    echo "── node $idx ───────────────────────────"
    curl -s "http://localhost:$port/healthz" | sed 's/^/  /'
    echo
    curl -s "http://localhost:$port/nodes" | sed 's/^/  /'
  done
}

cmd_up() {
  build_release
  for idx in "${NODES[@]}"; do start_node "$idx"; done
  sleep 1.5
  wait_for_cluster
  cluster_status
  echo
  echo "==> cluster ready. logs in $DATA_DIR/logs/, pids in $DATA_DIR/pids/"
  echo "    test:  python3 scripts/demo.py --bases http://localhost:8181 http://localhost:8182 http://localhost:8183"
}

cmd_down() {
  for idx in "${NODES[@]}"; do
    local pid_file="$DATA_DIR/pids/presence$idx.pid"
    if [[ -f "$pid_file" ]]; then
      local pid="$(cat "$pid_file")"
      if kill -0 "$pid" 2>/dev/null; then
        echo "==> killing node $idx (pid $pid)"
        kill "$pid" 2>/dev/null || true
        # gleam run spawns a beam.smp child; kill children too
        pkill -P "$pid" 2>/dev/null || true
      fi
      rm -f "$pid_file"
    fi
  done
  # Belt and braces: anything still bound to our ports?
  for port in "${PORTS[@]}"; do
    local pid
    pid="$(lsof -nP -iTCP:$port -sTCP:LISTEN 2>/dev/null | awk 'NR>1 {print $2; exit}' || true)"
    if [[ -n "$pid" ]]; then
      echo "==> port $port still bound by pid $pid; killing"
      kill "$pid" 2>/dev/null || true
    fi
  done
  echo "==> all nodes stopped"
}

cmd_tail() {
  exec tail -f "$DATA_DIR"/logs/presence*.log
}

case "${1:-up}" in
  up) cmd_up ;;
  down) cmd_down ;;
  tail) cmd_tail ;;
  status) cluster_status ;;
  *)
    echo "usage: $0 {up|down|tail|status}" >&2
    exit 2
    ;;
esac
