#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

readonly script_name="${0##*/}"
repository=''
webhook_url='https://98.90.186.114/gha-webhooks/github'
secret_file=''
temp_dir=''

cleanup() {
  if [[ -n "$temp_dir" && -d "$temp_dir" ]]; then
    rm -rf -- "$temp_dir"
  fi
}
trap cleanup EXIT HUP INT TERM

usage() {
  cat <<'USAGE'
Usage:
  register_gha_clone_budget_webhook.sh \
    --repository OWNER/REPO \
    --secret-file PATH \
    [--url HTTPS_URL]

Creates or updates exactly one repository webhook for workflow_run events.
The webhook secret is read from an owner-only regular file and is never accepted
from an environment variable, printed, or placed in a process argument.
Authentication is provided through GH_TOKEN or an existing `gh auth login`.

Required token/App authority: repository Administration read/write.
USAGE
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
command -v python3 >/dev/null 2>&1 || { echo "$script_name: python3 is required" >&2; exit 69; }

[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || {
  echo "$script_name: --repository must be exact OWNER/REPO" >&2
  exit 64
}
[[ -n "$secret_file" && -f "$secret_file" && -r "$secret_file" && ! -L "$secret_file" ]] || {
  echo "$script_name: --secret-file must name a readable, non-symlink regular file" >&2
  exit 66
}

python3 - "$webhook_url" <<'PY'
from __future__ import annotations

import sys
from urllib.parse import urlsplit

raw = sys.argv[1]
parsed = urlsplit(raw)
if (
    parsed.scheme != "https"
    or not parsed.hostname
    or parsed.username is not None
    or parsed.password is not None
    or parsed.query
    or parsed.fragment
    or parsed.path != "/gha-webhooks/github"
):
    raise SystemExit("webhook URL must be an exact credential-free HTTPS /gha-webhooks/github URL")
PY

temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/${script_name}.XXXXXX")"
normalized_secret_file="$temp_dir/webhook-secret"
payload_file="$temp_dir/hook.json"

# Open without following a final symlink, verify the descriptor is still an
# owner-only regular file, bound the read before allocation, normalize one
# optional terminal line ending, and validate the exact HMAC value GitHub gets.
python3 - "$secret_file" "$normalized_secret_file" <<'PY'
from __future__ import annotations

import os
import stat
import sys

source = sys.argv[1]
destination = sys.argv[2]
flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
try:
    descriptor = os.open(source, flags)
except OSError as exc:
    raise SystemExit(f"cannot securely open webhook secret: {exc}") from exc
try:
    metadata = os.fstat(descriptor)
    if not stat.S_ISREG(metadata.st_mode):
        raise SystemExit("webhook secret must remain a regular file after open")
    mode = stat.S_IMODE(metadata.st_mode)
    if not mode & stat.S_IRUSR:
        raise SystemExit("webhook secret must be owner-readable")
    if mode & 0o077:
        raise SystemExit("webhook secret must not be group/world accessible")
    with os.fdopen(descriptor, "rb", closefd=False) as handle:
        value = handle.read(4099)
finally:
    os.close(descriptor)

if len(value) > 4098:
    raise SystemExit("webhook secret file exceeds the bounded input size")
if value.endswith(b"\r\n"):
    value = value[:-2]
elif value.endswith(b"\n"):
    value = value[:-1]
if not 32 <= len(value) <= 4096:
    raise SystemExit("webhook secret must contain 32 to 4096 bytes after terminal-newline normalization")
if any(byte < 0x21 or byte > 0x7E for byte in value):
    raise SystemExit("webhook secret must be a single visible-ASCII line")

output_flags = (
    os.O_WRONLY
    | os.O_CREAT
    | os.O_EXCL
    | getattr(os, "O_CLOEXEC", 0)
    | getattr(os, "O_NOFOLLOW", 0)
)
output = os.open(destination, output_flags, 0o600)
with os.fdopen(output, "wb") as handle:
    handle.write(value)
PY

# Resolve by exact URL so reruns update rather than duplicate deliveries. More
# than one match is an unsafe pre-existing state: fail closed instead of silently
# selecting one hook and leaving the others active.
existing_ids="$(
  gh api "/repos/${repository}/hooks?per_page=100" --paginate \
    | jq -r --arg url "$webhook_url" '.[] | select(.config.url == $url) | .id'
)"
match_count="$(printf '%s\n' "$existing_ids" | awk 'NF { count += 1 } END { print count + 0 }')"
if ((match_count > 1)); then
  echo "$script_name: multiple hooks already use the exact callback URL; refusing an ambiguous update" >&2
  exit 65
fi
existing_id="$(printf '%s\n' "$existing_ids" | awk 'NF { print; exit }')"

jq -cn \
  --arg url "$webhook_url" \
  --rawfile secret "$normalized_secret_file" \
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
  }' >"$payload_file"

if [[ -n "$existing_id" ]]; then
  [[ "$existing_id" =~ ^[0-9]+$ ]] || { echo "$script_name: GitHub returned a non-numeric hook id" >&2; exit 1; }
  result="$(gh api --method PATCH "/repos/${repository}/hooks/${existing_id}" --input "$payload_file")"
  action='updated'
else
  result="$(gh api --method POST "/repos/${repository}/hooks" --input "$payload_file")"
  action='created'
fi

jq -e --arg url "$webhook_url" '
  (.id | type == "number") and
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
