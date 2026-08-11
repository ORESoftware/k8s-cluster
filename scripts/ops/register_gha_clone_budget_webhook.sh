#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly script_name="${0##*/}"
repository=''
webhook_url='https://98.90.186.114/gha-webhooks/github'
secret_file=''

usage() {
  cat <<'EOF'
Usage:
  register_gha_clone_budget_webhook.sh \
    --repository OWNER/REPO \
    --secret-file PATH \
    [--url HTTPS_URL]

Creates or updates one repository webhook for workflow_run events only. The
webhook secret is read from a file and is never printed. Authentication is
provided to the GitHub CLI through GH_TOKEN or an existing `gh auth login`.

Required token/App authority: repository Administration read/write.
EOF
}

while (($#)); do
  case "$1" in
    --repository)
      [[ $# -ge 2 ]] || { echo "$script_name: --repository requires a value" >&2; exit 64; }
      repository="$2"
      shift 2
      ;;
    --url)
      [[ $# -ge 2 ]] || { echo "$script_name: --url requires a value" >&2; exit 64; }
      webhook_url="$2"
      shift 2
      ;;
    --secret-file)
      [[ $# -ge 2 ]] || { echo "$script_name: --secret-file requires a value" >&2; exit 64; }
      secret_file="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "$script_name: unknown argument: $1" >&2
      usage >&2
      exit 64
      ;;
  esac
done

command -v gh >/dev/null 2>&1 || { echo "$script_name: gh is required" >&2; exit 69; }
command -v jq >/dev/null 2>&1 || { echo "$script_name: jq is required" >&2; exit 69; }

[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || {
  echo "$script_name: --repository must be exact OWNER/REPO" >&2
  exit 64
}
[[ "$webhook_url" =~ ^https://[^[:space:]]+/gha-webhooks/github$ ]] || {
  echo "$script_name: --url must be HTTPS and end in /gha-webhooks/github" >&2
  exit 64
}
[[ -n "$secret_file" && -f "$secret_file" && -r "$secret_file" ]] || {
  echo "$script_name: --secret-file must name a readable file" >&2
  exit 66
}

secret_bytes="$(wc -c <"$secret_file" | tr -d '[:space:]')"
[[ "$secret_bytes" =~ ^[0-9]+$ && "$secret_bytes" -ge 32 ]] || {
  echo "$script_name: webhook secret must contain at least 32 bytes" >&2
  exit 65
}

# Resolve by exact URL so reruns update instead of duplicating deliveries.
existing_id="$(
  gh api "/repos/${repository}/hooks?per_page=100" --paginate \
    | jq -r --arg url "$webhook_url" '.[] | select(.config.url == $url) | .id' \
    | head -n 1
)"

# --rawfile keeps the HMAC secret out of argv/process listings. Strip only a
# trailing file newline; internal bytes remain unchanged.
payload="$(jq -cn \
  --arg url "$webhook_url" \
  --rawfile secret "$secret_file" \
  '{
    name: "web",
    active: true,
    events: ["workflow_run"],
    config: {
      url: $url,
      content_type: "json",
      secret: ($secret | sub("[\r\n]+$"; "")),
      insecure_ssl: "0"
    }
  }')"

if [[ -n "$existing_id" ]]; then
  result="$(printf '%s' "$payload" | gh api --method PATCH "/repos/${repository}/hooks/${existing_id}" --input -)"
  action='updated'
else
  result="$(printf '%s' "$payload" | gh api --method POST "/repos/${repository}/hooks" --input -)"
  action='created'
fi

jq -e --arg url "$webhook_url" '
  .active == true and
  .config.url == $url and
  .config.content_type == "json" and
  .config.insecure_ssl == "0" and
  .events == ["workflow_run"]
' <<<"$result" >/dev/null || {
  echo "$script_name: GitHub returned a webhook that does not match the requested contract" >&2
  exit 1
}

hook_id="$(jq -r '.id' <<<"$result")"
printf '%s webhook id=%s repository=%s events=workflow_run url=%s\n' \
  "$action" "$hook_id" "$repository" "$webhook_url"
