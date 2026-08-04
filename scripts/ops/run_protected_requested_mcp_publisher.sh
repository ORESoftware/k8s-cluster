#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:?trusted k8s-cluster SHA required}"
child_script="${2:?unprivileged publisher script required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
case "$child_script" in
  /tmp/publish-requested-mcp-repositories.sh) ;;
  *) printf 'MCP_PUBLISHER_ERROR stage=initialization code=64\n'; exit 64 ;;
esac
test -f "$child_script"

# The caller supplies only an RSA-OAEP ciphertext envelope over stdin. It is
# safe to store in Git, but keeping it out of argv prevents accidental process
# listing or shell-history coupling.
envelope_line=""
IFS= read -r envelope_line || true

stage="protected-prerequisites"
work="$(mktemp -d /tmp/requested-mcp-broker.XXXXXX)"
key_root="/var/lib/oresoftware/requested-mcp-token-envelope"
private_key="$key_root/private.pem"
public_key="$key_root/public.pem"
cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN
  unset encoded_pat raw_pat secret_json credential_source ec2_home
  unset envelope_line envelope_fingerprint envelope_ciphertext actual_fingerprint
  rm -rf "$work"
}
on_error() {
  local status=$?
  trap - ERR
  printf 'MCP_PUBLISHER_ERROR stage=%s code=%d\n' "$stage" "$status"
  exit "$status"
}
trap cleanup EXIT
trap on_error ERR

valid_token() {
  test -n "${1:-}" &&
    [[ "$1" != *$'\n'* && "$1" != *$'\r'* &&
       "$1" != *$'\t'* && "$1" != *' '* ]]
}

consume_one_time_envelope() {
  stage="one-time-envelope-validation"
  command -v openssl >/dev/null 2>&1
  command -v base64 >/dev/null 2>&1
  command -v sha256sum >/dev/null 2>&1
  test -r "$private_key"
  test -r "$public_key"
  test "$(stat -c '%a' "$private_key")" = 600

  if [[ ! "$envelope_line" =~ ^MCP_TOKEN_ENVELOPE_V1[[:space:]]fingerprint=([0-9a-f]{64})[[:space:]]ciphertext=([A-Za-z0-9+/=]+)$ ]]; then
    printf 'MCP_PUBLISHER_ERROR stage=one-time-envelope-validation code=67\n'
    exit 67
  fi
  envelope_fingerprint="${BASH_REMATCH[1]}"
  envelope_ciphertext="${BASH_REMATCH[2]}"

  actual_fingerprint="$(
    openssl pkey -pubin -in "$public_key" -outform DER 2>/dev/null \
      | sha256sum \
      | awk '{print $1}'
  )"
  test "$actual_fingerprint" = "$envelope_fingerprint"

  printf '%s' "$envelope_ciphertext" | base64 --decode > "$work/token.enc"
  test "$(stat -c '%s' "$work/token.enc")" -eq 512

  stage="one-time-envelope-decryption"
  openssl pkeyutl \
    -decrypt \
    -inkey "$private_key" \
    -pkeyopt rsa_padding_mode:oaep \
    -pkeyopt rsa_oaep_md:sha256 \
    -pkeyopt rsa_mgf1_md:sha256 \
    -in "$work/token.enc" \
    -out "$work/token.bin" \
    >/dev/null 2>&1
  raw_pat="$(cat "$work/token.bin")"
  rm -f "$work/token.enc" "$work/token.bin"
  valid_token "$raw_pat"

  GH_TOKEN="$raw_pat"
  credential_source="one-time-rsa-envelope"
  unset raw_pat envelope_line envelope_ciphertext

  # Consume the private key exactly once, even if the downstream GitHub
  # preflight later rejects the token. A retry requires a fresh reviewed key
  # and ciphertext pair rather than silently reusing credential material.
  rm -f "$private_key" "$public_key"
  rmdir "$key_root" 2>/dev/null || true
}

stage="protected-credential"
credential_source=""
GH_TOKEN=""

# Prefer the protected EC2 instance-role path already used by established
# repository publishers on this host.
if command -v aws >/dev/null 2>&1; then
  secret_json="$(
    aws secretsmanager get-secret-value \
      --region "${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}" \
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
      credential_source="aws-secrets-manager"
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
      credential_source="kubernetes-secret"
      break
    fi
  done
fi
unset raw_pat encoded_pat

# Last protected persistent fallback: an authenticated ORESoftware gh profile
# owned by the unprivileged publication account.
if test -z "$GH_TOKEN" && command -v sudo >/dev/null 2>&1 && command -v getent >/dev/null 2>&1; then
  ec2_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
  case "$ec2_home" in
    /*)
      raw_pat="$(
        sudo -u ec2-user -H \
          env -u GH_TOKEN -u GITHUB_TOKEN -u GITHUB_REPOSITORY_ADMIN_TOKEN \
          HOME="$ec2_home" XDG_CONFIG_HOME="$ec2_home/.config" \
          bash -c 'command -v gh >/dev/null 2>&1 && gh auth token --hostname github.com' \
          2>/dev/null || true
      )"
      ;;
    *) raw_pat="" ;;
  esac
  if valid_token "$raw_pat"; then
    GH_TOKEN="$raw_pat"
    credential_source="protected-gh-profile"
  fi
fi
unset raw_pat

# If no durable protected credential exists, consume the reviewed one-time
# envelope generated for this publication only.
if ! valid_token "$GH_TOKEN"; then
  consume_one_time_envelope
fi

if ! valid_token "$GH_TOKEN"; then
  printf 'MCP_PUBLISHER_ERROR stage=protected-credential code=65\n'
  exit 65
fi
printf 'MCP_PUBLISHER_STAGE stage=protected-credential source=%s\n' "$credential_source"

stage="unprivileged-publication-boundary"
ec2_home="$(getent passwd ec2-user | awk -F: '$1 == "ec2-user" { print $6 }')"
case "$ec2_home" in
  /*) ;;
  *) printf 'MCP_PUBLISHER_ERROR stage=unprivileged-publication-boundary code=66\n'; exit 66 ;;
esac
chown ec2-user:ec2-user "$child_script"
chmod 700 "$child_script"
encoded_pat="$(printf '%s' "$GH_TOKEN" | base64 --wrap=0)"
unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN

printf '%s\n' "$encoded_pat" |
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

stage="complete"
printf 'MCP_PUBLISHER_STAGE stage=complete status=success\n'
