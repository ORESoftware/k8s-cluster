#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

: "${GITHUB_TOKEN:?workflow token is required}"
: "${GITHUB_WORKSPACE:?GitHub workspace is required}"
: "${RUNNER_TEMP:?runner temp directory is required}"

readonly control_repository="ORESoftware/k8s-cluster"
readonly carrier_issue="916"
readonly canonical_repository="zed-pkg-test/zed-pkg-e2e"
readonly canonical_sha="dd3157606e3412e533da7b782393724038562bf3"
readonly audit_sha="e8876528e782dab95918ff7d2f33e7e83d3e2a7d"
readonly queued_paced_run="30974844610"
readonly redundant_k8s_runs="30974029929 30973589779"

stage="initialization"
workflow_token="$GITHUB_TOKEN"
owner_token=""
work="$(mktemp -d "$RUNNER_TEMP/canonical-test-fleet.XXXXXX")"

cleanup() {
  unset owner_token GH_TOKEN GITHUB_TOKEN GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  find "$work" -type f -exec sh -c 'for file do : > "$file"; done' sh {} + 2>/dev/null || true
  rm -rf "$work"
}

post_control_comment() {
  local body_file="$1"
  WORKFLOW_TOKEN="$workflow_token" BODY_FILE="$body_file" python3 - <<'PY'
import json
import os
import urllib.request

repository = "ORESoftware/k8s-cluster"
issue = 916
body = open(os.environ["BODY_FILE"], encoding="utf-8").read()
request = urllib.request.Request(
    f"https://api.github.com/repos/{repository}/issues/{issue}/comments",
    data=json.dumps({"body": body}).encode(),
    method="POST",
    headers={
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {os.environ['WORKFLOW_TOKEN']}",
        "Content-Type": "application/json",
        "User-Agent": "canonical-test-fleet-sonus-ci",
        "X-GitHub-Api-Version": "2022-11-28",
    },
)
with urllib.request.urlopen(request, timeout=30) as response:
    payload = json.load(response)
print(payload["id"])
PY
}

report_failure() {
  local status=$?
  trap - ERR
  local failure="$work/failure.md"
  cat > "$failure" <<EOF
Canonical test-organization fleet publication failed at bounded stage \`$stage\` on the isolated \`sonus-ci\` runner. Existing repositories, commits, branches, and pull requests remain intact for an idempotent retry. No plaintext owner credential was committed, uploaded, placed in an artifact, written to Actions outputs, or retained after the job.
EOF
  post_control_comment "$failure" >/dev/null || true
  exit "$status"
}

trap cleanup EXIT
trap report_failure ERR

for command in git node python3 openssl; do
  command -v "$command" >/dev/null
  printf 'PREREQUISITE %s=%s\n' "$command" "$(command -v "$command")"
done

stage="clone-canonical-fleet"
canonical="$work/fleet"
git init "$canonical" >/dev/null
git -C "$canonical" remote add origin "https://github.com/${canonical_repository}.git"
git -C "$canonical" fetch --depth=1 origin "$canonical_sha" >/dev/null
git -C "$canonical" checkout --detach FETCH_HEAD >/dev/null
test "$(git -C "$canonical" rev-parse HEAD)" = "$canonical_sha"

stage="issue-ephemeral-challenge"
private_key="$work/private.pem"
public_key="$work/public.pem"
ciphertext_file="$work/ciphertext.bin"
openssl genpkey -quiet -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out "$private_key"
chmod 600 "$private_key"
openssl pkey -in "$private_key" -pubout -out "$public_key"
nonce="$(openssl rand -hex 24)"
[[ "$nonce" =~ ^[0-9a-f]{48}$ ]]
challenge="$work/challenge.md"
{
  printf '<!-- canonical-test-fleet-sonus-ci-challenge:%s -->\n' "$nonce"
  printf 'One-time RSA-OAEP-SHA256/MGF1-SHA256 challenge for the canonical 341-repository test fleet. The private key exists only on this isolated `sonus-ci` runner and is destroyed on exit. The run cancels redundant publishers before mutation and performs the exact live fleet audit before success.\n\n'
  printf '```pem\n'
  cat "$public_key"
  printf '```\n'
} > "$challenge"
challenge_id="$(post_control_comment "$challenge")"
[[ "$challenge_id" =~ ^[0-9]+$ ]]
response_marker="<!-- canonical-test-fleet-sonus-ci-response:${nonce} -->"

stage="await-encrypted-owner-response"
response_body="$(
  WORKFLOW_TOKEN="$workflow_token" RESPONSE_MARKER="$response_marker" CHALLENGE_ID="$challenge_id" python3 - <<'PY'
import json
import os
import time
import urllib.request

repository = "ORESoftware/k8s-cluster"
issue = 916
marker = os.environ["RESPONSE_MARKER"] + "\n"
challenge_id = int(os.environ["CHALLENGE_ID"])
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {os.environ['WORKFLOW_TOKEN']}",
    "User-Agent": "canonical-test-fleet-sonus-ci",
    "X-GitHub-Api-Version": "2022-11-28",
}
for _ in range(720):
    request = urllib.request.Request(
        f"https://api.github.com/repos/{repository}/issues/{issue}/comments?per_page=100",
        headers=headers,
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        comments = json.load(response)
    matches = [
        item for item in comments
        if int(item.get("id", 0)) > challenge_id
        and item.get("user", {}).get("login") == "ORESoftware"
        and str(item.get("body", "")).startswith(marker)
    ]
    if matches:
        print(sorted(matches, key=lambda item: int(item["id"]))[-1]["body"])
        raise SystemExit(0)
    time.sleep(5)
raise SystemExit("timed out waiting for encrypted owner response")
PY
)"
test -n "$response_body"
test "$(grep -c '^ciphertext-base64=' <<<"$response_body")" -eq 1
ciphertext="$(sed -n 's/^ciphertext-base64=//p' <<<"$response_body")"
[[ "$ciphertext" =~ ^[A-Za-z0-9+/=]+$ ]]
test "${#ciphertext}" -le 8192
printf '%s' "$ciphertext" | base64 --decode > "$ciphertext_file"
test -s "$ciphertext_file"

stage="decrypt-owner-credential"
owner_token="$(
  openssl pkeyutl -decrypt \
    -inkey "$private_key" \
    -in "$ciphertext_file" \
    -pkeyopt rsa_padding_mode:oaep \
    -pkeyopt rsa_oaep_md:sha256 \
    -pkeyopt rsa_mgf1_md:sha256 \
    2>/dev/null
)"
test -n "$owner_token"
[[ "$owner_token" != *$'\n'* && "$owner_token" != *$'\r'* && "$owner_token" != *$'\t'* && "$owner_token" != *' '* ]]
[[ "$owner_token" == ghp_* || "$owner_token" == github_pat_* ]]
echo "::add-mask::$owner_token"
export GH_TOKEN="$owner_token"

stage="verify-owner-and-organizations"
OWNER_TOKEN="$owner_token" python3 - <<'PY'
import json
import os
import urllib.error
import urllib.request

token = os.environ["OWNER_TOKEN"]
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {token}",
    "User-Agent": "canonical-test-fleet-sonus-ci",
    "X-GitHub-Api-Version": "2022-11-28",
}

def get(path):
    request = urllib.request.Request("https://api.github.com" + path, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read(2048).decode(errors="replace")
        raise SystemExit(f"GitHub owner preflight failed for {path}: HTTP {error.code}: {detail}") from error

identity = get("/user")
if identity.get("login") != "ORESoftware":
    raise SystemExit(f"unexpected owner identity {identity.get('login')!r}")
organizations = (
    "zed-pkg-test", "3fa-app-test", "declarative-migrations-test", "cliptown-test",
    "claritas-viz-test", "embedded-alerts-test", "evento-globolo-test", "fiducia-cloud-test",
    "memebank-test", "opto-sync-test", "quaestor-ledger-test", "sonus-auris-test",
    "messaging-intel-test", "scintilla-run-test", "file-tunnel-test", "shared-auth-test",
    "hypesiege-test", "streempilot-test",
)
for organization in organizations:
    membership = get(f"/user/memberships/orgs/{organization}")
    observed = (membership.get("state"), membership.get("role"))
    if observed != ("active", "admin"):
        raise SystemExit(f"{organization} membership is {observed!r}")
    print(f"OWNER_VERIFIED {organization}")
PY

stage="cancel-redundant-publishers"
OWNER_TOKEN="$owner_token" python3 - <<'PY'
import os
import time
import urllib.error
import urllib.request

token = os.environ["OWNER_TOKEN"]
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {token}",
    "Content-Length": "0",
    "User-Agent": "canonical-test-fleet-sonus-ci",
    "X-GitHub-Api-Version": "2022-11-28",
}
targets = (
    ("zed-pkg-test/zed-pkg-e2e", 30974844610),
    ("ORESoftware/k8s-cluster", 30974029929),
    ("ORESoftware/k8s-cluster", 30973589779),
)
for repository, run_id in targets:
    request = urllib.request.Request(
        f"https://api.github.com/repos/{repository}/actions/runs/{run_id}/cancel",
        data=b"",
        method="POST",
        headers=headers,
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            print(f"CANCEL_REQUESTED {repository} {run_id} HTTP {response.status}")
    except urllib.error.HTTPError as error:
        if error.code not in (404, 409):
            raise
        print(f"CANCEL_NOT_NEEDED {repository} {run_id} HTTP {error.code}")

time.sleep(10)
PY

stage="validate-canonical-contract"
cd "$canonical"
node --check scripts/bootstrap-test-org-fleet.mjs
node --check scripts/bootstrap-test-org-fleet-worktree-safe.mjs
node --check scripts/list-test-org-fleet-repositories.mjs
node scripts/validate-test-org-fleet.mjs
node --test \
  tests/test-org-fleet.test.mjs \
  tests/bootstrap-retry-policy.test.mjs \
  tests/paced-fleet-executor.test.mjs

organizations=(
  fiducia-cloud-test
  evento-globolo-test
  opto-sync-test
  quaestor-ledger-test
  memebank-test
  scintilla-run-test
  file-tunnel-test
  shared-auth-test
  hypesiege-test
  streempilot-test
  sonus-auris-test
  messaging-intel-test
  3fa-app-test
  declarative-migrations-test
  cliptown-test
  claritas-viz-test
  embedded-alerts-test
)
total=0
for organization in "${organizations[@]}"; do
  count="$(node scripts/list-test-org-fleet-repositories.mjs --org "$organization" --json | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s).repositoryCount))')"
  total=$((total + count))
done
test "$total" -eq 287
printf 'CANONICAL_PLAN organizations=%s specialized_repositories=%s\n' "${#organizations[@]}" "$total"

summary_writes() {
  node - "$1" <<'JS'
const fs = require('node:fs');
const value = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
console.log((value.created ?? 0) + (value.committed ?? 0) + (value.pullRequests ?? 0));
JS
}

apply_repository() {
  local organization="$1"
  local repository="$2"
  local summary="$work/${organization}-${repository}.json"
  local attempt
  for attempt in 1 2 3; do
    rm -f "$summary"
    if TEST_ORG_FLEET_SKIP_TOPICS=true node scripts/bootstrap-test-org-fleet-worktree-safe.mjs \
        --apply \
        --org "$organization" \
        --repo "$repository" \
        --concurrency 1 \
        --summary-json > "$summary"; then
      local writes
      writes="$(summary_writes "$summary")"
      printf 'REPOSITORY_COMPLETE organization=%s repository=%s writes=%s\n' "$organization" "$repository" "$writes"
      if test "$writes" -gt 0; then sleep 15; else sleep 2; fi
      return 0
    fi
    if test "$attempt" -lt 3; then sleep $((attempt * 90)); fi
  done
  return 1
}

finalize_organization() {
  local organization="$1"
  local summary="$work/${organization}-final.json"
  local attempt
  for attempt in 1 2 3; do
    rm -f "$summary"
    if TEST_ORG_FLEET_SKIP_TOPICS=true node scripts/bootstrap-test-org-fleet-worktree-safe.mjs \
        --apply \
        --org "$organization" \
        --concurrency 1 \
        --summary-json > "$summary"; then
      local writes
      writes="$(summary_writes "$summary")"
      printf 'ORGANIZATION_COMPLETE organization=%s writes=%s\n' "$organization" "$writes"
      if test "$writes" -gt 0; then sleep 15; fi
      return 0
    fi
    if test "$attempt" -lt 3; then sleep $((attempt * 90)); fi
  done
  return 1
}

stage="paced-canonical-publication"
processed=0
for organization in "${organizations[@]}"; do
  printf 'ORGANIZATION_START %s\n' "$organization"
  mapfile -t repositories < <(node scripts/list-test-org-fleet-repositories.mjs --org "$organization")
  for repository in "${repositories[@]}"; do
    test -n "$repository"
    apply_repository "$organization" "$repository"
    processed=$((processed + 1))
  done
  finalize_organization "$organization"
  printf 'ORGANIZATION_VERIFIED %s specialized=%s processed_total=%s\n' "$organization" "${#repositories[@]}" "$processed"
done
test "$processed" -eq 287

stage="run-exact-live-audit"
audit="$work/audit"
git init "$audit" >/dev/null
git -C "$audit" remote add origin "https://github.com/${canonical_repository}.git"
git -C "$audit" fetch --depth=1 origin "$audit_sha" >/dev/null
git -C "$audit" checkout --detach FETCH_HEAD >/dev/null
test "$(git -C "$audit" rev-parse HEAD)" = "$audit_sha"
cp "$canonical/bootstrap/test-org-fleet.json.gz" "$audit/bootstrap/test-org-fleet.json.gz"
cd "$audit"
node --check scripts/audit-test-org-fleet.mjs
node --check scripts/audit-test-org-fleet-live.mjs
node scripts/audit-test-org-fleet-live.mjs --plan-json > "$work/audit-plan.json"
node --test tests/audit-test-org-fleet.test.mjs
GH_TOKEN="$owner_token" node scripts/audit-test-org-fleet-live.mjs \
  --output "$work/live-audit.json" \
  --markdown "$work/live-audit.md"

OWNER_TOKEN="$owner_token" AUDIT_FILE="$work/live-audit.json" python3 - <<'PY'
import json
import os

report = json.load(open(os.environ["AUDIT_FILE"], encoding="utf-8"))
summary = report["summary"]
required = {
    "organizations": 18,
    "expectedRepositories": 341,
    "expectedGeneratedRepositories": 319,
    "expectedRetainedRepositories": 22,
    "verifiedRepositories": 341,
    "verifiedGeneratedRepositories": 319,
    "openGeneratedPullRequests": 319,
    "errors": 0,
}
for key, expected in required.items():
    observed = summary.get(key)
    if observed != expected:
        raise SystemExit(f"audit mismatch {key}: expected {expected}, observed {observed}")
print("AUDIT_EXACT_PASS " + json.dumps(summary, sort_keys=True))
PY

stage="record-success"
success="$work/success.md"
summary_json="$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["summary"], sort_keys=True))' "$work/live-audit.json")"
cat > "$success" <<EOF
Canonical paired test-organization fleet publication completed successfully on the isolated \`sonus-ci\` runner.

- Test organizations: **18**
- Expected physical repositories: **341**
- Generated/governance repositories verified: **319**
- Retained Zed fixtures verified: **22**
- Open generated draft pull requests verified: **319**
- Audit errors: **0**
- Explicitly excluded: \`r2g\`, \`r2g-test\`

Audit summary: \`$summary_json\`
EOF
post_control_comment "$success" >/dev/null
printf 'CANONICAL_TEST_FLEET_SUCCESS %s\n' "$summary_json"
