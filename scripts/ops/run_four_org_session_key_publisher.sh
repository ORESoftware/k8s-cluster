#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

base_script="${GITHUB_WORKSPACE:?GITHUB_WORKSPACE is required}/scripts/ops/run_four_org_device_auth_publisher.sh"
patched_script="${RUNNER_TEMP:?RUNNER_TEMP is required}/run_requested_fleets_device_auth_publisher.session-key.sh"
: "${STREEMPILOT_FLEET_ROOT:?STREEMPILOT_FLEET_ROOT is required}"

python3 - "$base_script" "$patched_script" <<'PY'
from __future__ import annotations

import re
import sys
from pathlib import Path

source_path = Path(sys.argv[1])
target_path = Path(sys.argv[2])
source = source_path.read_text(encoding="utf-8")

public_key = """-----BEGIN PUBLIC KEY-----
MIIBojANBgkqhkiG9w0BAQEFAAOCAY8AMIIBigKCAYEAs/AEBSv+XPPG41Wpr4wH
yu2fYx9f84iJDDtDjvWrTE4cojzkeEeMFamXgB7xm8yEhRPnY5z+HYLdgqqs0DRR
r/9m50PpQVjGg1Nv4WJ7I+qAI6Myjnr0cW26z1cJg7SOYNxiWb5xq2Dsj1b5Kq+M
7caER0VrInMpgJgk5pfTBayPCwq+NUnY+kTwohNvWGVifLorIVcxj+qTi146P85Y
VhRpO143mC4EM0ld3WDg2iITOcGSV+NW2hTH4rwQlRDLgI886kpZREgJIm+sPXu6
G6CGukE7ZU7AaO9O+5tmRJIvypBUoOvmZRstHPpw3CzU5Sci0a+jVUg0lT65aPOA
Q4hTqRXp2V5WeLM6ACERpNlsrp2Hi6GBpPYk9VWLbYRAvFcWyoGS6Q+pxFNEcq4A
TPVNPxRcEsL6srCO5kP3QhkXZNQqddrJm1jtwxgRwOdihgl5bz70vstJ3Etuou/I
TuvPIs+302HcV784dDLOP8UeW3s1i9YnestOOST0mziTAgMBAAE=
-----END PUBLIC KEY-----"""
fingerprint = "0a43ada96c0502be63c36c3fbdcb6977c3ded5289a8993d3c5427894fdd6d0a2"


def replace_once(old: str, new: str, label: str) -> None:
    global source
    matches = source.count(old)
    if matches != 1:
        raise SystemExit(f"{label}: expected one match, found {matches}")
    source = source.replace(old, new, 1)


pattern = re.compile(
    r"cat > \"\$public_key\" <<'PUBLIC_KEY'\n"
    r".*?\n"
    r"PUBLIC_KEY\n"
    r"expected_fingerprint='[0-9a-f]{64}'",
    re.DOTALL,
)
replacement = (
    'cat > "$public_key" <<\'PUBLIC_KEY\'\n'
    + public_key
    + "\nPUBLIC_KEY\nexpected_fingerprint='"
    + fingerprint
    + "'"
)
source, replacements = pattern.subn(lambda _: replacement, source, count=1)
if replacements != 1:
    raise SystemExit(f"device recipient: expected one block, replaced {replacements}")

replace_once(
    "organizations=(apostille-me evento-globolo hacker-house-medellin embedded-alerts)\n",
    "organizations=(apostille-me evento-globolo hacker-house-medellin embedded-alerts StreemPilot)\n",
    "organization list",
)

replace_once(
    '[[ -x "$FLEET_ROOT/scripts/publish-all.sh" ]]\n',
    '''[[ -x "$FLEET_ROOT/scripts/publish-all.sh" ]]
[[ -x "$STREEMPILOT_FLEET_ROOT/scripts/publish-all.sh" ]]
[[ -s "$STREEMPILOT_FLEET_ROOT/REPOSITORY_MANIFEST.json" ]]
[[ "$(jq -er '.repository_count' "$STREEMPILOT_FLEET_ROOT/REPOSITORY_MANIFEST.json")" == 8 ]]
''',
    "fleet prerequisites",
)

replace_once(
    '''gh auth setup-git --hostname github.com --force
"$FLEET_ROOT/scripts/publish-all.sh" "$FLEET_ROOT"
''',
    '''gh auth setup-git --hostname github.com --force
"$FLEET_ROOT/scripts/publish-all.sh" "$FLEET_ROOT"
CODE_VISIBILITY=private DRAFT_PRS=0 "$STREEMPILOT_FLEET_ROOT/scripts/publish-all.sh"
''',
    "publication calls",
)

replace_once(
    '''results="$FLEET_ROOT/publication-results.json"
[[ -s "$results" ]]
repository_count="$(jq -er '.repository_count // .summary.repository_count // 48' "$results")"
pull_request_count="$(jq -er '.pull_request_count // .summary.pull_request_count // 20' "$results")"
[[ "$repository_count" == 48 ]]
[[ "$pull_request_count" == 20 ]]
''',
    '''results="$FLEET_ROOT/publication-results.json"
[[ -s "$results" ]]
four_org_repository_count="$(jq -er '.repository_count // .summary.repository_count // 48' "$results")"
four_org_pull_request_count="$(jq -er '.pull_request_count // .summary.pull_request_count // 20' "$results")"
[[ "$four_org_repository_count" == 48 ]]
[[ "$four_org_pull_request_count" == 20 ]]

streempilot_repository_count="$(jq -er '.repositories | length' "$STREEMPILOT_FLEET_ROOT/REPOSITORY_MANIFEST.json")"
[[ "$streempilot_repository_count" == 8 ]]
streempilot_pull_request_count=0
while IFS=$'\t' read -r full feature; do
  count="$(gh pr list --repo "$full" --base main --head "$feature" --state all --json number --jq 'length')"
  (( count >= 1 )) || {
    echo "Pull request missing for ${full}:${feature}." >&2
    exit 82
  }
  streempilot_pull_request_count=$((streempilot_pull_request_count + 1))
done < <(
  jq -r '.repositories[] | [.full_name,.feature_branch] | @tsv' \
    "$STREEMPILOT_FLEET_ROOT/REPOSITORY_MANIFEST.json"
)
[[ "$streempilot_pull_request_count" == 8 ]]

repository_count=$((four_org_repository_count + streempilot_repository_count))
pull_request_count=$((four_org_pull_request_count + streempilot_pull_request_count))
[[ "$repository_count" == 56 ]]
[[ "$pull_request_count" == 28 ]]
''',
    "remote verification block",
)

replace_once(
    '"- Organizations: **4**\\n" +\n',
    '"- Organizations: **5**\\n" +\n',
    "success organization count",
)
replace_once(
    '"- Cloudflare Worker packages: **4**\\n\\n" +\n',
    '"- Cloudflare Worker packages: **5**\\n\\n" +\n',
    "success worker count",
)
replace_once(
    "printf 'FOUR_ORG_PUBLICATION_COMPLETE actor=%s repositories=%s pull_requests=%s\\n' \\\n",
    "printf 'REQUESTED_FLEETS_PUBLICATION_COMPLETE actor=%s repositories=%s pull_requests=%s\\n' \\\n",
    "completion marker",
)

if fingerprint not in source or public_key not in source:
    raise SystemExit("session recipient was not embedded")
if "0910b9a6f418e5e898957138ba98c641e721cb3da0a36d9e6da529d2a7d1db06" in source:
    raise SystemExit("legacy recipient fingerprint remains")
if "StreemPilot" not in source or '"$repository_count" == 56' not in source:
    raise SystemExit("combined fleet guards were not embedded")

target_path.write_text(source, encoding="utf-8")
PY

chmod 700 "$patched_script"
exec bash "$patched_script"
