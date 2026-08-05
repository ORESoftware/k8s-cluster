#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

stage=direct-bootstrap
fail() {
  local reason="${1:?failure reason required}"
  local status="${2:-64}"
  printf 'publisher-stage=%s status=failed reason=%s rc=%s\n' \
    "$stage" "$reason" "$status" >&2
  exit "$status"
}

trusted_sha="${1:-}"
source_root="${2:-}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]] || fail invalid-trusted-sha 64
[[ "$source_root" == /* ]] || fail source-root-not-absolute 64
[[ -d "$source_root" ]] || fail source-root-missing 66

raw_region="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
command -v tr >/dev/null 2>&1 || fail tr-unavailable 69
publisher_region="$(printf '%s' "$raw_region" | tr -d '[:space:]')"
unset raw_region
[[ "$publisher_region" =~ ^[a-z]{2}(-gov)?-[a-z0-9-]+-[0-9]$ ]] || \
  fail invalid-aws-region 64
export AWS_REGION="$publisher_region"
export AWS_DEFAULT_REGION="$publisher_region"
unset publisher_region

protected_runner="$source_root/scripts/ops/run_protected_org_dotgithub_all_publisher.sh"
all_publisher="$source_root/scripts/ops/bootstrap_org_dotgithub_repositories_all.py"
[[ -f "$protected_runner" ]] || fail protected-runner-missing 66
[[ -f "$all_publisher" ]] || fail all-publisher-missing 66
command -v python3 >/dev/null 2>&1 || fail python3-unavailable 69
bash -n "$protected_runner" || fail protected-runner-invalid 65
python3 -m py_compile "$all_publisher" || fail all-publisher-invalid 65
python3 -m unittest discover \
  -s "$source_root/tests/ops" \
  -p 'test_bootstrap_org_dotgithub_repositories_all.py' \
  -v || fail all-publisher-tests-failed 65
printf 'publisher-stage=%s status=passed\n' "$stage" >&2

stage=direct-delegate
set +e
bash "$protected_runner" "$trusted_sha" "$source_root"
status=$?
set -e
if [[ "$status" -ne 0 ]]; then
  printf 'publisher-stage=%s status=failed rc=%s\n' "$stage" "$status" >&2
  exit "$status"
fi
printf 'publisher-stage=%s status=passed\n' "$stage" >&2
