#!/usr/bin/env bash
# Ensure every GitHub organization owned by ORESoftware has an active
# Projects v2 board named <canonical-org-login>-project.
set -uo pipefail

: "${GH_TOKEN:?GH_TOKEN is required}"
JSON_OUTPUT="${1:?JSON output path is required}"
MARKDOWN_OUTPUT="${2:?Markdown output path is required}"

KNOWN_ORGS=(
  3FA-app OmniBlitz StreemPilot agent-pontifex akrion-sim anticaptrad athlet-o
  benefactor-cc canonical-cloud channelsiege claritas-viz cliptown daedalus-fab
  dancing-dragons declarative-migrations discrete-event-systems drone-mngr
  fanwaave fiducia-cloud fifa-math file-tunnel gha-indie-worker hypeblitz
  hypesiege memebank messaging-intel meta-agents-demo networking-components
  opto-sync quaestor-ledger rust-ssr-demos sagitta-stack scintilla-run
  shared-auth sonus-auris streamkore unreal-unity-poc usa-acc voxletra
  zed-pkg zed-pkg-test
)

mkdir -p "$(dirname "$JSON_OUTPUT")" "$(dirname "$MARKDOWN_OUTPUT")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
records="$work/records.jsonl"
: >"$records"

safe_error() {
  tr '\n\r\t' '   ' <"$1" | sed 's/[[:space:]][[:space:]]*/ /g' | cut -c1-700
}

record_failure() {
  local requested="$1" org="$2" action="$3" message="$4"
  jq -nc \
    --arg requested_org "$requested" \
    --arg org "$org" \
    --arg action "$action" \
    --arg error "$message" \
    '{requested_org:$requested_org,org:$org,action:$action,error:$error}' >>"$records"
}

authenticated="$(gh api user --jq .login)"
if [[ "$authenticated" != ORESoftware ]]; then
  echo "Refusing to mutate organizations as unexpected login: $authenticated" >&2
  exit 2
fi

printf '%s\n' "${KNOWN_ORGS[@]}" >"$work/known"
if ! gh api --paginate /user/orgs --jq '.[].login' >"$work/visible" 2>"$work/error"; then
  echo "Unable to enumerate authenticated organization memberships: $(safe_error "$work/error")" >&2
  exit 2
fi
cat "$work/known" "$work/visible" | awk 'NF' | sort -fu >"$work/candidates"

total="$(wc -l <"$work/candidates" | tr -d ' ')"
index=0
while IFS= read -r requested; do
  [[ -n "$requested" ]] || continue
  index=$((index + 1))
  echo "[$index/$total] ensuring $requested"

  if ! membership="$(gh api "user/memberships/orgs/$requested" 2>"$work/error")"; then
    message="$(safe_error "$work/error")"
    record_failure "$requested" "$requested" failed_membership "$message"
    echo "  -> failed_membership"
    continue
  fi

  canonical="$(jq -r '.organization.login // empty' <<<"$membership")"
  role="$(jq -r '.role // empty' <<<"$membership")"
  state="$(jq -r '.state // empty' <<<"$membership")"
  [[ -n "$canonical" ]] || canonical="$requested"

  if [[ "$role" != admin || "$state" != active ]]; then
    record_failure \
      "$requested" "$canonical" skipped_not_owner \
      "membership is ${role:-unknown}/${state:-unknown}, not active organization owner"
    echo "  -> skipped_not_owner"
    continue
  fi

  title="${canonical}-project"
  if ! before="$(gh project list --owner "$canonical" --closed --limit 100 --format json 2>"$work/error")"; then
    message="$(safe_error "$work/error")"
    record_failure "$requested" "$canonical" failed_list "$message"
    echo "  -> failed_list"
    continue
  fi

  count_before="$(jq -r '.projects | length' <<<"$before")"
  selected="$(jq -c --arg title "$title" '
    [.projects[]? | select(
      (.title | type == "string") and
      ((.title | ascii_downcase) == ($title | ascii_downcase))
    )] | sort_by(.number) | .[0] // empty
  ' <<<"$before")"

  action=existing
  if [[ -z "$selected" ]]; then
    if ! gh project create --owner "$canonical" --title "$title" --format json \
      >"$work/create.json" 2>"$work/error"; then
      message="$(safe_error "$work/error")"
      record_failure "$requested" "$canonical" failed_create "$message"
      echo "  -> failed_create"
      continue
    fi
    action=created
  else
    number="$(jq -r '.number' <<<"$selected")"
    old_title="$(jq -r '.title' <<<"$selected")"
    closed="$(jq -r '.closed // false' <<<"$selected")"
    renamed=false
    reopened=false

    if [[ "$old_title" != "$title" ]]; then
      if ! gh project edit "$number" --owner "$canonical" --title "$title" --format json \
        >"$work/edit.json" 2>"$work/error"; then
        message="$(safe_error "$work/error")"
        record_failure "$requested" "$canonical" failed_rename "$message"
        echo "  -> failed_rename"
        continue
      fi
      renamed=true
    fi

    if [[ "$closed" == true ]]; then
      if ! gh project close "$number" --owner "$canonical" --undo --format json \
        >"$work/reopen.json" 2>"$work/error"; then
        message="$(safe_error "$work/error")"
        record_failure "$requested" "$canonical" failed_reopen "$message"
        echo "  -> failed_reopen"
        continue
      fi
      reopened=true
    fi

    if [[ "$renamed" == true && "$reopened" == true ]]; then
      action=renamed_and_reopened
    elif [[ "$renamed" == true ]]; then
      action=renamed
    elif [[ "$reopened" == true ]]; then
      action=reopened
    fi
  fi

  if ! after="$(gh project list --owner "$canonical" --closed --limit 100 --format json 2>"$work/error")"; then
    message="$(safe_error "$work/error")"
    record_failure "$requested" "$canonical" failed_verify_list "$message"
    echo "  -> failed_verify_list"
    continue
  fi

  project="$(jq -c --arg title "$title" '
    [.projects[]? | select(.title == $title and ((.closed // false) == false))]
    | sort_by(.number) | .[0] // empty
  ' <<<"$after")"
  if [[ -z "$project" ]]; then
    record_failure "$requested" "$canonical" failed_verify \
      "canonical active project was absent after mutation"
    echo "  -> failed_verify"
    continue
  fi

  number="$(jq -r '.number' <<<"$project")"
  url="$(jq -r '.url' <<<"$project")"
  actual_title="$(jq -r '.title' <<<"$project")"
  jq -nc \
    --arg requested_org "$requested" \
    --arg org "$canonical" \
    --arg action "$action" \
    --arg title "$actual_title" \
    --arg url "$url" \
    --argjson number "$number" \
    --argjson project_count_before "$count_before" \
    '{
      requested_org:$requested_org,
      org:$org,
      action:$action,
      error:null,
      project:{
        number:$number,
        title:$title,
        url:$url,
        closed:false
      },
      project_count_before:$project_count_before,
      is_project_number_one:($number == 1)
    }' >>"$records"
  echo "  -> $action $url"
done <"$work/candidates"

generated_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
jq -s \
  --arg generated_at "$generated_at" \
  --arg authenticated_login "$authenticated" \
  '{
    schema_version:1,
    generated_at:$generated_at,
    authenticated_login:$authenticated_login,
    summary:{
      candidate_orgs:length,
      successful:(map(select(.error == null)) | length),
      failed:(map(select(.action | startswith("failed"))) | length),
      skipped_not_owner:(map(select(.action == "skipped_not_owner")) | length),
      project_number_one:(map(select(.is_project_number_one == true)) | length),
      actions:(group_by(.action) | map({key:.[0].action,value:length}) | from_entries)
    },
    results:sort_by(.org | ascii_downcase)
  }' "$records" >"$JSON_OUTPUT"

{
  echo '# GitHub organization Projects audit'
  echo
  jq -r '
    "- Generated: `\(.generated_at)`",
    "- Authenticated login: `\(.authenticated_login)`",
    "- Candidate organizations: `\(.summary.candidate_orgs)`",
    "- Successful canonical projects: `\(.summary.successful)`",
    "- Failures: `\(.summary.failed)`",
    "- Skipped non-owner memberships: `\(.summary.skipped_not_owner)`",
    "- Project number 1: `\(.summary.project_number_one)`",
    "",
    "| Organization | Result | Project | URL |",
    "|---|---|---:|---|",
    (.results[] |
      "| `\(.org)` | \(.action)\(if .error then ": " + (.error | gsub("[|\\n]"; " ")) else "" end) | \(.project.number // "") | \(.project.url // "") |"
    )
  ' "$JSON_OUTPUT"
} >"$MARKDOWN_OUTPUT"

jq -c '.summary' "$JSON_OUTPUT"
failed="$(jq -r '.summary.failed' "$JSON_OUTPUT")"
exit "$((failed > 0 ? 1 : 0))"
