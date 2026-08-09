#!/usr/bin/env bash
set -Eeuo pipefail

work=/tmp/den-2797-gated-e2e
manifest="$work/manifest.json"
token_file="$work/owner.token"
plan="$work/publish-plan.tsv"
askpass="$work/git-askpass.sh"
evidence="$work/evidence.json"

test -s "$manifest"
test -s "$token_file"
chmod 600 "$token_file"

OWNER_TOKEN_FILE="$token_file" MANIFEST_PATH="$manifest" PLAN_PATH="$plan" python3 - <<'PY'
import json
import os
import urllib.error
import urllib.request

with open(os.environ["OWNER_TOKEN_FILE"], "r", encoding="utf-8") as handle:
    token = handle.read()
with open(os.environ["MANIFEST_PATH"], "r", encoding="utf-8") as handle:
    manifest = json.load(handle)
base = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {token}",
    "Content-Type": "application/json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "den-2797-gated-e2e-publisher",
}

def request(method, path, payload=None, expected=()):
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(f"{base}{path}", data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req) as response:
            body = response.read()
            parsed = json.loads(body) if body else None
            if expected and response.status not in expected:
                raise SystemExit(f"unexpected HTTP {response.status} for {method} {path}")
            return response.status, parsed
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", errors="replace")
        if error.code in expected:
            try:
                parsed = json.loads(body) if body else None
            except json.JSONDecodeError:
                parsed = None
            return error.code, parsed
        message = ""
        try:
            message = json.loads(body).get("message", "")
        except Exception:
            message = body[:200]
        raise SystemExit(f"GitHub API failure for {method} {path}: HTTP {error.code}: {message}")

rows = []
for target in manifest["targets"]:
    repository = target["repository"]
    owner, name = repository.split("/", 1)
    status, existing = request("GET", f"/repos/{repository}", expected=(200, 404))
    disposition = "push"
    if status == 404:
        create_payload = {
            "name": name,
            "description": f"Canonical E2E harness for {owner}",
            "private": False,
            "has_issues": True,
            "has_projects": True,
            "has_wiki": False,
            "auto_init": False,
        }
        created_status, _ = request(
            "POST",
            f"/orgs/{owner}/repos",
            create_payload,
            expected=(201,),
        )
        if created_status != 201:
            raise SystemExit(f"repository creation failed for {repository}")
    else:
        ref_status, ref = request(
            "GET",
            f"/repos/{repository}/git/ref/heads/main",
            expected=(200, 404, 409),
        )
        if ref_status == 200:
            remote_head = ref["object"]["sha"]
            if remote_head != target["head"]:
                raise SystemExit(
                    f"refusing divergent existing repository {repository}: "
                    f"expected {target['head']}, got {remote_head}"
                )
            disposition = "already-exact"
        elif int(existing.get("size", 0)) == 0:
            disposition = "push-empty"
        else:
            raise SystemExit(f"refusing nonempty repository without main: {repository}")
    rows.append((repository, target["directory"], target["head"], disposition))

with open(os.environ["PLAN_PATH"], "w", encoding="utf-8") as handle:
    for row in rows:
        handle.write("\t".join(row) + "\n")
PY

cat >"$askpass" <<'EOF'
#!/usr/bin/env sh
case "$1" in
  *Username*) printf '%s\n' "x-access-token" ;;
  *Password*) cat "${DEN2797_OWNER_TOKEN_FILE:?DEN2797_OWNER_TOKEN_FILE is required}" ;;
  *) exit 1 ;;
esac
EOF
chmod 700 "$askpass"

while IFS=$'\t' read -r repository directory expected_head disposition; do
  test -d "$directory/.git"
  actual_head="$(git -C "$directory" rev-parse HEAD)"
  if [[ "$actual_head" != "$expected_head" ]]; then
    echo "local target moved for $repository: expected $expected_head, got $actual_head" >&2
    exit 41
  fi
  if [[ "$disposition" == already-exact ]]; then
    echo "$repository already has exact main $expected_head"
    continue
  fi

  if git -C "$directory" remote get-url origin >/dev/null 2>&1; then
    existing_origin="$(git -C "$directory" remote get-url origin)"
    if [[ "$existing_origin" != "https://github.com/$repository.git" ]]; then
      echo "refusing unrelated origin for $repository: $existing_origin" >&2
      exit 42
    fi
  else
    git -C "$directory" remote add origin "https://github.com/$repository.git"
  fi

  DEN2797_OWNER_TOKEN_FILE="$token_file" \
  GIT_TERMINAL_PROMPT=0 \
  git -C "$directory" \
    -c credential.helper= \
    -c core.askPass="$askpass" \
    push --set-upstream origin main

done <"$plan"

OWNER_TOKEN_FILE="$token_file" MANIFEST_PATH="$manifest" EVIDENCE_PATH_LOCAL="$evidence" python3 - <<'PY'
import json
import os
import urllib.error
import urllib.request

with open(os.environ["OWNER_TOKEN_FILE"], "r", encoding="utf-8") as handle:
    token = handle.read()
with open(os.environ["MANIFEST_PATH"], "r", encoding="utf-8") as handle:
    manifest = json.load(handle)
base = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {token}",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "den-2797-gated-e2e-publisher",
}
verified = []
for target in manifest["targets"]:
    repository = target["repository"]
    request = urllib.request.Request(
        f"{base}/repos/{repository}/git/ref/heads/main",
        headers=headers,
    )
    with urllib.request.urlopen(request) as response:
        payload = json.load(response)
    remote_head = payload["object"]["sha"]
    if remote_head != target["head"]:
        raise SystemExit(
            f"remote verification failed for {repository}: "
            f"expected {target['head']}, got {remote_head}"
        )
    verified.append({
        "repository": repository,
        "head": remote_head,
        "source_repository": target["source_repository"],
        "source_pull": target["source_pull"],
        "source_sha": target["source_sha"],
        "history_disposition": target["history_disposition"],
        "visibility": "public",
        "default_branch": "main",
    })

evidence = {
    "schema": 1,
    "linear": ["DEN-2797", "DEN-2031", "DEN-2288"],
    "control_repository": os.environ["CONTROL_REPOSITORY"],
    "control_commit": os.environ["GITHUB_SHA"],
    "workflow_run_id": os.environ.get("GITHUB_RUN_ID"),
    "workflow_run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
    "publication_policy": {
        "force_push": False,
        "history_rewrite": False,
        "plaintext_credential_persisted": False,
        "source_validation_before_credential": True,
        "existing_divergent_repository_refused": True,
    },
    "targets": verified,
}
with open(os.environ["EVIDENCE_PATH_LOCAL"], "w", encoding="utf-8") as handle:
    json.dump(evidence, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY

CONTROL_FILE_PATH="$EVIDENCE_PATH" \
CONTROL_FILE_SOURCE="$evidence" \
CONTROL_FILE_MESSAGE="docs(DEN-2797): record gated E2E publication evidence" \
python3 - <<'PY'
import base64
import json
import os
import urllib.error
import urllib.request

repository = os.environ["CONTROL_REPOSITORY"]
branch = os.environ["CONTROL_BRANCH"]
path = os.environ["CONTROL_FILE_PATH"]
source = os.environ["CONTROL_FILE_SOURCE"]
token = os.environ["CONTROL_TOKEN"]
message = os.environ["CONTROL_FILE_MESSAGE"]
base = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
url = f"{base}/repos/{repository}/contents/{path}"
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {token}",
    "Content-Type": "application/json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "den-2797-gated-e2e-publisher",
}
with open(source, "rb") as handle:
    raw = handle.read()
encoded = base64.b64encode(raw).decode("ascii")
payload = {"message": message, "content": encoded, "branch": branch}

check = urllib.request.Request(url, headers=headers)
try:
    with urllib.request.urlopen(check) as response:
        existing = json.load(response)
    existing_raw = base64.b64decode(existing["content"])
    if existing_raw != raw:
        raise SystemExit(f"refusing to replace differing evidence file: {path}")
except urllib.error.HTTPError as error:
    if error.code != 404:
        raise
    body = json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(url, data=body, method="PUT", headers=headers)
    with urllib.request.urlopen(request) as response:
        if response.status not in (200, 201):
            raise SystemExit(f"evidence publication returned HTTP {response.status}")
PY

echo "verified exact publication of all gated E2E targets"
