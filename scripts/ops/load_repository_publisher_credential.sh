#!/usr/bin/env bash
set -euo pipefail

: "${GITHUB_ENV:?GITHUB_ENV is required when loading the protected repository publisher}"

# Prefer the repository-scoped Actions secret. This avoids coupling repository
# publication to an unrelated AWS maintenance role. Keep Secrets Manager as a
# recovery path for installations where the Actions secret is intentionally absent.
token="${REMOTE_DEV_GH_PAT:-${LEGACY_GH_PAT:-}}"
source_name='GitHub Actions secret'

if [[ -z "${token//[[:space:]]/}" ]]; then
  source_name='AWS Secrets Manager fallback'
  secret_json="$(aws secretsmanager get-secret-value \
    --secret-id dd/remote-dev/agent-secrets \
    --query SecretString \
    --output text)"
  token="$(jq -er '.GH_PAT | select(type == "string" and length > 0)' <<<"$secret_json")"
  unset secret_json
fi

if [[ -z "$token" || "$token" == *$'\n'* || "$token" == *$'\r'* ]]; then
  echo 'protected repository publisher is empty or contains an invalid newline' >&2
  exit 1
fi

# Mask before adding the credential to the workflow environment. The next step
# independently verifies the exact user and organization-admin boundary before
# any repository mutation is permitted.
printf '::add-mask::%s\n' "$token"
{
  printf 'GH_TOKEN=%s\n' "$token"
  if [[ "${EXPORT_REPOSITORY_ADMIN_TOKEN:-false}" == true ]]; then
    printf 'GITHUB_REPOSITORY_ADMIN_TOKEN=%s\n' "$token"
  fi
} >> "$GITHUB_ENV"

unset token
echo "protected repository publisher loaded from ${source_name}"
