#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

source_script='scripts/ops/bootstrap_des_browser_test_fleet_session.sh'
[[ -r "$source_script" ]] || {
  echo "Missing canonical session bootstrap: $source_script" >&2
  exit 69
}

work="$(mktemp -d "${RUNNER_TEMP:-/tmp}/des-browser-session-wrapper.XXXXXX")"
patched_script="$work/bootstrap_des_browser_test_fleet_session.sh"
cleanup() {
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

python3 - "$source_script" "$patched_script" <<'PY'
from __future__ import annotations

import pathlib
import sys

source = pathlib.Path(sys.argv[1])
destination = pathlib.Path(sys.argv[2])
text = source.read_text(encoding="utf-8")

insert_before = "\ndelete_comment() {\n"
if text.count(insert_before) != 1:
    raise SystemExit("canonical bootstrap no longer has the expected delete_comment boundary")

public_reader = r'''

public_comments_request() {
  local output="$1"
  local endpoint="$2"
  local attempt status
  for attempt in $(seq 1 12); do
    status="$(
      curl \
        --silent \
        --show-error \
        --request GET \
        --header 'Accept: application/vnd.github+json' \
        --header "X-GitHub-Api-Version: ${API_VERSION}" \
        --output "$output" \
        --write-out '%{http_code}' \
        "https://api.github.com/repos/${GITHUB_REPOSITORY}${endpoint}" \
        || true
    )"
    if [[ "$status" == '200' ]]; then
      return 0
    fi
    if [[ "$status" == '403' || "$status" == '429' ]]; then
      sleep 5
      continue
    fi
    echo "Public issue-comment read failed with HTTP ${status}." >&2
    cat "$output" >&2 2>/dev/null || true
    return 1
  done
  echo 'Public issue-comment read remained rate-limited.' >&2
  return 1
}
'''
text = text.replace(insert_before, public_reader + insert_before)

old_read = '  api_request GET "/issues/${TRACKING_ISSUE}/comments?per_page=100" > "$work/comments.json"\n'
new_read = '  public_comments_request "$work/comments.json" "/issues/${TRACKING_ISSUE}/comments?per_page=100"\n'
if text.count(old_read) != 1:
    raise SystemExit("canonical bootstrap no longer has the expected authenticated polling call")
text = text.replace(old_read, new_read)
text = text.replace('  sleep 3\ndone\n', '  sleep 5\ndone\n', 1)

destination.write_text(text, encoding="utf-8")
destination.chmod(0o700)
PY

exec bash "$patched_script"
