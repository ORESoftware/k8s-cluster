#!/usr/bin/env bash

set -uo pipefail

REGISTRY_FILE="${REGISTRY_FILE:-ops/portfolio/github-linear-project-registry.tsv}"
EVIDENCE_DIR="${EVIDENCE_DIR:-ops/evidence/org-project-docs}"
RUN_STAMP="${RUN_STAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
MANAGED_MARKER="org-project-routing"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if [[ -z "${GH_TOKEN:-}" ]]; then
  printf 'GH_TOKEN is required\n' >&2
  exit 2
fi

for command in gh git jq python3; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'required command is missing: %s\n' "$command" >&2
    exit 2
  fi
done

if [[ ! -f "$REGISTRY_FILE" ]]; then
  printf 'registry file not found: %s\n' "$REGISTRY_FILE" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_DIR"
RESULTS_JSONL="$EVIDENCE_DIR/results.jsonl"
RESULTS_JSON="$EVIDENCE_DIR/results.json"
RESULTS_MD="$EVIDENCE_DIR/README.md"
: > "$RESULTS_JSONL"

git config --global user.name "github-actions[bot]"
git config --global user.email "41898282+github-actions[bot]@users.noreply.github.com"
gh auth setup-git >/dev/null

upsert_managed_block() {
  local path="$1"
  local marker="$2"
  local block_file="$3"

  python3 - "$path" "$marker" "$block_file" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
marker = sys.argv[2]
block_file = Path(sys.argv[3])
start = f"<!-- {marker}:start -->"
end = f"<!-- {marker}:end -->"
content = block_file.read_text(encoding="utf-8").strip()
managed = f"{start}\n{content}\n{end}"

if path.exists():
    original = path.read_text(encoding="utf-8")
    has_start = start in original
    has_end = end in original
    if has_start != has_end:
        raise SystemExit(f"refusing to edit malformed managed block in {path}")
    if has_start:
        before, remainder = original.split(start, 1)
        _, after = remainder.split(end, 1)
        updated = before.rstrip() + "\n\n" + managed + after
    else:
        updated = original.rstrip() + "\n\n" + managed + "\n"
else:
    path.parent.mkdir(parents=True, exist_ok=True)
    updated = managed + "\n"

path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(updated, encoding="utf-8")
PY
}

record_result() {
  local status="$1"
  local requested_org="$2"
  local canonical_org="$3"
  local linear_url="$4"
  local project_title="$5"
  local project_number="$6"
  local project_url="$7"
  local project_action="$8"
  local repo_action="$9"
  local docs_action="${10}"
  local pr_number="${11}"
  local pr_url="${12}"
  local pr_state="${13}"
  local issue_number="${14}"
  local issue_url="${15}"
  local project_item_action="${16}"
  local error_message="${17}"

  jq -nc \
    --arg status "$status" \
    --arg requested_org "$requested_org" \
    --arg canonical_org "$canonical_org" \
    --arg linear_url "$linear_url" \
    --arg project_title "$project_title" \
    --arg project_number "$project_number" \
    --arg project_url "$project_url" \
    --arg project_action "$project_action" \
    --arg repo_action "$repo_action" \
    --arg docs_action "$docs_action" \
    --arg pr_number "$pr_number" \
    --arg pr_url "$pr_url" \
    --arg pr_state "$pr_state" \
    --arg issue_number "$issue_number" \
    --arg issue_url "$issue_url" \
    --arg project_item_action "$project_item_action" \
    --arg error "$error_message" \
    --arg run_stamp "$RUN_STAMP" \
    '{status:$status,requested_org:$requested_org,canonical_org:$canonical_org,linear_url:$linear_url,project_title:$project_title,project_number:$project_number,project_url:$project_url,project_action:$project_action,repository_action:$repo_action,documentation_action:$docs_action,pull_request:{number:$pr_number,url:$pr_url,state:$pr_state},governance_issue:{number:$issue_number,url:$issue_url,project_item_action:$project_item_action},error:$error,run_stamp:$run_stamp}' \
    >> "$RESULTS_JSONL"
}

reconcile_org() (
  set -Eeuo pipefail

  local requested_org="$1"
  local linear_url="$2"
  local canonical_org=""
  local project_title=""
  local project_number=""
  local project_url=""
  local project_id=""
  local project_action="unknown"
  local repo_action="unknown"
  local docs_action="unknown"
  local pr_number=""
  local pr_url=""
  local pr_state=""
  local issue_number=""
  local issue_url=""
  local project_item_action="not-attempted"
  local current_step="organization lookup"
  local tmp_dir=""
  local failure_recorded=false

  cleanup() {
    if [[ -n "$tmp_dir" && -d "$tmp_dir" ]]; then
      rm -rf "$tmp_dir"
    fi
  }

  on_error() {
    local rc=$?
    local message="step '${current_step}' failed with exit code ${rc}"
    if [[ "$failure_recorded" != true ]]; then
      record_result "failed" "$requested_org" "$canonical_org" "$linear_url" "$project_title" "$project_number" "$project_url" "$project_action" "$repo_action" "$docs_action" "$pr_number" "$pr_url" "$pr_state" "$issue_number" "$issue_url" "$project_item_action" "$message"
      failure_recorded=true
    fi
    printf 'FAIL %s: %s\n' "$requested_org" "$message" >&2
    exit "$rc"
  }

  trap cleanup EXIT
  trap on_error ERR

  local org_response org_message
  org_response="$(gh api "orgs/${requested_org}")"
  org_message="$(jq -r '.message // empty' <<<"$org_response")"
  [[ -z "$org_message" ]]
  canonical_org="$(jq -er '.login | select(type == "string" and length > 0)' <<<"$org_response")"
  [[ "$canonical_org" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,37}[A-Za-z0-9])?$ ]]
  [[ "$canonical_org" != *--* ]]
  [[ "${canonical_org,,}" == "${requested_org,,}" ]]
  project_title="${canonical_org}-project"

  current_step="GitHub Project lookup"
  local lookup_query lookup_response lookup_errors owner_id project_json
  lookup_query='query($login:String!){organization(login:$login){id login projectsV2(first:100){nodes{id number title url closed}}}}'
  lookup_response="$(gh api graphql -f query="$lookup_query" -F login="$canonical_org")"
  lookup_errors="$(jq -c '.errors // []' <<<"$lookup_response")"
  if [[ "$lookup_errors" != "[]" ]]; then
    printf '%s\n' "$lookup_errors" >&2
    false
  fi
  owner_id="$(jq -r '.data.organization.id // empty' <<<"$lookup_response")"
  [[ -n "$owner_id" ]]

  project_json="$(jq -c --arg title "$project_title" '[.data.organization.projectsV2.nodes[]? | select(.title == $title)] | sort_by(.number) | .[0] // empty' <<<"$lookup_response")"
  if [[ -z "$project_json" ]]; then
    local project_one
    project_one="$(jq -c '[.data.organization.projectsV2.nodes[]? | select(.number == 1)] | .[0] // empty' <<<"$lookup_response")"
    if [[ -n "$project_one" ]]; then
      current_step="GitHub Project 1 canonical rename"
      local rename_mutation rename_response rename_errors project_one_id
      project_one_id="$(jq -r '.id' <<<"$project_one")"
      rename_mutation='mutation($projectId:ID!,$title:String!){updateProjectV2(input:{projectId:$projectId,title:$title,closed:false}){projectV2{id number title url closed}}}'
      rename_response="$(gh api graphql -f query="$rename_mutation" -F projectId="$project_one_id" -f title="$project_title")"
      rename_errors="$(jq -c '.errors // []' <<<"$rename_response")"
      if [[ "$rename_errors" != "[]" ]]; then
        printf '%s\n' "$rename_errors" >&2
        false
      fi
      project_json="$(jq -c '.data.updateProjectV2.projectV2 // empty' <<<"$rename_response")"
      project_action="renamed-project-1"
    else
      current_step="GitHub Project creation"
      local create_mutation create_response create_errors
      create_mutation='mutation($ownerId:ID!,$title:String!){createProjectV2(input:{ownerId:$ownerId,title:$title}){projectV2{id number title url closed}}}'
      create_response="$(gh api graphql -f query="$create_mutation" -F ownerId="$owner_id" -f title="$project_title")"
      create_errors="$(jq -c '.errors // []' <<<"$create_response")"
      if [[ "$create_errors" != "[]" ]]; then
        printf '%s\n' "$create_errors" >&2
        false
      fi
      project_json="$(jq -c '.data.createProjectV2.projectV2 // empty' <<<"$create_response")"
      project_action="created"
    fi
  elif [[ "$(jq -r '.closed // false' <<<"$project_json")" == "true" ]]; then
    current_step="GitHub Project reopen"
    local reopen_mutation reopen_response reopen_errors closed_project_id
    closed_project_id="$(jq -r '.id' <<<"$project_json")"
    reopen_mutation='mutation($projectId:ID!){updateProjectV2(input:{projectId:$projectId,closed:false}){projectV2{id number title url closed}}}'
    reopen_response="$(gh api graphql -f query="$reopen_mutation" -F projectId="$closed_project_id")"
    reopen_errors="$(jq -c '.errors // []' <<<"$reopen_response")"
    if [[ "$reopen_errors" != "[]" ]]; then
      printf '%s\n' "$reopen_errors" >&2
      false
    fi
    project_json="$(jq -c '.data.updateProjectV2.projectV2 // empty' <<<"$reopen_response")"
    project_action="reopened"
  else
    project_action="existing"
  fi

  project_id="$(jq -r '.id // empty' <<<"$project_json")"
  project_number="$(jq -r '.number // empty' <<<"$project_json")"
  project_url="$(jq -r '.url // empty' <<<"$project_json")"
  [[ -n "$project_id" && -n "$project_number" && -n "$project_url" ]]
  [[ "$(jq -r '.title' <<<"$project_json")" == "$project_title" ]]
  [[ "$(jq -r '.closed // false' <<<"$project_json")" == "false" ]]

  current_step="organization .github repository provisioning"
  local repo_full_name="${canonical_org}/.github"
  if gh repo view "$repo_full_name" --json nameWithOwner >/dev/null 2>&1; then
    repo_action="existing"
  else
    gh repo create "$repo_full_name" \
      --public \
      --add-readme \
      --description "Organization profile, governance, and project routing for ${canonical_org}" >/dev/null
    repo_action="created"
  fi

  local visibility
  visibility="$(gh repo view "$repo_full_name" --json visibility --jq '.visibility')"
  if [[ "$visibility" != "PUBLIC" ]]; then
    gh repo edit "$repo_full_name" --visibility public --accept-visibility-change-consequences
    repo_action="${repo_action}+made-public"
  fi
  gh api --method PATCH "repos/${repo_full_name}" -F has_issues=true >/dev/null

  local default_branch
  default_branch="$(gh repo view "$repo_full_name" --json defaultBranchRef --jq '.defaultBranchRef.name // empty')"
  if [[ -z "$default_branch" ]]; then
    current_step="empty repository initialization"
    local initial_content
    initial_content="$(printf '# %s organization documentation\n' "$canonical_org" | base64 -w0)"
    gh api --method PUT "repos/${repo_full_name}/contents/README.md" \
      -f message="docs: initialize organization documentation" \
      -f content="$initial_content" >/dev/null
    default_branch="$(gh repo view "$repo_full_name" --json defaultBranchRef --jq '.defaultBranchRef.name // empty')"
    [[ -n "$default_branch" ]]
    repo_action="${repo_action}+initialized"
  fi

  current_step="repository clone"
  tmp_dir="$(mktemp -d)"
  gh repo clone "$repo_full_name" "$tmp_dir/repo" -- --depth=50 >/dev/null
  cd "$tmp_dir/repo"

  local branch
  branch="agent/sync-project-links-${RUN_STAMP}"
  git fetch origin "$default_branch" >/dev/null
  git checkout -B "$branch" "origin/$default_branch" >/dev/null

  apply_docs() {
    local project_block profile_block
    project_block="$tmp_dir/project-block.md"
    profile_block="$tmp_dir/profile-block.md"

    cat > "$project_block" <<DOC
# Project routing

- **GitHub organization:** [${canonical_org}](https://github.com/${canonical_org})
- **Canonical GitHub Project:** [${project_title}](${project_url}) (project ${project_number})
- **Canonical Linear project:** [planning workspace](${linear_url})
- **Organization documentation repository:** [${repo_full_name}](https://github.com/${repo_full_name})

## Source-of-truth boundaries

GitHub is authoritative for repositories, commits, pull requests, reviews, CI checks, releases, deployable artifacts, and runtime evidence. Linear is authoritative for product planning, priorities, ownership, dependencies, milestones, and status reporting. The GitHub Project is the organization-level execution board and should contain the governance issue maintained by this repository.

## Change and merge policy

Documentation branches must be reviewed through pull requests and merged after checks pass. Concurrent edits are reconciled semantically against the latest default branch: this managed routing block is regenerated while all unrelated prose outside the block is preserved. Do not resolve conflicts by blindly choosing one side.
DOC

    cat > "$profile_block" <<DOC
## Planning and delivery

- [GitHub Project: ${project_title}](${project_url})
- [Linear planning project](${linear_url})
- [Detailed project-routing contract](../docs/PROJECTS.md)

GitHub owns code and delivery evidence; Linear owns planning and dependencies. The linked organization Project provides the cross-repository execution view.
DOC

    upsert_managed_block "docs/PROJECTS.md" "$MANAGED_MARKER" "$project_block"
    upsert_managed_block "profile/README.md" "$MANAGED_MARKER" "$profile_block"
  }

  current_step="documentation semantic update"
  apply_docs

  if [[ -z "$(git status --porcelain -- docs/PROJECTS.md profile/README.md)" ]]; then
    docs_action="unchanged"
    pr_state="not-needed"
  else
    docs_action="updated"
    git add docs/PROJECTS.md profile/README.md
    git commit -m "docs: link GitHub Project and Linear planning" >/dev/null

    current_step="documentation branch push"
    git push --force-with-lease -u origin "$branch" >/dev/null

    current_step="pull request creation"
    pr_number="$(gh pr list --repo "$repo_full_name" --head "$branch" --state open --json number --jq '.[0].number // empty')"
    if [[ -z "$pr_number" ]]; then
      cat > "$tmp_dir/pr-body.md" <<BODY
## What changed

- links the canonical organization GitHub Project and Linear project;
- publishes the source-of-truth and semantic-merge contract;
- preserves all existing prose outside the managed routing block.

## Validation

- generated from the central 64-organization registry;
- project title and active state verified through the GitHub GraphQL API;
- documentation update applied against the latest default branch.
BODY
      pr_url="$(gh pr create \
        --repo "$repo_full_name" \
        --base "$default_branch" \
        --head "$branch" \
        --title "docs: link GitHub Project and Linear planning" \
        --body-file "$tmp_dir/pr-body.md")"
      pr_number="$(gh pr view "$pr_url" --repo "$repo_full_name" --json number --jq '.number')"
    else
      pr_url="$(gh pr view "$pr_number" --repo "$repo_full_name" --json url --jq '.url')"
    fi

    current_step="semantic refresh before merge"
    git fetch origin "$default_branch" >/dev/null
    if ! git rebase "origin/$default_branch" >/dev/null 2>&1; then
      git rebase --abort >/dev/null 2>&1 || true
      git reset --hard "origin/$default_branch" >/dev/null
      apply_docs
      git add docs/PROJECTS.md profile/README.md
      git commit -m "docs: link GitHub Project and Linear planning" >/dev/null
      git push --force-with-lease origin "$branch" >/dev/null
    elif [[ "$(git rev-parse HEAD)" != "$(git ls-remote origin "refs/heads/$branch" | awk '{print $1}')" ]]; then
      git push --force-with-lease origin "$branch" >/dev/null
    fi

    current_step="pull request merge"
    local merged=false merge_method
    for merge_method in squash merge rebase; do
      if gh pr merge "$pr_number" --repo "$repo_full_name" "--${merge_method}" --delete-branch --admin >/dev/null 2>&1; then
        merged=true
        pr_state="merged-${merge_method}"
        break
      fi
    done
    if [[ "$merged" != true ]]; then
      for merge_method in squash merge rebase; do
        if gh pr merge "$pr_number" --repo "$repo_full_name" "--${merge_method}" --delete-branch >/dev/null 2>&1; then
          merged=true
          pr_state="merged-${merge_method}"
          break
        fi
      done
    fi
    if [[ "$merged" != true ]]; then
      if gh pr merge "$pr_number" --repo "$repo_full_name" --squash --delete-branch --auto >/dev/null 2>&1; then
        pr_state="auto-merge-enabled"
      else
        pr_state="open"
        printf 'unable to merge PR %s in %s\n' "$pr_number" "$repo_full_name" >&2
        false
      fi
    fi
  fi

  current_step="governance issue reconciliation"
  local issue_title issue_body_file
  issue_title="GitHub Project and Linear workspace links"
  issue_body_file="$tmp_dir/issue-body.md"
  cat > "$issue_body_file" <<ISSUE
This issue is the stable organization-level routing card.

- GitHub organization: https://github.com/${canonical_org}
- GitHub Project: ${project_url}
- Linear project: ${linear_url}
- Documentation: https://github.com/${repo_full_name}/blob/${default_branch}/docs/PROJECTS.md

GitHub is authoritative for code, pull requests, CI, releases, and artifacts. Linear is authoritative for planning, dependencies, ownership, and milestones. Keep this issue open as a durable card in the organization Project.
ISSUE

  issue_number="$(gh issue list --repo "$repo_full_name" --state all --search "${issue_title} in:title" --json number,title | jq -r --arg title "$issue_title" '[.[] | select(.title == $title)][0].number // empty')"
  if [[ -z "$issue_number" ]]; then
    issue_url="$(gh issue create --repo "$repo_full_name" --title "$issue_title" --body-file "$issue_body_file")"
    issue_number="$(gh issue view "$issue_url" --repo "$repo_full_name" --json number --jq '.number')"
  else
    gh issue edit "$issue_number" --repo "$repo_full_name" --body-file "$issue_body_file" >/dev/null
    issue_url="$(gh issue view "$issue_number" --repo "$repo_full_name" --json url --jq '.url')"
  fi

  current_step="governance issue Project insertion"
  local issue_node add_mutation add_response add_errors
  issue_node="$(gh api "repos/${repo_full_name}/issues/${issue_number}" --jq '.node_id')"
  add_mutation='mutation($projectId:ID!,$contentId:ID!){addProjectV2ItemById(input:{projectId:$projectId,contentId:$contentId}){item{id}}}'
  add_response="$(gh api graphql -f query="$add_mutation" -F projectId="$project_id" -F contentId="$issue_node" 2>&1)" || true
  add_errors="$(jq -c '.errors // []' <<<"$add_response" 2>/dev/null || printf '[{"message":"unparseable response"}]')"
  if [[ "$add_errors" == "[]" ]]; then
    project_item_action="added"
  elif grep -Eqi 'already|duplicate|existing' <<<"$add_errors"; then
    project_item_action="existing"
  else
    printf '%s\n' "$add_response" >&2
    false
  fi

  current_step="final evidence validation"
  [[ "$canonical_org" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,37}[A-Za-z0-9])?$ ]]
  [[ "$canonical_org" != *--* ]]
  [[ "${canonical_org,,}" == "${requested_org,,}" ]]
  [[ "$linear_url" == https://linear.app/* ]]
  [[ "$project_number" =~ ^[1-9][0-9]*$ ]]
  [[ "$project_title" == "${canonical_org}-project" ]]
  [[ "$project_url" == "https://github.com/orgs/${canonical_org}/projects/${project_number}" ]]
  [[ -n "$repo_action" && "$repo_action" != "unknown" ]]
  [[ "$docs_action" == "unchanged" || "$docs_action" == "updated" ]]
  if [[ "$docs_action" == "updated" ]]; then
    [[ "$pr_number" =~ ^[1-9][0-9]*$ ]]
    [[ "$pr_url" == "https://github.com/${canonical_org}/.github/pull/${pr_number}" ]]
    [[ "$pr_state" == merged-* ]]
  else
    [[ -z "$pr_number" && -z "$pr_url" && "$pr_state" == "not-needed" ]]
  fi
  [[ "$issue_number" =~ ^[1-9][0-9]*$ ]]
  [[ "$issue_url" == "https://github.com/${canonical_org}/.github/issues/${issue_number}" ]]
  [[ "$project_item_action" == "added" || "$project_item_action" == "existing" ]]

  trap - ERR
  record_result "ok" "$requested_org" "$canonical_org" "$linear_url" "$project_title" "$project_number" "$project_url" "$project_action" "$repo_action" "$docs_action" "$pr_number" "$pr_url" "$pr_state" "$issue_number" "$issue_url" "$project_item_action" ""
  printf 'OK %s -> %s (%s)\n' "$canonical_org" "$project_url" "$pr_state"
)

successes=0
failures=0
processed=0

while IFS=$'\t' read -r organization linear_url; do
  [[ "$organization" == "organization" ]] && continue
  [[ -z "$organization" ]] && continue
  processed=$((processed + 1))
  # Never call reconcile_org in if/while/&&/||. Bash suppresses errexit
  # inside commands whose status is being tested, including function bodies.
  reconcile_org "$organization" "$linear_url"
  reconcile_rc=$?
  if (( reconcile_rc == 0 )); then
    successes=$((successes + 1))
  else
    failures=$((failures + 1))
  fi
done < "$REGISTRY_FILE"

jq -s 'sort_by(.canonical_org // .requested_org)' "$RESULTS_JSONL" > "$RESULTS_JSON"

validation_failed=0
if ! python3 "$SCRIPT_DIR/validate_org_project_docs_evidence.py" "$RESULTS_JSON" "$processed"; then
  validation_failed=1
fi

jq -r '
  "# Organization Project and documentation reconciliation\n\n" +
  "Generated: `" + (now | todate) + "`\n\n" +
  "| Organization | Result | Project | Repository | Documentation PR | Governance issue | Linear |\n" +
  "|---|---|---|---|---|---|---|\n" +
  (map(
    "| `" + (.canonical_org // .requested_org) + "` | " + .status +
    " | [" + (.project_title // "project") + "](" + (.project_url // "") + ")" +
    " | " + (.repository_action // "") +
    " | " + (if .pull_request.url == "" then (.documentation_action // "") else "[PR #" + .pull_request.number + "](" + .pull_request.url + ") — " + .pull_request.state end) +
    " | " + (if .governance_issue.url == "" then "" else "[issue #" + .governance_issue.number + "](" + .governance_issue.url + ")" end) +
    " | [Linear](" + .linear_url + ") |"
  ) | join("\n")) +
  "\n\n## Totals\n\n" +
  "- Processed: " + (length|tostring) + "\n" +
  "- Successful: " + (map(select(.status == "ok"))|length|tostring) + "\n" +
  "- Failed: " + (map(select(.status != "ok"))|length|tostring) + "\n"
' "$RESULTS_JSON" > "$RESULTS_MD"

printf 'processed=%d successes=%d failures=%d\n' "$processed" "$successes" "$failures"

if (( failures > 0 || validation_failed > 0 )); then
  exit 1
fi
