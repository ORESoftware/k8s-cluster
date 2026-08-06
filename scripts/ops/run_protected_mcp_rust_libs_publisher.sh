#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
child_script="${2:?unprivileged publisher script required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
case "$child_script" in
  /tmp/publish-mcp-rust-libs.sh) ;;
  *) echo 'publisher-stage=initialization status=failed reason=unexpected-child-path' >&2; exit 64 ;;
esac
test -f "$child_script"

stage=initialization
work="$(mktemp -d /tmp/mcp-rust-libs-broker.XXXXXX)"
cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset encoded_pat raw_pat secret_json credential_source
  unset ec2_home
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

valid_token() {
  test -n "${1:-}" && \
    [[ "$1" != *$'\n'* && "$1" != *$'\r'* && \
       "$1" != *$'\t'* && "$1" != *' '* ]]
}

stage=protected-credential
credential_source=''
GH_TOKEN=''

# Prefer the EC2 instance-role path used by the repository's established
# missing-organization publisher and External Secrets deployment.
if command -v aws >/dev/null 2>&1; then
  secret_json="$(
    aws secretsmanager get-secret-value \
      --region us-east-1 \
      --secret-id dd/remote-dev/agent-secrets \
      --query SecretString \
      --output text 2>/dev/null || true
  )"
  if test -n "$secret_json"; then
    raw_pat="$(
      printf '%s' "$secret_json" | python3 -c '
import json
import sys
try:
    payload = json.load(sys.stdin)
except (json.JSONDecodeError, OSError):
    raise SystemExit(0)
value = payload.get("GH_PAT")
if isinstance(value, str) and value:
    sys.stdout.write(value)
' 2>/dev/null || true
    )"
    if valid_token "$raw_pat"; then
      GH_TOKEN="$raw_pat"
      credential_source=aws-secrets-manager
    fi
  fi
fi
unset raw_pat secret_json

# Fall back to the reconciled Kubernetes Secret across the protected host's
# known kubeconfig locations.
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
    unset encoded_pat
    if valid_token "$raw_pat"; then
      GH_TOKEN="$raw_pat"
      credential_source="kubernetes-secret:${kubeconfig}"
      break
    fi
  done
fi
unset raw_pat encoded_pat

# Last protected fallback: use the authenticated ORESoftware gh profile through
# the CLI abstraction, allowing either file-backed or keyring-backed storage.
if test -z "$GH_TOKEN" && command -v sudo >/dev/null 2>&1; then
  raw_pat="$(
    sudo -u ec2-user -H \
      env -u GH_TOKEN -u GITHUB_TOKEN -u GITHUB_REPOSITORY_ADMIN_TOKEN \
      bash -lc 'command -v gh >/dev/null 2>&1 && gh auth token --hostname github.com' \
      2>/dev/null || true
  )"
  if valid_token "$raw_pat"; then
    GH_TOKEN="$raw_pat"
    credential_source=protected-gh-profile
  fi
fi
unset raw_pat

if ! valid_token "$GH_TOKEN"; then
  echo 'publisher-stage=protected-credential status=failed reason=no-readable-protected-credential' >&2
  exit 65
fi
printf 'publisher-stage=%s status=passed source=%s\n' "$stage" "$credential_source"

stage=unprivileged-publication-boundary
ec2_home="$(getent passwd ec2-user | awk -F: '$1=="ec2-user"{print $6}')"
case "$ec2_home" in
  /*) ;;
  *) echo 'publisher-stage=unprivileged-publication-boundary status=failed reason=invalid-ec2-home' >&2; exit 66 ;;
esac
chown ec2-user:ec2-user "$child_script"
chmod 700 "$child_script"
encoded_pat="$(printf '%s' "$GH_TOKEN" | base64 --wrap=0)"
unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN

printf '%s\n' "$encoded_pat" | \
  sudo -u ec2-user -H \
    env \
      -u CODEX_HOME \
      -u GH_TOKEN \
      -u GITHUB_TOKEN \
      -u GH_ENTERPRISE_TOKEN \
      -u GITHUB_REPOSITORY_ADMIN_TOKEN \
      -u GIT_ASKPASS \
      -u GIT_ASKPASS_REQUIRE \
      HOME="$ec2_home" \
      XDG_CONFIG_HOME="$ec2_home/.config" \
      bash -c 'exec bash "$1" "$2"' _ "$child_script" "$trusted_sha"
unset encoded_pat ec2_home

stage=complete
printf 'publisher-stage=%s status=success\n' "$stage"
