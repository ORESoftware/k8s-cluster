#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

: "${GH_TOKEN:?GitHub Actions token is required}"
: "${GITHUB_REPOSITORY:?repository context is required}"

tracking_issue="${TRACKING_ISSUE:-1175}"
private_key="$(mktemp)"
public_key="$(mktemp)"
ciphertext="$(mktemp)"
token_file="$(mktemp)"

cleanup() {
  unset OWNER_TOKEN
  rm -f "$private_key" "$public_key" "$ciphertext" "$token_file"
}
trap cleanup EXIT

nonce="$(openssl rand -hex 24)"
[[ "$nonce" =~ ^[0-9a-f]{48}$ ]]
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out "$private_key" 2>/dev/null
chmod 600 "$private_key"
openssl pkey -in "$private_key" -pubout -out "$public_key" 2>/dev/null
public_key_b64="$(base64 --wrap=0 "$public_key")"

gh issue comment "$tracking_issue" --repo "$GITHUB_REPOSITORY" --body \
  "Ephemeral test-org bootstrap handoff ready. The private key exists only in this active runner.\n\nNonce: \`${nonce}\`\n\nPublic key (base64-encoded PEM):\n\n\`\`\`text\n${public_key_b64}\n\`\`\`\n\nAccepted response: \`/agent-test-bootstrap-ciphertext ${nonce} <base64-ciphertext>\`" >/dev/null

response=''
for _ in $(seq 1 420); do
  comments="$(gh api "repos/${GITHUB_REPOSITORY}/issues/${tracking_issue}/comments?per_page=100")"
  response="$(jq -r --arg nonce "$nonce" '
    [
      .[]
      | select(.user.login == "ORESoftware")
      | .body
      | select(startswith("/agent-test-bootstrap-ciphertext " + $nonce + " "))
    ]
    | last // empty
  ' <<<"$comments")"
  test -z "$response" || break
  sleep 2
done
test -n "$response"

prefix="/agent-test-bootstrap-ciphertext ${nonce} "
encoded_ciphertext="${response#"$prefix"}"
test "$encoded_ciphertext" != "$response"
[[ "$encoded_ciphertext" =~ ^[A-Za-z0-9+/=]+$ ]]
test "${#encoded_ciphertext}" -le 8192
printf '%s' "$encoded_ciphertext" | base64 --decode > "$ciphertext"

openssl pkeyutl -decrypt \
  -inkey "$private_key" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -pkeyopt rsa_mgf1_md:sha256 \
  -in "$ciphertext" \
  -out "$token_file" 2>/dev/null
chmod 600 "$token_file"

owner_token="$(cat "$token_file")"
test -n "$owner_token"
test "${#owner_token}" -le 1024
case "$owner_token" in
  ghp_*|github_pat_*) ;;
  *) echo 'decrypted value is not a supported GitHub owner token' >&2; exit 65 ;;
esac
case "$owner_token" in
  *$'\n'*|*$'\r'*|*$'\t'*|*' '*)
    echo 'decrypted token contains invalid whitespace' >&2
    exit 65
    ;;
esac
printf '::add-mask::%s\n' "$owner_token"
export OWNER_TOKEN="$owner_token"
unset owner_token

python3 - <<'PY'
import json
import os
import urllib.error
import urllib.request

token = os.environ["OWNER_TOKEN"]
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {token}",
    "User-Agent": "agent-test-org-bootstrap",
    "X-GitHub-Api-Version": "2022-11-28",
}


def request(method: str, path: str, body=None, allow_404: bool = False):
    payload = None if body is None else json.dumps(body).encode("utf-8")
    req = urllib.request.Request(
        "https://api.github.com" + path,
        data=payload,
        headers={
            **headers,
            **({"Content-Type": "application/json"} if payload else {}),
        },
        method=method,
    )
    try:
        with urllib.request.urlopen(req, timeout=45) as response:
            raw = response.read()
    except urllib.error.HTTPError as error:
        error.read(8192)
        if allow_404 and error.code == 404:
            return None
        raise SystemExit(
            f"GitHub API failed: {method} {path} HTTP {error.code}"
        ) from error
    return {} if not raw else json.loads(raw)


identity = request("GET", "/user")
if identity.get("login") != "ORESoftware":
    raise SystemExit("owner credential identity mismatch")

for organization in ("agent-pontifex-test", "fiducia-cloud-test"):
    membership = request("GET", f"/user/memberships/orgs/{organization}")
    if (membership.get("role"), membership.get("state")) != ("admin", "active"):
        raise SystemExit(f"missing active admin membership in {organization}")

repositories = [
    {
        "organization": "agent-pontifex-test",
        "name": "agent-conformance",
        "visibility": "public",
        "description": "Independent black-box conformance and compatibility tests for Agent Pontifex servers and SDKs.",
        "topics": ["agent-pontifex", "conformance", "e2e", "rust", "ai-agents"],
    },
    {
        "organization": "fiducia-cloud-test",
        "name": "fiducia-agent-conformance",
        "visibility": "private",
        "description": "Independent Fiducia bridge and coordinator conformance, fencing, tenancy, and failure-mode certification.",
        "topics": ["fiducia", "conformance", "e2e", "fencing", "ai-agents"],
    },
]

for spec in repositories:
    full_name = f"{spec['organization']}/{spec['name']}"
    existing = request("GET", f"/repos/{full_name}", allow_404=True)
    if existing is None:
        request(
            "POST",
            f"/orgs/{spec['organization']}/repos",
            {
                "name": spec["name"],
                "description": spec["description"],
                "visibility": spec["visibility"],
                "private": spec["visibility"] == "private",
                "auto_init": True,
                "has_issues": True,
                "has_projects": True,
                "has_wiki": False,
            },
        )
        action = "created"
    else:
        action = "existing"

    request(
        "PATCH",
        f"/repos/{full_name}",
        {
            "description": spec["description"],
            "default_branch": "main",
            "delete_branch_on_merge": True,
            "allow_squash_merge": True,
            "allow_merge_commit": True,
            "allow_rebase_merge": True,
            "has_issues": True,
            "has_projects": True,
            "has_wiki": False,
        },
    )
    request("PUT", f"/repos/{full_name}/topics", {"names": spec["topics"]})

    verified = request("GET", f"/repos/{full_name}")
    if verified.get("visibility") != spec["visibility"]:
        raise SystemExit(f"visibility mismatch for {full_name}")
    if verified.get("default_branch") != "main":
        raise SystemExit(f"default branch mismatch for {full_name}")
    if verified.get("archived") is not False or verified.get("disabled") is not False:
        raise SystemExit(f"repository is not active: {full_name}")
    print(
        f"repository={full_name} action={action} "
        f"visibility={spec['visibility']} default=main"
    )

print("bootstrap status=success repositories=2")
PY

gh issue comment "$tracking_issue" --repo "$GITHUB_REPOSITORY" --body \
  "Created and verified the independent test repositories in \`agent-pontifex-test\` and \`fiducia-cloud-test\`. Run: https://github.com/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}" >/dev/null
