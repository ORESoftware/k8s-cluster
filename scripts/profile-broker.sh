#!/usr/bin/env bash
set -euo pipefail

mode="${1:-sample}"
profile="${PROFILE:-profiling}"
out_dir="${OUT_DIR:-target/profiles}"
profile_target="${PROFILE_TARGET:-broker}"
broker_host="${BROKER_HOST:-127.0.0.1}"
broker_http_port="${BROKER_HTTP_PORT:-6971}"
redis_host="${REDIS_HOST:-$broker_host}"
redis_port="${REDIS_PORT:-7379}"
redis_data_root="${REDIS_DATA_ROOT:-$out_dir/redis-profile}"
redis_server_bin="${REDIS_SERVER:-redis-server}"
raft_http_base_port="${RAFT_HTTP_BASE_PORT:-6972}"
raft_rpc_base_port="${RAFT_RPC_BASE_PORT:-7980}"
raft_data_root="${RAFT_DATA_ROOT:-$out_dir/raft-profile-cluster}"
raft_sync_log="${RAFT_SYNC_LOG:-true}"
raft_sync_commit="${RAFT_SYNC_COMMIT:-true}"
raft_target_quorum_extra_fanout="${RAFT_TARGET_QUORUM_EXTRA_FANOUT:-0}"
raft_bench_route="${RAFT_BENCH_ROUTE:-leader}"
bench_workers="${BENCH_WORKERS:-8}"
bench_keys="${BENCH_KEYS:-256}"
bench_duration_ms="${BENCH_DURATION_MS:-10000}"
bench_ttl_ms="${BENCH_TTL_MS:-5000}"
bench_io_timeout_ms="${BENCH_IO_TIMEOUT_MS:-5000}"
bench_http_keepalive="${BENCH_HTTP_KEEPALIVE:-true}"
capture_metrics="${CAPTURE_METRICS:-true}"
metrics_timeout_s="${METRICS_TIMEOUT_SECONDS:-2}"
sample_seconds="${SAMPLE_SECONDS:-8}"
sample_interval_ms="${SAMPLE_INTERVAL_MS:-1}"
perf_freq="${PERF_FREQ:-997}"
bench_raft_metrics="${BENCH_RAFT_METRICS:-}"
bench_raft_metrics_endpoints="${BENCH_RAFT_METRICS_ENDPOINTS:-}"
bench_http_auth_token="${BENCH_HTTP_AUTH_TOKEN:-}"
if [ -z "$bench_http_auth_token" ]; then
  bench_http_auth_token="${BENCH_RAFT_AUTH_TOKEN:-}"
fi
if [ -z "$bench_http_auth_token" ]; then
  bench_http_auth_token="${LMX_LIVE_RAFT_AUTH_TOKEN:-}"
fi
if [ -z "$bench_http_auth_token" ]; then
  bench_http_auth_token="${ALL_DOGS:-}"
fi
if [ -z "$bench_http_auth_token" ]; then
  bench_http_auth_token="${LMX_AUTH_TOKEN:-}"
fi

mkdir -p "$out_dir"

export RUSTFLAGS="${RUSTFLAGS:-} -C force-frame-pointers=yes"

server_pids=()
profile_pid=""
bench_pid=""
bench_status=0

cleanup() {
  if [ -n "$bench_pid" ]; then
    kill "$bench_pid" >/dev/null 2>&1 || true
    wait "$bench_pid" >/dev/null 2>&1 || true
  fi
  for pid in "${server_pids[@]}"; do
    kill "$pid" >/dev/null 2>&1 || true
  done
  for pid in "${server_pids[@]}"; do
    wait "$pid" >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

usage() {
  local raft_metrics_default="${bench_raft_metrics:-$capture_metrics}"
  local auth_status="<unset>"
  if [ -n "$bench_http_auth_token" ]; then
    auth_status="<set>"
  fi
  echo "usage: $0 [bench|sample|perf|flamegraph]" >&2
  echo "env: PROFILE_TARGET=$profile_target # broker|raft|redis" >&2
  echo "env: PROFILE=$profile OUT_DIR=$out_dir BROKER_HOST=$broker_host BROKER_HTTP_PORT=$broker_http_port" >&2
  echo "env: REDIS_HOST=$redis_host REDIS_PORT=$redis_port REDIS_DATA_ROOT=$redis_data_root REDIS_SERVER=$redis_server_bin" >&2
  echo "env: RAFT_HTTP_BASE_PORT=$raft_http_base_port RAFT_RPC_BASE_PORT=$raft_rpc_base_port RAFT_SYNC_LOG=$raft_sync_log RAFT_SYNC_COMMIT=$raft_sync_commit" >&2
  echo "env: RAFT_TARGET_QUORUM_EXTRA_FANOUT=$raft_target_quorum_extra_fanout" >&2
  echo "env: RAFT_BENCH_ROUTE=$raft_bench_route # leader|round-robin for PROFILE_TARGET=raft" >&2
  echo "env: BENCH_WORKERS=$bench_workers BENCH_KEYS=$bench_keys BENCH_DURATION_MS=$bench_duration_ms" >&2
  echo "env: BENCH_HTTP_KEEPALIVE=$bench_http_keepalive" >&2
  echo "env: BENCH_HTTP_AUTH_TOKEN=$auth_status # falls back to BENCH_RAFT_AUTH_TOKEN, LMX_LIVE_RAFT_AUTH_TOKEN, ALL_DOGS, or LMX_AUTH_TOKEN" >&2
  echo "env: BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH=${BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH:-<optional>}" >&2
  echo "env: BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE=${BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE:-<optional>}" >&2
  echo "env: BENCH_MAX_RAFT_LOG_APPEND_WRITE_US_PER_CYCLE=${BENCH_MAX_RAFT_LOG_APPEND_WRITE_US_PER_CYCLE:-<optional>}" >&2
  echo "env: BENCH_MAX_RAFT_COMMIT_SLOT_WRITE_US_PER_CYCLE=${BENCH_MAX_RAFT_COMMIT_SLOT_WRITE_US_PER_CYCLE:-<optional>}" >&2
  echo "env: BENCH_RAFT_METRICS=$raft_metrics_default BENCH_RAFT_METRICS_ENDPOINTS=${bench_raft_metrics_endpoints:-<all-local-raft-nodes>}" >&2
  echo "env: CAPTURE_METRICS=$capture_metrics METRICS_TIMEOUT_SECONDS=$metrics_timeout_s" >&2
}

case "$profile_target" in
  broker | raft | redis) ;;
  *)
    echo "PROFILE_TARGET must be broker, raft, or redis; got $profile_target" >&2
    usage
    exit 2
    ;;
esac

if ! [[ "$redis_port" =~ ^[1-9][0-9]*$ ]]; then
  echo "REDIS_PORT must be a positive integer; got $redis_port" >&2
  exit 2
fi

case "$raft_sync_log" in
  true | false) ;;
  1) raft_sync_log=true ;;
  0) raft_sync_log=false ;;
  *)
    echo "RAFT_SYNC_LOG must be true or false; got $raft_sync_log" >&2
    exit 2
    ;;
esac

case "$raft_sync_commit" in
  true | false) ;;
  1) raft_sync_commit=true ;;
  0) raft_sync_commit=false ;;
  *)
    echo "RAFT_SYNC_COMMIT must be true or false; got $raft_sync_commit" >&2
    exit 2
    ;;
esac

case "$raft_bench_route" in
  leader | leader-preferred | leader_preferred) raft_bench_route=leader ;;
  round-robin | round_robin | rr | lb) raft_bench_route=round-robin ;;
  *)
    echo "RAFT_BENCH_ROUTE must be leader or round-robin; got $raft_bench_route" >&2
    exit 2
    ;;
esac

case "$bench_http_keepalive" in
  true | false) ;;
  1) bench_http_keepalive=true ;;
  0) bench_http_keepalive=false ;;
  *)
    echo "BENCH_HTTP_KEEPALIVE must be true or false; got $bench_http_keepalive" >&2
    exit 2
    ;;
esac

artifact_label="$profile_target"
if [ "$profile_target" = "raft" ]; then
  artifact_label="raft-$raft_bench_route"
fi

case "$capture_metrics" in
  true | false) ;;
  1) capture_metrics=true ;;
  0) capture_metrics=false ;;
  *)
    echo "CAPTURE_METRICS must be true or false; got $capture_metrics" >&2
    exit 2
    ;;
esac

bench_raft_metrics="${bench_raft_metrics:-$capture_metrics}"
case "$bench_raft_metrics" in
  true | false) ;;
  1) bench_raft_metrics=true ;;
  0) bench_raft_metrics=false ;;
  *)
    echo "BENCH_RAFT_METRICS must be true or false; got $bench_raft_metrics" >&2
    exit 2
    ;;
esac

case "$mode" in
  bench)
    ;;
  sample)
    if ! command -v sample >/dev/null 2>&1; then
      echo "Apple sample(1) is not available on this host" >&2
      exit 1
    fi
    ;;
  perf)
    if ! command -v perf >/dev/null 2>&1; then
      echo "Linux perf is not available on this host" >&2
      exit 1
    fi
    ;;
  flamegraph)
    if ! command -v flamegraph >/dev/null 2>&1; then
      echo "flamegraph is not installed. Install with: cargo install flamegraph" >&2
      exit 1
    fi
    ;;
  *)
    usage
    exit 2
    ;;
esac

port_open() {
  local host="${1:-$broker_host}"
  local port="${2:-$broker_http_port}"
  (exec 3<>"/dev/tcp/$host/$port") >/dev/null 2>&1
}

http_capture_body() {
  local host="$1"
  local port="$2"
  local path="$3"
  local out="$4"
  if command -v curl >/dev/null 2>&1; then
    curl -sS --max-time "$metrics_timeout_s" "http://$host:$port$path" >"$out"
    return
  fi

  local in_body=false
  exec 3<>"/dev/tcp/$host/$port"
  printf 'GET %s HTTP/1.1\r\nHost: %s\r\nConnection: close\r\n\r\n' "$path" "$host" >&3
  while IFS= read -r line <&3; do
    line="${line%$'\r'}"
    if [ "$in_body" = true ]; then
      printf '%s\n' "$line"
    elif [ -z "$line" ]; then
      in_body=true
    fi
  done >"$out"
  exec 3<&-
  exec 3>&-
}

capture_endpoint_artifacts() {
  local label="$1"
  local port="$2"
  local phase="$3"
  local prefix="$out_dir/$label-$phase"
  if ! http_capture_body "$broker_host" "$port" "/metrics" "$prefix.metrics.prom"; then
    echo "warning: failed to capture $label $phase /metrics" >&2
    rm -f "$prefix.metrics.prom"
  fi
  if [ "$profile_target" = "raft" ]; then
    if ! http_capture_body "$broker_host" "$port" "/raft/status" "$prefix.status.json"; then
      echo "warning: failed to capture $label $phase /raft/status" >&2
      rm -f "$prefix.status.json"
    fi
    if ! http_capture_body "$broker_host" "$port" "/raft/progress" "$prefix.progress.json"; then
      echo "warning: failed to capture $label $phase /raft/progress" >&2
      rm -f "$prefix.progress.json"
    fi
    if ! http_capture_body "$broker_host" "$port" "/raft/leaderz" "$prefix.leaderz.json"; then
      rm -f "$prefix.leaderz.json"
    fi
  else
    if ! http_capture_body "$broker_host" "$port" "/status" "$prefix.status.html"; then
      echo "warning: failed to capture $label $phase /status" >&2
      rm -f "$prefix.status.html"
    fi
  fi
}

capture_redis_artifacts() {
  local phase="$1"
  local prefix="$out_dir/redis-$phase"
  if command -v redis-cli >/dev/null 2>&1; then
    if ! redis-cli -h "$redis_host" -p "$redis_port" INFO >"$prefix.info" 2>"$prefix.info.err"; then
      echo "warning: failed to capture redis INFO" >&2
      rm -f "$prefix.info"
    fi
    rm -f "$prefix.info.err"
  fi
}

capture_profile_artifacts() {
  local phase="$1"
  if [ "$capture_metrics" != true ]; then
    return 0
  fi
  case "$profile_target" in
    raft)
      local idx=0
      for port in "${raft_http_ports[@]}"; do
        capture_endpoint_artifacts "raft-node-$((idx + 1))" "$port" "$phase"
        idx=$((idx + 1))
      done
      ;;
    broker)
      capture_endpoint_artifacts "broker" "$broker_http_port" "$phase"
      ;;
    redis)
      capture_redis_artifacts "$phase"
      ;;
  esac
}

wait_for_port() {
  local host="$1"
  local port="$2"
  local pid="$3"
  for _ in $(seq 1 100); do
    if port_open "$host" "$port"; then
      return 0
    fi
    if [ -n "$pid" ] && ! kill -0 "$pid" >/dev/null 2>&1; then
      echo "server exited before opening $host:$port" >&2
      return 1
    fi
    sleep 0.05
  done
  echo "timed out waiting for server on $host:$port" >&2
  return 1
}

raft_leader_ready() {
  local port="$1"
  local status=""
  exec 3<>"/dev/tcp/$broker_host/$port" || return 1
  printf 'GET /raft/leaderz HTTP/1.1\r\nHost: %s\r\nConnection: close\r\n\r\n' "$broker_host" >&3
  IFS= read -r status <&3 || true
  exec 3<&-
  exec 3>&-
  case "$status" in
    *" 200 "*) return 0 ;;
    *) return 1 ;;
  esac
}

wait_for_raft_leader() {
  for _ in $(seq 1 200); do
    local idx=0
    for port in "${raft_http_ports[@]}"; do
      if raft_leader_ready "$port"; then
        profile_pid="${server_pids[$idx]}"
        raft_leader_http_port="$port"
        return 0
      fi
      idx=$((idx + 1))
    done
    for pid in "${server_pids[@]}"; do
      if ! kill -0 "$pid" >/dev/null 2>&1; then
        echo "BrokerRaft node pid $pid exited before a ready leader was elected" >&2
        return 1
      fi
    done
    sleep 0.05
  done
  echo "timed out waiting for a ready BrokerRaft leader" >&2
  return 1
}

raft_benchmark_endpoint_list() {
  local endpoint_list=""
  for port in "${raft_http_ports[@]}"; do
    if [ -n "$endpoint_list" ]; then
      endpoint_list="$endpoint_list,"
    fi
    endpoint_list="$endpoint_list$broker_host:$port"
  done
  printf '%s' "$endpoint_list"
}

build_binaries() {
  cargo build --no-default-features --profile "$profile" \
    --bin dd-rust-network-mutex \
    --example redis_vs_raft_bench >/dev/null
}

start_broker() {
  if port_open "$broker_host" "$broker_http_port"; then
    echo "$broker_host:$broker_http_port is already in use" >&2
    exit 1
  fi
  server_log="$out_dir/broker-server.log"
  rm -f "$server_log"
  LMX_DISABLE_TCP="${LMX_DISABLE_TCP:-true}" \
    LMX_DISABLE_HTTP="${LMX_DISABLE_HTTP:-false}" \
    LMX_BIND_HOST="$broker_host" \
    LMX_HTTP_PORT="$broker_http_port" \
    LMX_TTL_SWEEP_INTERVAL_MS="${LMX_TTL_SWEEP_INTERVAL_MS:-0}" \
    "target/$profile/dd-rust-network-mutex" >"$server_log" 2>&1 &
  profile_pid=$!
  server_pids+=("$profile_pid")
  wait_for_port "$broker_host" "$broker_http_port" "$profile_pid"
}

start_redis() {
  if ! command -v "$redis_server_bin" >/dev/null 2>&1; then
    echo "redis-server is not available; set REDIS_SERVER=/path/to/redis-server or skip PROFILE_TARGET=redis" >&2
    exit 1
  fi
  if port_open "$redis_host" "$redis_port"; then
    echo "$redis_host:$redis_port is already in use" >&2
    exit 1
  fi
  mkdir -p "$redis_data_root"
  rm -f "$redis_data_root/profile.rdb"
  server_log="$out_dir/redis-server.log"
  rm -f "$server_log"
  "$redis_server_bin" \
    --bind "$redis_host" \
    --port "$redis_port" \
    --save "" \
    --appendonly no \
    --dir "$redis_data_root" \
    --dbfilename profile.rdb \
    --loglevel warning >"$server_log" 2>&1 &
  profile_pid=$!
  server_pids+=("$profile_pid")
  wait_for_port "$redis_host" "$redis_port" "$profile_pid"
}

write_raft_config() {
  local config_path="$1"
  local node_id="$2"
  local http_port="$3"
  local rpc_port="$4"
  local data_dir="$5"

  cat >"$config_path" <<EOF
[server]
bind_host = "$broker_host"
tcp_port = 0
http_port = $http_port
disable_tcp = true
disable_http = false
tcp_nodelay = true
tcp_quickack = true

[broker]
default_ttl_ms = 4000
max_lock_holders = 1
max_concurrency_cap = 1000
ttl_sweep_interval_ms = 0
idle_key_grace_ms = 60000

[raft]
enabled = true
node_id = "$node_id"
bind_addr = "$broker_host:$rpc_port"
advertise_addr = "$broker_host:$rpc_port"
data_dir = "$data_dir"
heartbeat_interval_ms = 50
election_timeout_min_ms = 150
election_timeout_max_ms = 300
snapshot_interval_ms = 1800000
snapshot_max_log_entries = 100000
snapshot_max_log_bytes = 67108864
snapshot_max_log_age_ms = 1800000
trailing_log_entries = 10000
append_entries_max_entries = 256
append_entries_max_bytes = 1048576
append_entries_max_inline_batches = 64
target_quorum_extra_fanout = $raft_target_quorum_extra_fanout
install_snapshot_chunk_bytes = 1048576
install_snapshot_max_staged_bytes = 134217728
install_snapshot_max_staged_transfers = 4
install_snapshot_stale_transfer_ms = 1800000
client_batch_max_entries = 32
client_pipeline_max_batches = 4
client_batch_max_pending = 8192
client_batch_max_delay_ms = 1
client_response_cache_max_entries = 8192
sync_log = $raft_sync_log
sync_commit = $raft_sync_commit

[[raft.peers]]
id = "node-1"
addr = "$broker_host:$((raft_rpc_base_port + 0))"

[[raft.peers]]
id = "node-2"
addr = "$broker_host:$((raft_rpc_base_port + 1))"

[[raft.peers]]
id = "node-3"
addr = "$broker_host:$((raft_rpc_base_port + 2))"
EOF
}

start_raft_cluster() {
  rm -rf "$raft_data_root"
  mkdir -p "$raft_data_root"
  raft_http_ports=()
  raft_leader_http_port=""

  for offset in 0 1 2; do
    local http_port=$((raft_http_base_port + offset))
    local rpc_port=$((raft_rpc_base_port + offset))
    if port_open "$broker_host" "$http_port"; then
      echo "$broker_host:$http_port is already in use" >&2
      exit 1
    fi
    if port_open "$broker_host" "$rpc_port"; then
      echo "$broker_host:$rpc_port is already in use" >&2
      exit 1
    fi
  done

  for offset in 0 1 2; do
    local node_num=$((offset + 1))
    local node_id="node-$node_num"
    local http_port=$((raft_http_base_port + offset))
    local rpc_port=$((raft_rpc_base_port + offset))
    local node_dir="$raft_data_root/$node_id"
    local config_path="$raft_data_root/$node_id.toml"
    local server_log="$out_dir/raft-$node_id-server.log"
    mkdir -p "$node_dir"
    write_raft_config "$config_path" "$node_id" "$http_port" "$rpc_port" "$node_dir"
    rm -f "$server_log"
    LMX_CONFIG="$config_path" \
      "target/$profile/dd-rust-network-mutex" >"$server_log" 2>&1 &
    local pid=$!
    server_pids+=("$pid")
    raft_http_ports+=("$http_port")
  done

  local idx=0
  for port in "${raft_http_ports[@]}"; do
    wait_for_port "$broker_host" "$port" "${server_pids[$idx]}"
    idx=$((idx + 1))
  done
  wait_for_raft_leader
}

start_benchmark() {
  bench_log="$out_dir/$artifact_label-bench.out"
  rm -f "$bench_log"
  capture_profile_artifacts before
  if [ "$profile_target" = "redis" ]; then
    BENCH_TARGET=redis \
      BENCH_REDIS="$redis_host:$redis_port" \
      BENCH_WORKERS="$bench_workers" \
      BENCH_KEYS="$bench_keys" \
      BENCH_DURATION_MS="$bench_duration_ms" \
      BENCH_TTL_MS="$bench_ttl_ms" \
      BENCH_IO_TIMEOUT_MS="$bench_io_timeout_ms" \
      "target/$profile/examples/redis_vs_raft_bench" >"$bench_log" 2>&1 &
  elif [ "$profile_target" = "raft" ]; then
    local raft_bench_endpoints
    local raft_metrics_enabled
    local raft_metrics_endpoints
    if [ "$raft_bench_route" = "leader" ]; then
      raft_bench_endpoints="$broker_host:$raft_leader_http_port"
    else
      raft_bench_endpoints="$(raft_benchmark_endpoint_list)"
    fi
    raft_metrics_enabled="${bench_raft_metrics:-$capture_metrics}"
    raft_metrics_endpoints="${bench_raft_metrics_endpoints:-$(raft_benchmark_endpoint_list)}"
    BENCH_TARGET=raft \
      BENCH_RAFT="$raft_bench_endpoints" \
      BENCH_RAFT_METRICS_ENDPOINTS="$raft_metrics_endpoints" \
      BENCH_RAFT_ROUTE="$raft_bench_route" \
      BENCH_HTTP_AUTH_TOKEN="$bench_http_auth_token" \
      BENCH_WORKERS="$bench_workers" \
      BENCH_KEYS="$bench_keys" \
      BENCH_DURATION_MS="$bench_duration_ms" \
      BENCH_TTL_MS="$bench_ttl_ms" \
      BENCH_IO_TIMEOUT_MS="$bench_io_timeout_ms" \
      BENCH_HTTP_KEEPALIVE="$bench_http_keepalive" \
      BENCH_RAFT_METRICS="$raft_metrics_enabled" \
      "target/$profile/examples/redis_vs_raft_bench" >"$bench_log" 2>&1 &
  else
    BENCH_TARGET=broker \
      BENCH_BROKER="$broker_host:$broker_http_port" \
      BENCH_HTTP_AUTH_TOKEN="$bench_http_auth_token" \
      BENCH_WORKERS="$bench_workers" \
      BENCH_KEYS="$bench_keys" \
      BENCH_DURATION_MS="$bench_duration_ms" \
      BENCH_TTL_MS="$bench_ttl_ms" \
      BENCH_IO_TIMEOUT_MS="$bench_io_timeout_ms" \
      BENCH_HTTP_KEEPALIVE="$bench_http_keepalive" \
      "target/$profile/examples/redis_vs_raft_bench" >"$bench_log" 2>&1 &
  fi
  bench_pid=$!
}

finish_benchmark() {
  bench_status=0
  wait "$bench_pid" || bench_status=$?
  bench_pid=""
  capture_profile_artifacts after
}

run_flamegraph_capture() {
  local out="$1"
  local status=0
  flamegraph --pid "$profile_pid" --output "$out" &
  local flamegraph_pid=$!
  (
    sleep "$sample_seconds"
    kill -INT "$flamegraph_pid" >/dev/null 2>&1 || true
  ) &
  local flamegraph_stop_pid=$!
  wait "$flamegraph_pid" || status=$?
  kill "$flamegraph_stop_pid" >/dev/null 2>&1 || true
  wait "$flamegraph_stop_pid" >/dev/null 2>&1 || true
  if [ "$status" -ne 0 ] && [ ! -s "$out" ]; then
    echo "flamegraph failed with status $status and did not write $out" >&2
    return "$status"
  fi
  if [ "$status" -ne 0 ]; then
    echo "flamegraph exited with status $status after writing $out" >&2
  fi
  return 0
}

fail_if_benchmark_failed() {
  if [ "$bench_status" -ne 0 ]; then
    echo "benchmark failed with status $bench_status; see $bench_log" >&2
    exit "$bench_status"
  fi
}

build_binaries
case "$profile_target" in
  broker)
    start_broker
    ;;
  raft)
    start_raft_cluster
    ;;
  redis)
    start_redis
    ;;
esac

case "$mode" in
  bench)
    start_benchmark
    finish_benchmark
    fail_if_benchmark_failed
    cat "$bench_log"
    echo "$bench_log"
    ;;
  sample)
    out="$out_dir/$artifact_label-server.sample.txt"
    rm -f "$out"
    start_benchmark
    sample_status=0
    sample "$profile_pid" "$sample_seconds" "$sample_interval_ms" -mayDie -file "$out" || sample_status=$?
    finish_benchmark
    if [ "$sample_status" -ne 0 ]; then
      echo "sample failed with status $sample_status; macOS may require sudo or full Xcode/Instruments permissions" >&2
      exit "$sample_status"
    fi
    fail_if_benchmark_failed
    echo "$out"
    ;;
  perf)
    out="$out_dir/$artifact_label-server.perf.data"
    start_benchmark
    perf_status=0
    perf record -F "$perf_freq" -g -p "$profile_pid" -o "$out" -- sleep "$sample_seconds" || perf_status=$?
    finish_benchmark
    if [ "$perf_status" -ne 0 ]; then
      echo "perf failed with status $perf_status" >&2
      exit "$perf_status"
    fi
    fail_if_benchmark_failed
    perf report -i "$out" --stdio | tee "$out.report.txt"
    echo "$out"
    ;;
  flamegraph)
    out="$out_dir/$artifact_label-server.svg"
    start_benchmark
    flamegraph_status=0
    run_flamegraph_capture "$out" || flamegraph_status=$?
    finish_benchmark
    if [ "$flamegraph_status" -ne 0 ]; then
      exit "$flamegraph_status"
    fi
    fail_if_benchmark_failed
    echo "$out"
    ;;
esac
