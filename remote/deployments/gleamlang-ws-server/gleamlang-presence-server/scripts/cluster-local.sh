#!/usr/bin/env bash
# Bring up a 3-node BEAM cluster on localhost, plus (optionally) sidecar
# Docker containers for Postgres and NATS so all four messaging layers
# can be exercised end-to-end:
#
#   1. ETS registry (per-node)        — local typed sends
#   2. Erlang `pg` + fanout relay     — within-cluster cross-pod fanout
#   3. PG LISTEN/NOTIFY (sharded)     — DB-driven membership changes
#   4. NATS                           — cross-cluster / external pub/sub
#
# Usage:
#   ./scripts/cluster-local.sh up                    # 3 BEAM nodes, in-memory store, no NATS
#   ./scripts/cluster-local.sh up --with-pg          # + PG container at :5439
#   ./scripts/cluster-local.sh up --with-nats        # + NATS container at :4222
#   ./scripts/cluster-local.sh up --with-pg --with-nats   # everything
#   ./scripts/cluster-local.sh down                  # kill nodes + sidecars
#   ./scripts/cluster-local.sh tail                  # tail all three logs at once
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

PG_CONTAINER="presence-local-pg"
PG_HOST_PORT=5439
PG_USER="postgres"
PG_PASS="postgres"
PG_DB="presence"

NATS_CONTAINER="presence-local-nats"
NATS_HOST_PORT=4222

# The central CDC gateway streams from the shared pg-defs Postgres to the
# JetStream `CDC` stream that every Rust service subscribes to (see
# `remote/deployments/wal-gateway-rs` and `remote/libs/wal-consumer-rs`).
GATEWAY_PORT=8104

ENABLE_PG=0
ENABLE_NATS=0
ENABLE_GATEWAY=0

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

  local extra_env=""
  if [[ "$ENABLE_PG" == "1" ]]; then
    extra_env="$extra_env PG_DATABASE_URL=postgres://$PG_USER:$PG_PASS@127.0.0.1:$PG_HOST_PORT/$PG_DB"
  fi
  if [[ "$ENABLE_NATS" == "1" ]]; then
    extra_env="$extra_env NATS_URL=nats://127.0.0.1:$NATS_HOST_PORT"
  fi

  env $extra_env \
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

start_pg() {
  if docker ps --format '{{.Names}}' | grep -q "^$PG_CONTAINER\$"; then
    echo "==> postgres container $PG_CONTAINER already running"
    return
  fi
  echo "==> starting postgres container $PG_CONTAINER on :$PG_HOST_PORT"
  # We need wal_level=logical AND the wal2json extension installed so
  # pg_wal.gleam can attach a logical-replication slot with JSON output.
  # The official `postgres:16` image is debian-based; the wal2json
  # package lives in apt as `postgresql-16-wal2json`. We install it
  # before the server is started by injecting into the image's standard
  # `/docker-entrypoint-initdb.d/` initialization hook isn't reliable
  # (it runs as the postgres user, not root). Simpler: pull the image,
  # spawn a one-shot apt-get container to install the package, then start
  # the real server with that volume.
  #
  # In practice the easiest reliable path is: start postgres, then run
  # `apt-get install` inside the running container as root, then RELOAD
  # — wal2json is a shared library extension so it doesn't require a
  # restart to become available.
  docker run -d --rm --name "$PG_CONTAINER" \
    -e POSTGRES_PASSWORD="$PG_PASS" -e POSTGRES_DB="$PG_DB" \
    -p "$PG_HOST_PORT":5432 \
    postgres:16 \
    postgres -c wal_level=logical -c max_replication_slots=10 \
             -c max_wal_senders=10 -c max_slot_wal_keep_size=100MB \
    >/dev/null
  # Wait for readiness.
  for _ in {1..30}; do
    if docker exec -e PGPASSWORD="$PG_PASS" "$PG_CONTAINER" \
         pg_isready -U "$PG_USER" -d "$PG_DB" >/dev/null 2>&1; then
      break
    fi
    sleep 0.5
  done
  echo "==> installing wal2json"
  docker exec -u root "$PG_CONTAINER" sh -c \
    "apt-get -qq update && apt-get -qq install -y postgresql-16-wal2json" \
    >"$DATA_DIR/logs/pg-wal2json-install.log" 2>&1 || \
    echo "    (apt install failed; pg_wal will run disabled — see logs)"
  echo "==> applying schema"
  docker cp ../../../libs/pg-defs/schema/schema.sql "$PG_CONTAINER":/schema.sql >/dev/null
  docker exec -e PGPASSWORD="$PG_PASS" "$PG_CONTAINER" \
    psql -h 127.0.0.1 -U "$PG_USER" -d "$PG_DB" -f /schema.sql \
    >"$DATA_DIR/logs/pg-schema-apply.log" 2>&1 || true
}

start_nats() {
  if docker ps --format '{{.Names}}' | grep -q "^$NATS_CONTAINER\$"; then
    echo "==> nats container $NATS_CONTAINER already running"
    return
  fi
  echo "==> starting nats container $NATS_CONTAINER on :$NATS_HOST_PORT (jetstream enabled)"
  # `-js` turns on JetStream, which the wal-gateway uses for the durable
  # `CDC` stream and every Rust consumer reads from via durable pull
  # consumers. Without `-js` the gateway will refuse to start.
  docker run -d --rm --name "$NATS_CONTAINER" \
    -p "$NATS_HOST_PORT":4222 nats:2-alpine -js >/dev/null
  sleep 1
}

start_gateway() {
  # Run the central WAL→JetStream gateway against the local PG and NATS
  # containers. Single process; the K8s deployment runs N replicas and
  # the gateway internally leader-elects via a pg_advisory_lock, but for
  # local dev one replica is plenty.
  local gateway_dir="../wal-gateway-rs"
  local pid_file="$DATA_DIR/pids/wal-gateway.pid"
  local log_file="$DATA_DIR/logs/wal-gateway.log"
  if [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
    echo "==> wal-gateway already running (pid $(cat "$pid_file"))"
    return
  fi
  if [[ ! -d "$gateway_dir" ]]; then
    echo "==> wal-gateway-rs not found at $gateway_dir; skipping"
    return
  fi
  echo "==> building wal-gateway-rs (cargo build --release)"
  (cd "$gateway_dir" && cargo build --release) \
    >"$DATA_DIR/logs/wal-gateway-build.log" 2>&1 || {
      echo "    (cargo build failed — see $DATA_DIR/logs/wal-gateway-build.log)"
      return
    }
  echo "==> starting wal-gateway on :$GATEWAY_PORT"
  env \
    WAL_GATEWAY_DATABASE_URL="postgres://$PG_USER:$PG_PASS@127.0.0.1:$PG_HOST_PORT/$PG_DB?sslmode=disable" \
    WAL_GATEWAY_NATS_URL="nats://127.0.0.1:$NATS_HOST_PORT" \
    PORT="$GATEWAY_PORT" \
    WAL_GATEWAY_POLL_MS=250 \
    nohup "$gateway_dir/target/release/dd-wal-gateway" \
      >"$log_file" 2>&1 &
  echo $! > "$pid_file"
  sleep 1
}

stop_gateway() {
  local pid_file="$DATA_DIR/pids/wal-gateway.pid"
  if [[ -f "$pid_file" ]]; then
    local pid="$(cat "$pid_file")"
    if kill -0 "$pid" 2>/dev/null; then
      echo "==> stopping wal-gateway (pid $pid)"
      kill "$pid" 2>/dev/null || true
    fi
    rm -f "$pid_file"
  fi
}

stop_sidecar() {
  local name="$1"
  if docker ps --format '{{.Names}}' | grep -q "^$name\$"; then
    echo "==> stopping container $name"
    docker stop "$name" >/dev/null 2>&1 || true
  fi
}

cmd_up() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --with-pg)      ENABLE_PG=1      ;;
      --with-nats)    ENABLE_NATS=1    ;;
      --with-gateway) ENABLE_GATEWAY=1 ; ENABLE_PG=1 ; ENABLE_NATS=1 ;;
      *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac
    shift
  done

  build_release
  [[ "$ENABLE_PG"      == "1" ]] && start_pg
  [[ "$ENABLE_NATS"    == "1" ]] && start_nats
  [[ "$ENABLE_GATEWAY" == "1" ]] && start_gateway

  for idx in "${NODES[@]}"; do start_node "$idx"; done
  sleep 1.5
  wait_for_cluster
  cluster_status
  echo
  echo "==> cluster ready. logs in $DATA_DIR/logs/, pids in $DATA_DIR/pids/"
  if [[ "$ENABLE_PG" == "1" ]]; then
    echo "    PG:   postgres://$PG_USER:$PG_PASS@127.0.0.1:$PG_HOST_PORT/$PG_DB"
  fi
  if [[ "$ENABLE_NATS" == "1" ]]; then
    echo "    NATS: nats://127.0.0.1:$NATS_HOST_PORT"
  fi
  if [[ "$ENABLE_GATEWAY" == "1" ]]; then
    echo "    CDC:  http://localhost:$GATEWAY_PORT     (subject prefix: cdc.>)"
  fi
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
  stop_gateway
  stop_sidecar "$PG_CONTAINER"
  stop_sidecar "$NATS_CONTAINER"
  echo "==> all nodes stopped"
}

cmd_tail() {
  exec tail -f "$DATA_DIR"/logs/presence*.log
}

case "${1:-up}" in
  up) shift; cmd_up "$@" ;;
  down) cmd_down ;;
  tail) cmd_tail ;;
  status) cluster_status ;;
  *)
    echo "usage: $0 {up [--with-pg] [--with-nats] [--with-gateway] | down | tail | status}" >&2
    echo "  --with-gateway implies --with-pg --with-nats and starts wal-gateway-rs" >&2
    exit 2
    ;;
esac
