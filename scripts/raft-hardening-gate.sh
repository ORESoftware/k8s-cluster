#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
cd "$repo_root"

mode="${LMX_RAFT_GATE_MODE:-full}"
profile="${PROFILE:-profiling}"
bench_workers="${BENCH_WORKERS:-4}"
bench_keys="${BENCH_KEYS:-128}"
bench_duration_ms="${BENCH_DURATION_MS:-3000}"
bench_min_raft_batch_entries="${BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH:-}"
bench_max_raft_commit_slot_writes="${BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE:-}"
bench_max_raft_log_append_write_us="${BENCH_MAX_RAFT_LOG_APPEND_WRITE_US_PER_CYCLE:-}"
bench_max_raft_commit_slot_write_us="${BENCH_MAX_RAFT_COMMIT_SLOT_WRITE_US_PER_CYCLE:-}"
run_bench="${RUN_BENCH:-true}"
run_clippy="${RUN_CLIPPY:-true}"
clippy_scope="${CLIPPY_SCOPE:-all-targets}"
run_secret_scan="${RUN_SECRET_SCAN:-true}"
run_redis_bench="${RUN_REDIS_BENCH:-false}"
run_k8s_raft_live="${RUN_K8S_RAFT_LIVE:-false}"
run_k8s_raft_live_require_metrics="${RUN_K8S_RAFT_LIVE_REQUIRE_METRICS:-true}"
run_raft_version_skew="${RUN_RAFT_VERSION_SKEW:-false}"
test_threads="${TEST_THREADS:-1}"
redis_host="${REDIS_HOST:-127.0.0.1}"
redis_port="${REDIS_PORT:-17379}"
broker_raft_test_filters=()

usage() {
  cat >&2 <<EOF
usage: LMX_RAFT_GATE_MODE=quick|full RUN_BENCH=true|false $0

quick: formatting, benchmark-example tests, self-audited BrokerRaft unit coverage, raft_cluster
full:  quick checks plus full no-default-features lib tests and local Broker/BrokerRaft bench evidence

env:
  PROFILE=$profile
  BENCH_WORKERS=$bench_workers
  BENCH_KEYS=$bench_keys
  BENCH_DURATION_MS=$bench_duration_ms
  RUN_BENCH=$run_bench
  RUN_CLIPPY=$run_clippy
  CLIPPY_SCOPE=$clippy_scope
  RUN_SECRET_SCAN=$run_secret_scan
  RUN_REDIS_BENCH=$run_redis_bench
  RUN_K8S_RAFT_LIVE=$run_k8s_raft_live
  RUN_K8S_RAFT_LIVE_REQUIRE_METRICS=$run_k8s_raft_live_require_metrics
  RUN_RAFT_VERSION_SKEW=$run_raft_version_skew
  TEST_THREADS=$test_threads
  REDIS_HOST=$redis_host
  REDIS_PORT=$redis_port
  BENCH_FAIL_ON_ZERO_SUCCESS=true|false
  BENCH_MIN_REDIS_OPS_PER_SEC=<optional>
  BENCH_MIN_BROKER_OPS_PER_SEC=<optional>
  BENCH_MIN_RAFT_OPS_PER_SEC=<optional>
  BENCH_MAX_REDIS_P99_MS=<optional>
  BENCH_MAX_BROKER_P99_MS=<optional>
  BENCH_MAX_RAFT_P99_MS=<optional>
  BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH=${bench_min_raft_batch_entries:-<optional>}
  BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE=${bench_max_raft_commit_slot_writes:-<optional>}
  BENCH_MAX_RAFT_LOG_APPEND_WRITE_US_PER_CYCLE=${bench_max_raft_log_append_write_us:-<optional>}
  BENCH_MAX_RAFT_COMMIT_SLOT_WRITE_US_PER_CYCLE=${bench_max_raft_commit_slot_write_us:-<optional>}
EOF
}

case "$mode" in
  quick | full) ;;
  *)
    usage
    exit 2
    ;;
esac

case "$run_bench" in
  true | false) ;;
  1) run_bench=true ;;
  0) run_bench=false ;;
  *)
    echo "RUN_BENCH must be true or false; got $run_bench" >&2
    exit 2
    ;;
esac

case "$run_clippy" in
  true | false) ;;
  1) run_clippy=true ;;
  0) run_clippy=false ;;
  *)
    echo "RUN_CLIPPY must be true or false; got $run_clippy" >&2
    exit 2
    ;;
esac

case "$clippy_scope" in
  all-targets | lib) ;;
  *)
    echo "CLIPPY_SCOPE must be all-targets or lib; got $clippy_scope" >&2
    exit 2
    ;;
esac

case "$run_secret_scan" in
  true | false) ;;
  1) run_secret_scan=true ;;
  0) run_secret_scan=false ;;
  *)
    echo "RUN_SECRET_SCAN must be true or false; got $run_secret_scan" >&2
    exit 2
    ;;
esac

case "$run_redis_bench" in
  true | false) ;;
  1) run_redis_bench=true ;;
  0) run_redis_bench=false ;;
  *)
    echo "RUN_REDIS_BENCH must be true or false; got $run_redis_bench" >&2
    exit 2
    ;;
esac

if ! [[ "$redis_port" =~ ^[1-9][0-9]*$ ]]; then
  echo "REDIS_PORT must be a positive integer; got $redis_port" >&2
  exit 2
fi

case "$run_k8s_raft_live" in
  true | false) ;;
  1) run_k8s_raft_live=true ;;
  0) run_k8s_raft_live=false ;;
  *)
    echo "RUN_K8S_RAFT_LIVE must be true or false; got $run_k8s_raft_live" >&2
    exit 2
    ;;
esac

case "$run_k8s_raft_live_require_metrics" in
  true | false) ;;
  1) run_k8s_raft_live_require_metrics=true ;;
  0) run_k8s_raft_live_require_metrics=false ;;
  *)
    echo "RUN_K8S_RAFT_LIVE_REQUIRE_METRICS must be true or false; got $run_k8s_raft_live_require_metrics" >&2
    exit 2
    ;;
esac

case "$run_raft_version_skew" in
  true | false) ;;
  1) run_raft_version_skew=true ;;
  0) run_raft_version_skew=false ;;
  *)
    echo "RUN_RAFT_VERSION_SKEW must be true or false; got $run_raft_version_skew" >&2
    exit 2
    ;;
esac

run_broker_raft_tests() {
  local filter="$1"
  run cargo test --no-default-features \
    "broker_raft::tests::$filter" \
    --lib -- --test-threads="$test_threads"
}

run_broker_raft_singletons() {
  local filter
  for filter in "$@"; do
    run_broker_raft_tests "$filter"
  done
}

record_broker_raft_test_filters() {
  local arg
  for arg in "$@"; do
    case "$arg" in
      broker_raft::tests::*)
        broker_raft_test_filters+=("$arg")
        ;;
    esac
  done
}

run() {
  record_broker_raft_test_filters "$@"
  printf '\n==> %s\n' "$*" >&2
  "$@"
}

require_live_metrics_endpoints_for_k8s_gate() {
  if [ "$run_k8s_raft_live" != true ] || [ "$run_k8s_raft_live_require_metrics" != true ]; then
    return
  fi
  if ! printf '%s' "${LMX_LIVE_RAFT_METRICS_ENDPOINTS:-}" | rg -q '[^[:space:],]'; then
    cat >&2 <<EOF
RUN_K8S_RAFT_LIVE=true requires LMX_LIVE_RAFT_METRICS_ENDPOINTS by default so the live gate guards every Raft pod against full-log hot-path fallback.
Set LMX_LIVE_RAFT_METRICS_ENDPOINTS to one stable HTTP endpoint per expected Raft node, or set RUN_K8S_RAFT_LIVE_REQUIRE_METRICS=false for a behavior-only live smoke.
EOF
    exit 2
  fi
}

check_secret_patterns() {
  printf '\n==> repository secret pattern scan\n' >&2
  local sts_token_prefix="IQoJb3Jp"
  local sts_token_suffix="Z2luX2Vj"
  local aws_access_key_id_name="ACCESS_KEY_ID"
  local aws_secret_access_key_name="SECRET_ACCESS_KEY"
  local aws_session_token_name="SESSION_TOKEN"
  local aws_credential_expiration_name="CREDENTIAL_EXPIRATION"
  local pattern="ASIA[0-9A-Z]{16}|${sts_token_prefix}${sts_token_suffix}|AWS_(${aws_access_key_id_name}|${aws_secret_access_key_name}|${aws_session_token_name}|${aws_credential_expiration_name})[[:space:]]*="
  local output=""
  local status=0
  set +e
  output="$(
    rg -n --hidden \
      --glob '!target/**' \
      --glob '!.git/**' \
      --glob '!vendor/**' \
      "$pattern" \
      .
  )"
  status=$?
  set -e
  case "$status" in
    0)
      printf '%s\n' "$output" >&2
      echo "credential-like AWS patterns found in repository files" >&2
      exit 1
      ;;
    1)
      ;;
    *)
      printf '%s\n' "$output" >&2
      echo "secret pattern scan failed with status $status" >&2
      exit "$status"
      ;;
  esac
}

check_old_protocol_fixture_contract() {
  printf '\n==> old-protocol raft RPC fixture contract\n' >&2
  if grep -n "minProtocolVersion" tests/fixtures/raft/old_protocol_*.json; then
    echo "old-protocol raft RPC fixtures must not contain minProtocolVersion" >&2
    exit 1
  fi
}

check_raft_full_log_call_allowlist() {
  printf '\n==> raft full-log primitive call allowlist\n' >&2
  local output=""
  local status=0
  set +e
  output="$(
    awk '
      /^[[:space:]]*mod[[:space:]]+tests[[:space:]]*\{/ {
        in_tests = 1
      }
      in_tests {
        next
      }
      /^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+[A-Za-z0-9_]+/ {
        line = $0
        sub(/^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+/, "", line)
        sub(/[<(].*/, "", line)
        current_fn = line
      }
      /(read_log_entries|rewrite_log)[[:space:]]*\(/ &&
        $0 !~ /^[[:space:]]*fn[[:space:]]+(read_log_entries|rewrite_log)[[:space:]]*\(/ {
        allowed = 0
        if (current_fn == "open_with_sync_policy_and_telemetry" || current_fn == "read_entries" || current_fn == "replace_all" || current_fn == "reload_retained_log_from_disk_locked" || current_fn == "install_snapshot_from_leader" || current_fn == "compact_through") {
          allowed = 1
        }
        if (!allowed) {
          printf "%s:%d: %s calls full-log helper directly: %s\n", FILENAME, FNR, current_fn, $0
        }
      }
    ' src/broker_raft.rs
  )"
  status=$?
  set -e
  if [ "$status" -ne 0 ]; then
    printf '%s\n' "$output" >&2
    echo "raft full-log primitive allowlist scan failed with status $status" >&2
    exit "$status"
  fi
  if [ -n "$output" ]; then
    printf '%s\n' "$output" >&2
    cat >&2 <<EOF
Unexpected direct full-log read/rewrite helper call found.
If this is startup, recovery, compaction, snapshot install, or a test-only helper, add the function to the allowlist with a comment in docs/raft.md.
Replication and proposal hot paths must keep using retained caches, bounded AppendEntries, append-only writes, truncate+append repair, or InstallSnapshot.
EOF
    exit 1
  fi
}

check_broker_raft_quick_gate_unit_coverage() {
  printf '\n==> broker_raft quick gate unit-test coverage audit\n' >&2
  local all_tests_file=""
  local filters_file=""
  local missing_file=""
  all_tests_file="$(mktemp "${TMPDIR:-/tmp}/lmx-raft-all-tests.XXXXXX")"
  filters_file="$(mktemp "${TMPDIR:-/tmp}/lmx-raft-gate-filters.XXXXXX")"
  missing_file="$(mktemp "${TMPDIR:-/tmp}/lmx-raft-missing-tests.XXXXXX")"

  cargo test --no-default-features --lib -- --list |
    awk '/^broker_raft::tests::/ { sub(/: test$/, ""); print }' |
    sort -u >"$all_tests_file"

  if [ "${#broker_raft_test_filters[@]}" -eq 0 ]; then
    echo "no BrokerRaft unit-test filters were recorded before coverage audit" >&2
    exit 1
  fi

  printf '%s\n' "${broker_raft_test_filters[@]}" | sort -u >"$filters_file"
  awk '
    NR == FNR {
      filters[++filter_count] = $0
      next
    }
    {
      covered = 0
      for (i = 1; i <= filter_count; i++) {
        if (index($0, filters[i]) > 0) {
          covered = 1
          break
        }
      }
      if (!covered) {
        print
      }
    }
  ' "$filters_file" "$all_tests_file" >"$missing_file"

  if [ -s "$missing_file" ]; then
    cat "$missing_file" >&2
    cat >&2 <<EOF
BrokerRaft unit tests are not covered by the quick hardening gate.
Add a broad run_broker_raft_tests prefix when the test belongs to a family, or
add an explicit run_broker_raft_singletons entry when it is a standalone guard.
EOF
    exit 1
  fi

  rm -f "$all_tests_file" "$filters_file" "$missing_file"
}

run cargo fmt --check
check_old_protocol_fixture_contract
check_raft_full_log_call_allowlist
require_live_metrics_endpoints_for_k8s_gate
if [ "$run_secret_scan" = true ]; then
  check_secret_patterns
fi

if [ "$run_clippy" = true ]; then
  if [ "$clippy_scope" = all-targets ]; then
    run cargo clippy --no-default-features --all-targets -- \
      -A warnings \
      -D clippy::too_many_arguments \
      -D clippy::result_large_err
  else
    run cargo clippy --no-default-features --lib -- \
      -A warnings \
      -D clippy::too_many_arguments \
      -D clippy::result_large_err
  fi
fi

run cargo test --no-default-features --example redis_vs_raft_bench

run cargo test --no-default-features \
  config::tests::shipped_raft_config_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::fuzz_append_entries_conflict_repair_converges_in_bounded_batches \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::append_ \
  --lib -- --test-threads="$test_threads"

run_broker_raft_tests handle_append_entries_
run_broker_raft_tests hard_state_
run_broker_raft_tests changed_hard_state_
run_broker_raft_tests leader_
run_broker_raft_tests candidate_
run_broker_raft_tests step_
run_broker_raft_tests stale_
run_broker_raft_tests higher_
run_broker_raft_tests heartbeat_

run cargo test --no-default-features \
  broker_raft::tests::fuzz_log_store_append_replace_and_compact_invariants \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::change_membership_ \
  --lib -- --test-threads="$test_threads"

run_broker_raft_tests broker_open_
run_broker_raft_tests broker_raft_config_
run_broker_raft_tests broker_raft_open_
run_broker_raft_tests open_
run_broker_raft_tests log_
run_broker_raft_tests local_
run_broker_raft_tests replace_
run_broker_raft_tests rollback_
run_broker_raft_tests entries_range_
run_broker_raft_tests byte_
run_broker_raft_tests cached_
run_broker_raft_tests visible_
run_broker_raft_tests term_

run cargo test --no-default-features \
  broker_raft::tests::pre_vote_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::request_vote_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::election_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::membership_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::apply_membership_ \
  --lib -- --test-threads="$test_threads"

run_broker_raft_tests apply_staged_learners_
run_broker_raft_tests apply_committed_
run_broker_raft_tests stage_learners_
run_broker_raft_tests remove_staged_learners_
run_broker_raft_tests staged_
run_broker_raft_tests learner_
run_broker_raft_tests promoted_
run_broker_raft_tests joint_
run_broker_raft_tests failed_
run_broker_raft_tests removed_

run cargo test --no-default-features \
  broker_raft::tests::maintenance_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::snapshot_maintenance_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::threshold_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::overdue_snapshot_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::compaction_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::age_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::install_snapshot_stream_stops_and_counts_when_prepared_snapshot_goes_stale \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::install_snapshot_ \
  --lib -- --test-threads="$test_threads"

run_broker_raft_tests handle_install_snapshot_

run cargo test --no-default-features \
  broker_raft::tests::handle_install_snapshot_rejects_missing_or_bad_checksum \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::handle_install_snapshot_rejects_same_boundary_checksum_conflict \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::install_snapshot_rejects_request_identity_conflict_with_retained_suffix_before_rewrite \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::handle_install_snapshot_rejects_request_identity_conflict_with_retained_suffix \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::already_installed_snapshot_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::write_snapshot_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::write_snapshot_rejects_same_boundary_request_identity_conflict_with_retained_suffix \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::write_snapshot_checksumless_upgrade_rejects_retained_request_identity_conflict_before_write \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::final_install_snapshot_ \
  --lib -- --test-threads="$test_threads"

run_broker_raft_tests snapshot_payload_
run_broker_raft_tests snapshot_
run_broker_raft_tests persisted_log_
run_broker_raft_tests log_
run_broker_raft_tests retained_
run_broker_raft_tests invalid_
run_broker_raft_tests large_
run_broker_raft_tests legacy_

run cargo test --no-default-features \
  broker::tests::raft_snapshot_validation_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::client_request_queue_limit_rejects_before_append \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::active_quorum_admission_failure_rejects_client_write_before_append \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::concurrent_client_requests_share_one_replicated_batch \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::client_pipeline_drains_multiple_configured_batches_in_one_quorum_round \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::client_batch_refills_pipeline_after_commit_lock_wait \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::client_batch_prunes_requests_cancelled_while_waiting_for_commit_lock \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::client_batch_short_commit_index_result_releases_unapplied_identity \
  --lib -- --test-threads="$test_threads"

run_broker_raft_tests client_batch_
run_broker_raft_tests client_proposal_
run_broker_raft_tests committed_
run_broker_raft_tests duplicate_
run_broker_raft_tests deterministic_
run_broker_raft_tests public_
run_broker_raft_tests quorum_
run_broker_raft_tests progress_

run cargo test --no-default-features \
  broker_raft::tests::follower_proxy_ \
  --lib -- --test-threads="$test_threads"

run_broker_raft_tests follower_
run_broker_raft_tests proxy_
run_broker_raft_tests pooled_peer_rpc_
run_broker_raft_tests rpc_
run_broker_raft_tests raft_rpc_

run cargo test --no-default-features \
  broker_raft::tests::target_replication_peer_selection_replaces_busy_joint_overlap_peer \
  --lib -- --test-threads="$test_threads"

run_broker_raft_tests target_replication_
run_broker_raft_tests replicate_

run cargo test --no-default-features \
  broker_raft::tests::target_replication_limits_simple_fanout_to_quorum_need \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::target_replication_extra_fanout_contacts_spare_but_returns_on_quorum \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::target_replication_limits_joint_fanout_to_both_quorum_needs \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::membership_replication_limits_joint_fanout_before_joint_is_active \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::target_replication_limited_fanout_rotates_after_failed_peer \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::target_replication_rotates_around_busy_selected_peer \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::no_target_fanout_catches_up_over_bounded_append_batches \
  --lib -- --test-threads="$test_threads"

run_broker_raft_tests no_target_fanout_

run cargo test --no-default-features \
  broker_raft::tests::no_target_fanout_yields_after_inline_append_batch_cap \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::post_commit_fanout_refills_after_inline_append_batch_yield \
  --lib -- --test-threads="$test_threads"

run_broker_raft_tests post_commit_fanout_

run cargo test --no-default-features \
  broker_raft::tests::supplied_tail_cached_progress_check_does_not_lock_log_store \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::startup_rewrites_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::broker_raft_open_uses_configured_startup_retention \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::retained_log_reload_rewrites_snapshot_covered_prefix_with_configured_retention \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::entries_from_limited_ \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::prev_term_and_entries_limited_uses_retained_cache_without_full_log_scan \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::last_index_for_term_uses_retained_term_index_without_full_log_scan \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::term_at_uses_retained_index_without_full_log_scan \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::append_entries_conflict_hint_uses_retained_first_term_index \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::leader_snapshot_fallback_handles_unknown_conflict_term_below_retained_floor \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::conflict_below_retained_floor_falls_back_to_install_snapshot \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::raft_rpc_requests_tolerate_unknown_future_fields_and_defaulted_old_fields \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::raft_rpc_responses_tolerate_unknown_future_fields_and_defaulted_old_fields \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::literal_old_protocol_raft_rpc_request_wire_fixtures_decode \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::literal_old_protocol_raft_rpc_response_wire_fixtures_decode \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::raft_rpc_serializers_advertise_current_protocol_minimum \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::raft_rpc_protocol_minimum_rejections_are_counted_and_exported \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::raft_rpc_protocol_minimum_scan_only_uses_top_level_field \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::future_protocol_pre_vote_response_does_not_start_election \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::future_protocol_request_vote_response_does_not_elect_candidate \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::future_protocol_proxy_response_does_not_return_client_success \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::future_protocol_append_response_does_not_count_as_quorum_contact \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::future_protocol_install_snapshot_response_does_not_update_progress \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::append_response_from_replaced_peer_address_does_not_update_progress \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::install_snapshot_response_from_replaced_peer_address_does_not_update_progress \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::follower_proxy_ignores_success_from_replaced_peer_address \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::missing_peer_rpc_token_warning_only_targets_network_clusters \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::raft_rpc_stream_returns_error_for_unsupported_protocol_minimum \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::old_protocol_pre_vote_stream_accepts_missing_protocol_minimum \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::old_protocol_rpc_stream_accepts_missing_protocol_minimum \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::old_protocol_append_stream_replicates_without_protocol_minimum \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::old_protocol_install_snapshot_stream_installs_without_protocol_minimum \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::old_protocol_proxy_request_stream_reaches_leader_without_protocol_minimum \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  broker_raft::tests::future_protocol_inbound_requests_do_not_mutate_raft_state \
  --lib -- --test-threads="$test_threads"

run_broker_raft_singletons \
  applied_request_id_without_cached_response_is_retryable \
  applied_request_id_without_response_survives_response_cache_trim \
  broker_raft_metrics_count_compaction_full_log_rewrite \
  broker_raft_metrics_count_failed_startup_full_log_scan \
  broker_raft_metrics_count_retained_byte_cache_repairs \
  broker_raft_metrics_count_startup_full_log_scan \
  cancelled_client_batch_entries_are_pruned_before_append \
  cannot_compact_past_latest_snapshot \
  client_request_queue_limit_prunes_cancelled_pending_before_rejecting \
  compact_to_latest_snapshot_noops_when_prefix_is_already_trimmed \
  conflicting_peer_repairs_suffix_without_resending_full_log \
  detached_fanout_yield_starts_followup_worker_when_idle \
  direct_client_admission_applies_durable_membership_before_enqueue \
  drop_client_applies_durable_membership_after_commit_lock_wait \
  effective_append_entries_max_bytes_respects_frame_cap \
  elected_leader_appends_and_commits_current_term_noop \
  ephemeral_rejects_blank_request_uuid_before_reserving_or_appending \
  final_membership_entry_commits_under_current_joint_quorum \
  finish_existing_install_snapshot_rejects_current_active_staged_learner_before_commit_advance \
  in_memory_peer_rpc_rejects_response_type_mismatch \
  inbound_raft_rpc_connection_cap_drops_excess_sockets \
  inbound_raft_rpc_idle_timeout_releases_connection_slot \
  inbound_raft_rpc_partial_frame_idle_timeout_releases_connection_slot \
  is_leader_uses_cached_role_when_runtime_lock_is_contended \
  lagging_peer_catches_up_over_bounded_append_batches \
  latest_snapshot_file_rejects_stable_metadata_drift \
  missing_leader_progress_starts_with_optimistic_tail_probe \
  never_promoted_learner_accepts_initial_append_entries \
  new_membership_peer_catchup_runs_learners_concurrently \
  no_target_append_entries_skips_busy_peer_before_frame_build \
  no_target_install_snapshot_skips_busy_peer_before_payload_prepare \
  no_term_conflict_above_stale_probe_repairs_to_compacted_follower_floor \
  no_wait_composite_acquire_grants_atomically_or_fails_fast_without_waiter \
  no_wait_ephemeral_acquire_does_not_queue_or_append_drop_client \
  oversized_append_entry_falls_back_to_install_snapshot \
  peer_proxy_local_leader_path_does_not_forward_after_leadership_loss \
  pending_request_id_survives_response_cache_trim \
  prebuilt_skip_busy_send_returns_without_counting_outbound_rpc \
  preselected_append_entries_frame_rejects_oversize_and_stale_lens \
  preselected_append_entries_frame_uses_cached_lens_for_exact_fit \
  promoted_voter_catchup_normalizes_low_next_index_to_known_match \
  promoted_voter_catchup_resets_untrusted_progress_above_local_tail \
  raft_client_id_sequence_resumes_after_reopen_above_log_tail \
  raft_client_ids_are_namespaced_by_node_and_do_not_collide_on_drop \
  raft_proxy_request_to_follower_is_not_reproxied \
  read_verified_snapshot_payload_file_does_not_follow_part_symlink \
  rejected_higher_term_append_entries_persists_term_before_reply \
  replication_batch_read_stops_before_unneeded_tail \
  request_id_reservation_is_released_when_queue_full_before_append \
  rewrite_log_does_not_follow_runtime_rewrite_tmp_symlink \
  same_term_step_down_preserves_persisted_vote \
  serialized_log_entries_buffer_and_lens_match_entry_json_lengths \
  shared_leader_gate_applies_durable_membership_before_internal_append \
  simple_membership_commit_thresholds_for_three_four_and_five \
  single_node_cluster_is_its_own_quorum_and_commits_without_peers \
  snapshotted_request_id_cache_survives_compacted_log_reopen \
  start_election_does_not_mutate_runtime_when_hard_state_write_fails \
  static_membership_requires_at_least_three_but_allows_even \
  target_append_batch_cached_progress_skips_frame_build_and_rpc \
  target_install_snapshot_cached_progress_skips_payload_prepare \
  target_install_snapshot_skips_busy_peer_before_payload_prepare \
  truncate_and_append_does_not_follow_main_log_symlink

check_broker_raft_quick_gate_unit_coverage

run cargo test --no-default-features \
  sim::tests::in_memory_seeded_chaos_preserves_lock_model_after_failover_and_restarts \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  sim::tests::in_memory_concurrent_partition_heal_preserves_single_key_linearizability \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  sim::tests::in_memory_partition_heal_no_wait_history_is_linearizable \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  sim::tests::in_memory_stale_leader_restart_no_wait_history_is_linearizable \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  sim::tests::in_memory_lagging_follower_catches_up_over_bounded_append_entries_without_snapshot \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  sim::tests::in_memory_lagging_follower_recovers_via_install_snapshot_after_compaction \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  sim::tests::in_memory_rolling_restart_no_wait_history_is_linearizable \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features \
  sim::tests::in_memory_shrunk_membership_failover_restarts_and_history_stay_incremental \
  --lib -- --test-threads="$test_threads"

run cargo test --no-default-features --test fuzz_fencing_multikey -- --test-threads="$test_threads"

run cargo test --no-default-features --test composite_liveness_fuzz -- --test-threads="$test_threads"

run cargo test --no-default-features --test integration composite -- --test-threads="$test_threads"

run cargo test --no-default-features --test stress_concurrency -- --test-threads="$test_threads"

run cargo test --no-default-features --test chaos_fuzz -- --test-threads="$test_threads"

run cargo test --no-default-features --test chaos_fuzz_extra -- --test-threads="$test_threads"

run cargo test --no-default-features --test raft_version_skew -- --test-threads="$test_threads"

run cargo test --no-default-features --test k8s_raft_live_smoke -- --test-threads="$test_threads"

if [ "$mode" = full ]; then
  run cargo test --no-default-features --lib -- --test-threads="$test_threads"
fi

run cargo test --no-default-features --test raft_cluster \
  raft_five_voter_cluster_commits_with_three_node_quorum \
  -- --test-threads="$test_threads"

run cargo test --no-default-features --test raft_cluster \
  raft_five_voter_minority_rejects_public_write_without_phantom_lock \
  -- --test-threads="$test_threads"

run cargo test --no-default-features --test raft_cluster \
  raft_follower_proxy_survives_leaderless_failover_window \
  -- --test-threads="$test_threads"

run cargo test --no-default-features --test raft_cluster \
  raft_membership_promotes_new_voters_and_survives_old_majority_loss \
  -- --test-threads="$test_threads"

run cargo test --no-default-features --test raft_cluster -- --test-threads="$test_threads"

if [ "$run_raft_version_skew" = true ]; then
  run cargo test --no-default-features --test raft_version_skew -- --ignored --nocapture --test-threads="$test_threads"
fi

if [ "$run_k8s_raft_live" = true ]; then
  run cargo test --no-default-features --test k8s_raft_live_smoke -- --ignored --nocapture --test-threads="$test_threads"
fi

if [ "$run_bench" = true ]; then
  if [ "$run_redis_bench" = true ]; then
    run env PROFILE="$profile" \
      PROFILE_TARGET=redis \
      REDIS_HOST="$redis_host" \
      REDIS_PORT="$redis_port" \
      BENCH_WORKERS="$bench_workers" \
      BENCH_KEYS="$bench_keys" \
      BENCH_DURATION_MS="$bench_duration_ms" \
      CAPTURE_METRICS=true \
      "$script_dir/profile-broker.sh" bench
  fi

  run env PROFILE="$profile" \
    PROFILE_TARGET=broker \
    BENCH_WORKERS="$bench_workers" \
    BENCH_KEYS="$bench_keys" \
    BENCH_DURATION_MS="$bench_duration_ms" \
    CAPTURE_METRICS=true \
    "$script_dir/profile-broker.sh" bench

  run env PROFILE="$profile" \
    PROFILE_TARGET=raft \
    RAFT_BENCH_ROUTE=leader \
    BENCH_WORKERS="$bench_workers" \
    BENCH_KEYS="$bench_keys" \
    BENCH_DURATION_MS="$bench_duration_ms" \
    CAPTURE_METRICS=true \
    BENCH_RAFT_METRICS=true \
    BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH="$bench_min_raft_batch_entries" \
    BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE="$bench_max_raft_commit_slot_writes" \
    BENCH_MAX_RAFT_LOG_APPEND_WRITE_US_PER_CYCLE="$bench_max_raft_log_append_write_us" \
    BENCH_MAX_RAFT_COMMIT_SLOT_WRITE_US_PER_CYCLE="$bench_max_raft_commit_slot_write_us" \
    "$script_dir/profile-broker.sh" bench

  run env PROFILE="$profile" \
    PROFILE_TARGET=raft \
    RAFT_BENCH_ROUTE=round-robin \
    BENCH_WORKERS="$bench_workers" \
    BENCH_KEYS="$bench_keys" \
    BENCH_DURATION_MS="$bench_duration_ms" \
    CAPTURE_METRICS=true \
    BENCH_RAFT_METRICS=true \
    BENCH_MIN_RAFT_CLIENT_BATCH_ENTRIES_PER_BATCH="$bench_min_raft_batch_entries" \
    BENCH_MAX_RAFT_COMMIT_SLOT_WRITES_PER_CYCLE="$bench_max_raft_commit_slot_writes" \
    BENCH_MAX_RAFT_LOG_APPEND_WRITE_US_PER_CYCLE="$bench_max_raft_log_append_write_us" \
    BENCH_MAX_RAFT_COMMIT_SLOT_WRITE_US_PER_CYCLE="$bench_max_raft_commit_slot_write_us" \
    "$script_dir/profile-broker.sh" bench
fi

run git diff --check
