#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
usage: register-github-webhook.sh (--repo owner/repo | --org organization) --url https://host/webhooks/github

Required environment:
  GH_TOKEN                  token or GitHub App installation token with hook admin permission
  GITHUB_WEBHOOK_SECRET     HMAC secret already stored in the cluster secret manager
USAGE
}

command -v gh >/dev/null 2>&1 || { echo 'gh is required' >&2; exit 2; }
command -v jq >/dev/null 2>&1 || { echo 'jq is required' >&2; exit 2; }

scope=''
target=''
webhook_url=''
while (($#)); do
  case "$1" in
    --repo|--org)
      test -z "$scope" || { echo 'choose exactly one of --repo or --org' >&2; exit 2; }
      scope="${1#--}"
      target="${2:-}"
      shift 2
      ;;
    --url)
      webhook_url="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

test -n "${GH_TOKEN:-}" || { echo 'GH_TOKEN is required' >&2; exit 2; }
test -n "${GITHUB_WEBHOOK_SECRET:-}" || { echo 'GITHUB_WEBHOOK_SECRET is required' >&2; exit 2; }
test -n "$scope" && test -n "$target" && test -n "$webhook_url" || { usage; exit 2; }
case "$webhook_url" in
  https://*) ;;
  *) echo 'webhook URL must use HTTPS' >&2; exit 2 ;;
esac

if [[ "$scope" == repo ]]; then
  [[ "$target" == */* ]] || { echo '--repo requires owner/repo' >&2; exit 2; }
  endpoint="repos/${target}/hooks"
else
  [[ "$target" != */* ]] || { echo '--org requires an organization login' >&2; exit 2; }
  endpoint="orgs/${target}/hooks"
fi

existing_id="$(
  gh api --paginate "$endpoint" |
    jq -r --arg url "$webhook_url" '.[] | select(.config.url == $url) | .id' |
    head -n 1
)"
if [[ -n "$existing_id" ]]; then
  method='PATCH'
  hook_endpoint="${endpoint}/${existing_id}"
  action='updated'
else
  method='POST'
  hook_endpoint="$endpoint"
  action='registered'
fi

payload="$(
  jq -n \
    --arg url "$webhook_url" \
    --arg secret "$GITHUB_WEBHOOK_SECRET" \
    '{
      name: "web",
      active: true,
      events: ["workflow_run"],
      config: {
        url: $url,
        content_type: "json",
        secret: $secret,
        insecure_ssl: "0"
      }
    }'
)"
printf '%s' "$payload" | gh api --method "$method" "$hook_endpoint" --input - >/dev/null
printf '%s failure webhook for %s %s\n' "$action" "$scope" "$target"
