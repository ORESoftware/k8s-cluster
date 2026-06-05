#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf '%s\n' \
    "Usage: bash scripts/flamegraph-vapi.sh <check|local|attach> [pid]" \
    "" \
    "Commands:" \
    "  check        Verify flamegraph prerequisites without profiling." \
    "  local        Run dd-rust-vapi-phone under cargo flamegraph for a bounded local profile." \
    "  attach PID   Attach flamegraph to an already-running process for a bounded profile." \
    "" \
    "Environment:" \
    "  DURATION_SECONDS   Profile duration for local/attach modes. Default: 60." \
    "  HOST               Local bind host for local mode. Default: 127.0.0.1." \
    "  PORT               Local bind port for local mode. Default: 18113." \
    "  VAPI_FLAMEGRAPH_DIR Directory for route-visible SVG + metadata output." \
    "  OUTPUT             Output SVG path. Default: VAPI_FLAMEGRAPH_DIR/dd-rust-vapi-phone-<timestamp>.svg." \
    "  METADATA_PATH      Run metadata JSON path. Default: output directory/latest.json."
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
service_dir="$(cd "$script_dir/.." && pwd)"
manifest_path="$service_dir/Cargo.toml"
duration_seconds="${DURATION_SECONDS:-60}"
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
if [ -n "${VAPI_FLAMEGRAPH_DIR:-}" ]; then
  flamegraph_dir="${VAPI_FLAMEGRAPH_DIR%/}"
elif [ -n "${CARGO_TARGET_DIR:-}" ]; then
  flamegraph_dir="${CARGO_TARGET_DIR%/}/flamegraphs"
else
  flamegraph_dir="$service_dir/target/flamegraphs"
fi
output="${OUTPUT:-$flamegraph_dir/dd-rust-vapi-phone-$timestamp.svg}"
metadata_path="${METADATA_PATH:-$(dirname "$output")/latest.json}"
command_name="${1:-check}"

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'missing required command: %s\n' "$1" >&2
    return 1
  fi
}

check_prereqs() {
  need_command cargo
  need_command flamegraph
  need_command cargo-flamegraph

  case "$(uname -s)" in
    Linux)
      need_command perf
      if [ -r /proc/sys/kernel/perf_event_paranoid ]; then
        perf_paranoid="$(cat /proc/sys/kernel/perf_event_paranoid)"
        printf 'perf_event_paranoid=%s\n' "$perf_paranoid"
      fi
      ;;
    Darwin)
      need_command xctrace
      ;;
    *)
      printf 'unsupported profiler host OS: %s\n' "$(uname -s)" >&2
      return 1
      ;;
  esac
}

profiling_rustflags() {
  flags="${RUSTFLAGS:-}"
  flags="$flags -C force-frame-pointers=yes"
  if [ "$(uname -s)" = "Linux" ]; then
    flags="$flags -Clink-arg=-Wl,--no-rosegment"
  fi
  printf '%s' "$flags"
}

run_with_timeout() {
  if ! command -v timeout >/dev/null 2>&1; then
    printf 'missing required command: timeout\n' >&2
    printf 'Install coreutils or run flamegraph manually and stop it with Ctrl-C.\n' >&2
    return 1
  fi
  set +e
  timeout -s INT "${duration_seconds}s" "$@"
  status="$?"
  set -e
  if [ "$status" -eq 124 ]; then
    return 0
  fi
  return "$status"
}

json_escape() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//$'\n'/\\n}"
  value="${value//$'\r'/\\r}"
  value="${value//$'\t'/\\t}"
  printf '%s' "$value"
}

write_metadata() {
  local mode="$1"
  local pid="${2:-}"
  local run_finished_at
  local run_finished_epoch
  local actual_duration_seconds
  local svg_file
  local pid_json

  run_finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  run_finished_epoch="$(date -u +%s)"
  actual_duration_seconds="$((run_finished_epoch - run_started_epoch))"
  svg_file="$(basename "$output")"
  if [ -n "$pid" ]; then
    pid_json="\"$(json_escape "$pid")\""
  else
    pid_json="null"
  fi

  mkdir -p "$(dirname "$metadata_path")"
  cat > "$metadata_path" <<EOF
{
  "service": "dd-rust-vapi-phone",
  "mode": "$(json_escape "$mode")",
  "pid": $pid_json,
  "runStartedAtUtc": "$(json_escape "$run_started_at")",
  "runFinishedAtUtc": "$(json_escape "$run_finished_at")",
  "durationSeconds": $actual_duration_seconds,
  "svgFile": "$(json_escape "$svg_file")",
  "outputPath": "$(json_escape "$output")"
}
EOF
  printf 'metadata=%s\n' "$metadata_path"
}

run_local_profile() {
  check_prereqs
  mkdir -p "$(dirname "$output")"
  export CARGO_PROFILE_RELEASE_DEBUG="${CARGO_PROFILE_RELEASE_DEBUG:-true}"
  export RUSTFLAGS
  RUSTFLAGS="$(profiling_rustflags)"
  export HOST="${HOST:-127.0.0.1}"
  export PORT="${PORT:-18113}"
  export VAPI_SERVER_SECRET="${VAPI_SERVER_SECRET:-flamegraph-local-vapi-secret}"
  export SERVER_AUTH_SECRET="${SERVER_AUTH_SECRET:-flamegraph-local-admin-secret}"

  printf 'profiling local dd-rust-vapi-phone for %ss\n' "$duration_seconds"
  printf 'output=%s\n' "$output"
  run_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  run_started_epoch="$(date -u +%s)"
  run_with_timeout cargo flamegraph \
    --manifest-path "$manifest_path" \
    --bin dd-rust-vapi-phone \
    --output "$output"
  write_metadata "local"
}

run_attach_profile() {
  pid="${1:-}"
  if [ -z "$pid" ]; then
    usage >&2
    return 2
  fi
  check_prereqs
  mkdir -p "$(dirname "$output")"
  printf 'profiling pid %s for %ss\n' "$pid" "$duration_seconds"
  printf 'output=%s\n' "$output"
  run_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  run_started_epoch="$(date -u +%s)"
  run_with_timeout flamegraph --pid "$pid" --output "$output"
  write_metadata "attach" "$pid"
}

case "$command_name" in
  check)
    check_prereqs
    ;;
  local)
    run_local_profile
    ;;
  attach)
    shift
    run_attach_profile "${1:-}"
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
