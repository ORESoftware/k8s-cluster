#!/usr/bin/env bash
set -euo pipefail

repo="ORESoftware/k8s-cluster"
workflow="remote-k8s-maintenance.yml"
ref="${GCS_LOADTEST_REF:-dev}"
target_revision="${GCS_LOADTEST_TARGET_REVISION:-dev}"
duration="${GCS_LOADTEST_DURATION_SECONDS:-180}"
loader_replicas="${GCS_LOADTEST_LOADER_REPLICAS:-3}"
parallel_jobs="${GCS_LOADTEST_PARALLEL_JOBS:-3}"
gcs_replicas="${GCS_LOADTEST_GCS_REPLICAS:-1}"
collect_pprof="${GCS_LOADTEST_COLLECT_PPROF:-true}"
out_dir="${GCS_LOADTEST_OUT_DIR:-/private/tmp/gcs-loadtest-proof-$(date -u +%Y%m%dT%H%M%SZ)}"

mkdir -p "$out_dir"

run_case() {
  local label="$1"
  local aggregate="$2"
  local rate="$3"
  local run_id=""
  local watch_status=0
  local log_path="${out_dir}/${label}.log"
  local proof_path="${out_dir}/${label}.proof.log"

  echo "=== dispatch ${label}: aggregate=${aggregate} rate=${rate} gcs_replicas=${gcs_replicas} ==="
  gh workflow run "$workflow" \
    --repo "$repo" \
    --ref "$ref" \
    -f operation=loadtest-gcs-wss \
    -f target_revision="$target_revision" \
    -f loadtest_label="${label}" \
    -f loadtest_duration_seconds="$duration" \
    -f loadtest_loader_replicas="$loader_replicas" \
    -f loadtest_aggregate_connections="$aggregate" \
    -f loadtest_parallel_jobs="$parallel_jobs" \
    -f loadtest_gcs_replicas="$gcs_replicas" \
    -f loadtest_messages_per_second_per_client="$rate" \
    -f loadtest_collect_pprof="$collect_pprof"

  sleep 12
  run_id="$(gh run list \
    --repo "$repo" \
    --workflow "$workflow" \
    --event workflow_dispatch \
    --branch "$ref" \
    --limit 1 \
    --json databaseId \
    --jq '.[0].databaseId')"

  if [ -z "$run_id" ] || [ "$run_id" = "null" ]; then
    echo "ERROR: could not discover GitHub Actions run id for ${label}" >&2
    exit 1
  fi

  echo "=== watch ${label}: run_id=${run_id} ==="
  set +e
  gh run watch "$run_id" --repo "$repo" --exit-status
  watch_status="$?"
  set -e

  echo "=== collect ${label}: run_id=${run_id} ==="
  gh run view "$run_id" --repo "$repo" --log > "$log_path"
  grep -E '^(PROOF|proof-summary|ERROR:|WARNING:|pprof-|top-sample|gcs-summary|loki-summary|prom-|correctness_result=|ALL TESTS PASSED)' "$log_path" > "$proof_path" || true

  echo "=== proof ${label}: ${proof_path} ==="
  grep -E '^(PROOF loadtest_target|PROOF loader_pass|PROOF dependency_health|PROOF pprof_profile|PROOF service_health|PROOF loadtest_all_passed|ERROR:|WARNING:)' "$proof_path" || true

  if [ "$watch_status" -ne 0 ]; then
    echo "ERROR: ${label} failed; full log: ${log_path}" >&2
    exit "$watch_status"
  fi
}

run_case "40k-light-1pod-pprof" "40000" "1.0"
run_case "40k-medium-1pod-pprof" "40000" "2.5"
run_case "50k-light-1pod-pprof" "50000" "1.0"
run_case "50k-medium-1pod-pprof" "50000" "2.5"

echo "=== complete: proof logs in ${out_dir} ==="
