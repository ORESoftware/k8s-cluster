#!/usr/bin/env bash
set -Eeuo pipefail

python3 - <<'PY'
import json
import os
import urllib.error
import urllib.parse
import urllib.request

repository = os.environ["CONTROL_REPOSITORY"]
branch = os.environ["CONTROL_BRANCH"]
token = os.environ["CONTROL_TOKEN"]
base = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {token}",
    "Content-Type": "application/json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "den-2797-gated-e2e-publisher",
}
paths = [
    os.environ["WORKFLOW_PATH"],
    "scripts/den-2797-gated-e2e-prepare.sh",
    "scripts/den-2797-gated-e2e-receive.sh",
    "scripts/den-2797-gated-e2e-publish.sh",
    "scripts/den-2797-gated-e2e-finalize.sh",
    "scripts/den-2797-gated-e2e-scrub.sh",
    os.environ["PUBLIC_KEY_PATH"],
    os.environ["CIPHERTEXT_PATH"],
]
for path in paths:
    encoded_path = urllib.parse.quote(path, safe="/")
    url = f"{base}/repos/{repository}/contents/{encoded_path}?ref={branch}"
    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request) as response:
            metadata = json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            continue
        raise
    payload = json.dumps({
        "message": f"chore(DEN-2797): remove one-shot gated E2E control file {path}",
        "sha": metadata["sha"],
        "branch": branch,
    }).encode("utf-8")
    delete_request = urllib.request.Request(
        url.split("?", 1)[0],
        data=payload,
        method="DELETE",
        headers=headers,
    )
    with urllib.request.urlopen(delete_request) as response:
        if response.status != 200:
            raise SystemExit(f"cleanup returned HTTP {response.status} for {path}")
PY

echo "one-shot control files removed; evidence retained"
