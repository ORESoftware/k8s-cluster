#!/usr/bin/env bash
set -Eeuo pipefail
control_api="https://api.github.com/repos/${CONTROL_REPOSITORY}"

control_call() {
  local method="$1" url="$2" data="${3:-}"
  local args=(
    --fail-with-body --silent --show-error
    --request "$method"
    --url "$url"
    --header 'Accept: application/vnd.github+json'
    --header "Authorization: Bearer ${CONTROL_TOKEN}"
    --header 'X-GitHub-Api-Version: 2022-11-28'
  )
  if [[ -n "$data" ]]; then
    args+=(--data-binary "$data")
  fi
  curl "${args[@]}"
}

put_control_file() {
  local path="$1" source="$2" message="$3"
  local encoded payload metadata sha
  encoded="$(base64 -w0 "$source")"
  payload="$(jq -nc --arg message "$message" --arg content "$encoded" --arg branch "$CONTROL_BRANCH" \
    '{message:$message,content:$content,branch:$branch}')"
  if metadata="$(control_call GET "${control_api}/contents/${path}?ref=${CONTROL_BRANCH}" 2>/dev/null)"; then
    sha="$(jq -r '.sha // empty' <<<"$metadata")"
    if [[ -n "$sha" ]]; then
      payload="$(jq -nc --arg message "$message" --arg content "$encoded" --arg branch "$CONTROL_BRANCH" --arg sha "$sha" \
        '{message:$message,content:$content,branch:$branch,sha:$sha}')"
    fi
  fi
  control_call PUT "${control_api}/contents/${path}" "$payload" >/dev/null
}

delete_control_file() {
  local path="$1" message="$2"
  local metadata sha payload
  metadata="$(control_call GET "${control_api}/contents/${path}?ref=${CONTROL_BRANCH}")"
  sha="$(jq -r '.sha // empty' <<<"$metadata")"
  test -n "$sha"
  payload="$(jq -nc --arg message "$message" --arg sha "$sha" --arg branch "$CONTROL_BRANCH" \
    '{message:$message,sha:$sha,branch:$branch}')"
  control_call DELETE "${control_api}/contents/${path}" "$payload" >/dev/null
}

put_control_file "$EVIDENCE_PATH" /tmp/den-2797-wave7-evidence.json \
  'docs(DEN-2797): record Wave 7 repository publication evidence'
delete_control_file "$WORKFLOW_PATH" \
  'ops(DEN-2797): remove completed one-shot Wave 7 publisher'
echo "WAVE7_PUBLICATION_COMPLETE evidence=${EVIDENCE_PATH}"
