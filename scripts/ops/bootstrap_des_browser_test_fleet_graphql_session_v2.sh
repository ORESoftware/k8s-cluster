#!/usr/bin/env bash
set -Eeuo pipefail

base_script="${GITHUB_WORKSPACE:?GITHUB_WORKSPACE is required}/scripts/ops/bootstrap_des_browser_test_fleet_graphql_session.sh"
patched_script="${RUNNER_TEMP:?RUNNER_TEMP is required}/bootstrap_des_browser_test_fleet_graphql_session.v2.sh"

python3 - "$base_script" "$patched_script" <<'PY'
from pathlib import Path
import sys

source_path = Path(sys.argv[1])
target_path = Path(sys.argv[2])
source = source_path.read_text(encoding="utf-8")
old = '''  graphql "$lookup_query" "$variables" "$work/repository.json"
  if [[ "$(jq -r '.data.repository.id // empty' "$work/repository.json")" == '' ]]; then
    create_mutation='mutation($ownerId:ID!,$name:String!,$description:String!){createRepository(input:{ownerId:$ownerId,name:$name,description:$description,visibility:PUBLIC,hasIssuesEnabled:true,hasWikiEnabled:false}){repository{id nameWithOwner url}}}'
    variables="$(jq -nc --arg ownerId "$org_id" --arg name "$name" --arg description "$description" '{ownerId:$ownerId,name:$name,description:$description}')"
    graphql "$create_mutation" "$variables" "$work/repository.json"
  fi
'''
new = '''  if ! graphql "$lookup_query" "$variables" "$work/repository.json"; then
    if jq -e '.errors[]? | select(.type == "NOT_FOUND" or (.message | startswith("Could not resolve to a Repository")))' "$work/repository.json" >/dev/null; then
      jq -nc '{data:{repository:null}}' > "$work/repository.json"
    else
      return 1
    fi
  fi
  if [[ "$(jq -r '.data.repository.id // empty' "$work/repository.json")" == '' ]]; then
    create_mutation='mutation($ownerId:ID!,$name:String!,$description:String!){createRepository(input:{ownerId:$ownerId,name:$name,description:$description,visibility:PUBLIC,hasIssuesEnabled:true,hasWikiEnabled:false}){repository{id nameWithOwner url}}}'
    variables="$(jq -nc --arg ownerId "$org_id" --arg name "$name" --arg description "$description" '{ownerId:$ownerId,name:$name,description:$description}')"
    graphql "$create_mutation" "$variables" "$work/repository.json"
  fi
'''
if source.count(old) != 1:
    raise SystemExit("expected repository lookup block was not found exactly once")
source = source.replace(old, new, 1)
target_path.write_text(source, encoding="utf-8")
PY

chmod 700 "$patched_script"
exec bash "$patched_script"
