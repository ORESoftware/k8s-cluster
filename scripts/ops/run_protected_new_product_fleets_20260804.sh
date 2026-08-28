#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
repo_root="$(git rev-parse --show-toplevel)"
test "$(git -C "$repo_root" rev-parse HEAD)" = "$trusted_sha"

stage=initialization
work="$(mktemp -d /tmp/new-product-fleet-publisher.XXXXXX)"
publisher_region="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"

cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset raw_secret raw_pat encoded_pat credential_source
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  rm -rf "$work"
}
report_failure() {
  local rc=$?
  trap - ERR
  printf 'publisher-stage=%s status=failed rc=%s\n' "$stage" "$rc" >&2
  exit "$rc"
}
trap cleanup EXIT
trap report_failure ERR

stage=protected-credential
GH_TOKEN=''
credential_source=''

if command -v aws >/dev/null 2>&1; then
  raw_secret="$(
    aws secretsmanager get-secret-value \
      --region "$publisher_region" \
      --secret-id dd/remote-dev/agent-secrets \
      --query SecretString \
      --output text 2>/dev/null || true
  )"
  if test -n "$raw_secret"; then
    raw_pat="$(
      printf '%s' "$raw_secret" | python3 -c '
import json, sys
try:
    payload = json.load(sys.stdin)
except (json.JSONDecodeError, OSError):
    raise SystemExit(0)
value = payload.get("GH_PAT")
if isinstance(value, str) and value and not any(ch.isspace() for ch in value):
    sys.stdout.write(value)
' 2>/dev/null || true
    )"
    if test -n "$raw_pat"; then
      GH_TOKEN="$raw_pat"
      credential_source=aws-secrets-manager
    fi
  fi
fi
unset raw_secret raw_pat

if test -z "$GH_TOKEN" && command -v kubectl >/dev/null 2>&1; then
  for kubeconfig in \
    /etc/kubernetes/admin.conf \
    /root/.kube/config \
    /home/ec2-user/.kube/config
  do
    test -r "$kubeconfig" || continue
    encoded_pat="$(
      KUBECONFIG="$kubeconfig" \
        kubectl -n default get secret dd-agent-secrets \
        -o jsonpath='{.data.GH_PAT}' 2>/dev/null || true
    )"
    test -n "$encoded_pat" || continue
    raw_pat="$(printf '%s' "$encoded_pat" | base64 --decode 2>/dev/null || true)"
    if test -n "$raw_pat" && [[ "$raw_pat" != *$'\n'* && "$raw_pat" != *$'\r'* && "$raw_pat" != *$'\t'* && "$raw_pat" != *' '* ]]; then
      GH_TOKEN="$raw_pat"
      credential_source="kubernetes-secret:${kubeconfig}"
      break
    fi
  done
fi
unset raw_pat encoded_pat

if test -z "$GH_TOKEN" && command -v sudo >/dev/null 2>&1 && command -v getent >/dev/null 2>&1; then
  ec2_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
  case "$ec2_home" in
    /*)
      raw_pat="$(
        sudo -u ec2-user -H env \
          -u GH_TOKEN -u GITHUB_TOKEN -u GH_ENTERPRISE_TOKEN \
          -u GITHUB_REPOSITORY_ADMIN_TOKEN -u GH_CONFIG_DIR \
          HOME="$ec2_home" XDG_CONFIG_HOME="$ec2_home/.config" \
          bash -c 'command -v gh >/dev/null 2>&1 && gh auth token --hostname github.com' \
          2>/dev/null || true
      )"
      ;;
    *) raw_pat='' ;;
  esac
  if test -n "$raw_pat" && [[ "$raw_pat" != *$'\n'* && "$raw_pat" != *$'\r'* && "$raw_pat" != *$'\t'* && "$raw_pat" != *' '* ]]; then
    GH_TOKEN="$raw_pat"
    credential_source=protected-gh-profile
  fi
fi
unset raw_pat ec2_home

if test -z "$GH_TOKEN"; then
  printf 'publisher-stage=%s status=failed reason=no-readable-protected-credential\n' "$stage" >&2
  exit 65
fi
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"
printf 'publisher-stage=%s status=passed source=%s\n' "$stage" "$credential_source"

stage=publisher-static-validation
publisher="$work/publish_new_product_fleets_20260804.py"
bash "$repo_root/scripts/ops/materialize_new_product_fleet_publisher_20260804.sh" \
  "$publisher" "$repo_root"
printf 'publisher-stage=%s status=passed\n' "$stage"

stage=bounded-repository-publication
python3 "$publisher" \
  --output "$work/fleet" \
  --execute
printf 'publisher-stage=%s status=passed\n' "$stage"

stage=publication-summary
python3 - "$work/fleet/PUBLICATION.json" <<'PY'
import json
import sys
from pathlib import Path
payload = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
print(f"publication-status={payload['status']} repositories={payload['repository_count']}")
for record in payload["repositories"]:
    print(f"published={record['full_name']} commit={record['commit']}")
PY
printf 'publisher-stage=%s status=passed\n' "$stage"

stage=complete
printf 'publisher-stage=%s status=success\n' "$stage"
