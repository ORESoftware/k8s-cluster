#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

remote_stage="bootstrap"
work=""
askpass=""
on_error() {
  local status=$?
  trap - ERR
  printf 'MCP_PUBLISHER_ERROR stage=%s code=%d\n' "$remote_stage" "$status"
  exit "$status"
}
cleanup() {
  unset GH_TOKEN GITHUB_TOKEN GITHUB_REPOSITORY_ADMIN_TOKEN encoded_pat
  unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT
  if test -n "$askpass"; then
    rm -f "$askpass"
  fi
  if test -n "$work"; then
    rm -rf "$work"
  fi
  rm -f /tmp/requested-mcp-publication.json
}
trap on_error ERR
trap cleanup EXIT

trusted_sha="${1:?trusted SHA required}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]
work="$(mktemp -d /tmp/requested-mcp-publisher.XXXXXX)"

remote_stage="receive-protected-credential"
IFS= read -r encoded_pat
test -n "$encoded_pat"
GH_TOKEN="$(printf '%s' "$encoded_pat" | base64 --decode)"
unset encoded_pat
test -n "$GH_TOKEN"
[[ "$GH_TOKEN" != *[[:space:]]* ]]
export GH_TOKEN
export GITHUB_REPOSITORY_ADMIN_TOKEN="$GH_TOKEN"

# One-shot, idempotent membership reconciliation requested on 2026-08-23.
# The credential is supplied only through the protected broker's RSA-OAEP
# envelope, is never placed in argv or output, and is removed by cleanup.
remote_stage="invite-the1mills"
python3 - <<'PY'
from __future__ import annotations

import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any

API = "https://api.github.com"
AUTHENTICATED_OWNER = "ORESoftware"
TARGET = "the1mills"
AUDIT_REPOSITORY = "ORESoftware/k8s-cluster"
AUDIT_ISSUE = 1413
TOKEN = os.environ.get("GH_TOKEN", "")

if not TOKEN or any(ch.isspace() for ch in TOKEN):
    raise SystemExit("bounded GitHub credential is missing or malformed")

HEADERS = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {TOKEN}",
    "Content-Type": "application/json",
    "User-Agent": "oresoftware-one-shot-org-membership-reconciler/1.0",
    "X-GitHub-Api-Version": "2022-11-28",
}


@dataclass(frozen=True)
class ApiResult:
    status: int
    payload: Any
    message: str


def safe_message(payload: Any) -> str:
    if isinstance(payload, dict):
        message = payload.get("message")
        if isinstance(message, str):
            return " ".join(message.split())[:180]
    return ""


def api(method: str, path: str, body: Any | None = None) -> ApiResult:
    data = None
    if body is not None:
        data = json.dumps(body, separators=(",", ":")).encode("utf-8")
    url = f"{API}{path}"
    for attempt in range(5):
        request = urllib.request.Request(
            url,
            data=data,
            headers=HEADERS,
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=45) as response:
                raw = response.read(2_000_000)
                payload = json.loads(raw) if raw else None
                return ApiResult(response.status, payload, safe_message(payload))
        except urllib.error.HTTPError as error:
            raw = error.read(64_000)
            try:
                payload = json.loads(raw) if raw else None
            except json.JSONDecodeError:
                payload = None
            message = safe_message(payload)
            retryable = error.code in {429, 500, 502, 503, 504}
            if error.code == 403 and (
                "secondary rate limit" in message.lower()
                or "rate limit exceeded" in message.lower()
            ):
                retryable = True
            if retryable and attempt < 4:
                retry_after = error.headers.get("Retry-After")
                try:
                    delay = min(max(float(retry_after or 0), 1.0), 30.0)
                except ValueError:
                    delay = min(2.0 ** attempt, 30.0)
                time.sleep(delay)
                continue
            return ApiResult(error.code, payload, message)
        except (TimeoutError, urllib.error.URLError) as error:
            if attempt < 4:
                time.sleep(min(2.0 ** attempt, 20.0))
                continue
            return ApiResult(0, None, type(error).__name__)
    return ApiResult(0, None, "retry budget exhausted")


def quote(value: str) -> str:
    return urllib.parse.quote(value, safe="")


actor = api("GET", "/user")
if actor.status != 200 or not isinstance(actor.payload, dict):
    raise SystemExit(f"unable to authenticate supplied GitHub credential: HTTP {actor.status}")
actor_login = actor.payload.get("login")
if not isinstance(actor_login, str) or actor_login.casefold() != AUTHENTICATED_OWNER.casefold():
    raise SystemExit(
        f"credential belongs to {actor_login!r}, expected {AUTHENTICATED_OWNER!r}"
    )

target = api("GET", f"/users/{quote(TARGET)}")
if target.status != 200:
    raise SystemExit(f"target GitHub account {TARGET!r} was not found: HTTP {target.status}")

owned_orgs: set[str] = set()
page = 1
while True:
    memberships = api(
        "GET",
        f"/user/memberships/orgs?state=active&per_page=100&page={page}",
    )
    if memberships.status != 200 or not isinstance(memberships.payload, list):
        raise SystemExit(
            "unable to enumerate authenticated organization memberships: "
            f"HTTP {memberships.status} {memberships.message}"
        )
    if not memberships.payload:
        break
    for membership in memberships.payload:
        if not isinstance(membership, dict):
            continue
        organization = membership.get("organization")
        login = organization.get("login") if isinstance(organization, dict) else None
        if (
            isinstance(login, str)
            and membership.get("state") == "active"
            and membership.get("role") == "admin"
        ):
            owned_orgs.add(login)
    if len(memberships.payload) < 100:
        break
    page += 1

rows: list[dict[str, Any]] = []
for organization in sorted(owned_orgs, key=str.casefold):
    membership_path = (
        f"/orgs/{quote(organization)}/memberships/{quote(TARGET)}"
    )
    current = api("GET", membership_path)
    if current.status == 200 and isinstance(current.payload, dict):
        state = current.payload.get("state")
        if state == "active":
            action = "already_member"
        elif state == "pending":
            action = "already_pending"
        else:
            action = "unexpected_membership_state"
        rows.append(
            {
                "organization": organization,
                "action": action,
                "state": state or "unknown",
                "http": current.status,
                "detail": "",
            }
        )
    elif current.status == 404:
        invited = api("PUT", membership_path, {"role": "member"})
        state = (
            invited.payload.get("state")
            if isinstance(invited.payload, dict)
            else None
        )
        rows.append(
            {
                "organization": organization,
                "action": "invited" if invited.status == 200 else "failed",
                "state": state or "unknown",
                "http": invited.status,
                "detail": invited.message,
            }
        )
    else:
        rows.append(
            {
                "organization": organization,
                "action": "failed",
                "state": "unknown",
                "http": current.status,
                "detail": current.message,
            }
        )
    time.sleep(0.12)

counts = {
    "invited": sum(row["action"] == "invited" for row in rows),
    "already_member": sum(row["action"] == "already_member" for row in rows),
    "already_pending": sum(row["action"] == "already_pending" for row in rows),
    "failed": sum(
        row["action"] not in {"invited", "already_member", "already_pending"}
        for row in rows
    ),
}

summary = [
    "### One-shot organization invitation ledger",
    "",
    f"- Authenticated owner: `{actor_login}`",
    f"- Target account: `{TARGET}`",
    f"- Active owned organizations discovered: **{len(rows)}**",
    f"- New invitations created: **{counts['invited']}**",
    f"- Existing active memberships: **{counts['already_member']}**",
    f"- Existing pending invitations: **{counts['already_pending']}**",
    f"- Failures or unexpected states: **{counts['failed']}**",
    "",
]

table_header = [
    "| Organization | Result | State | HTTP | Detail |",
    "|---|---|---:|---:|---|",
]
table_rows = []
for row in rows:
    detail = str(row["detail"]).replace("|", "\\|")
    table_rows.append(
        f"| `{row['organization']}` | `{row['action']}` | "
        f"`{row['state']}` | `{row['http']}` | {detail} |"
    )

# Split the ledger so every GitHub comment remains comfortably below the
# documented body-size ceiling while preserving one row per organization.
chunks: list[str] = []
current_lines = summary + table_header
for line in table_rows:
    candidate = "\n".join(current_lines + [line])
    if len(candidate.encode("utf-8")) > 50_000 and len(current_lines) > len(summary) + 2:
        chunks.append("\n".join(current_lines))
        current_lines = [
            "### One-shot organization invitation ledger — continued",
            "",
            *table_header,
            line,
        ]
    else:
        current_lines.append(line)
chunks.append("\n".join(current_lines))

audit_failures: list[str] = []
for index, comment_body in enumerate(chunks, start=1):
    posted = api(
        "POST",
        f"/repos/{AUDIT_REPOSITORY}/issues/{AUDIT_ISSUE}/comments",
        {"body": comment_body},
    )
    if posted.status != 201:
        audit_failures.append(
            f"part {index}: HTTP {posted.status} {posted.message}".strip()
        )

print(
    "ORG_INVITE_SUMMARY "
    f"owner={actor_login} target={TARGET} owned={len(rows)} "
    f"invited={counts['invited']} active={counts['already_member']} "
    f"pending={counts['already_pending']} failed={counts['failed']} "
    f"audit_parts={len(chunks)} audit_failures={len(audit_failures)}"
)

if audit_failures:
    print("ORG_INVITE_AUDIT_ERROR " + "; ".join(audit_failures))
if counts["failed"]:
    raise SystemExit(71)
PY

remote_stage="unprivileged-prerequisites"
command -v git >/dev/null
command -v python3 >/dev/null

# The trusted source repository is private. GH_TOKEN is not automatically used
# by Git, so install an exact, temporary askpass helper before fetching the
# pinned k8s-cluster commit. The credential remains in the environment; it is
# never embedded in the remote URL, argv, config, or command output.
remote_stage="trusted-source-auth"
askpass="$work/github-askpass.sh"
cat > "$askpass" <<'ASKPASS'
#!/usr/bin/env sh
case "${1:-}" in
  *Username*) printf '%s\n' x-access-token ;;
  *Password*) printf '%s\n' "${GH_TOKEN:?GH_TOKEN required}" ;;
  *) exit 1 ;;
esac
ASKPASS
chmod 0700 "$askpass"
export GIT_ASKPASS="$askpass"
export GIT_ASKPASS_REQUIRE=force
export GIT_TERMINAL_PROMPT=0

remote_stage="trusted-source-checkout"
git init "$work/k8s-cluster" >/dev/null
git -C "$work/k8s-cluster" remote add origin https://github.com/ORESoftware/k8s-cluster.git
git -C "$work/k8s-cluster" fetch --quiet --depth=1 origin "$trusted_sha"
git -C "$work/k8s-cluster" checkout --quiet --detach FETCH_HEAD
test "$(git -C "$work/k8s-cluster" rev-parse HEAD)" = "$trusted_sha"
rm -f "$askpass"
askpass=""
unset GIT_ASKPASS GIT_ASKPASS_REQUIRE GIT_TERMINAL_PROMPT

remote_stage="publisher-contract-tests"
cd "$work/k8s-cluster"
python3 -m py_compile \
  scripts/ops/publish_requested_mcp_servers.py \
  scripts/ops/requested_mcp_publisher/*.py
python3 -m unittest -v scripts/ops/tests/test_publish_requested_mcp_servers.py

remote_stage="github-preflight-and-publication"
python3 scripts/ops/publish_requested_mcp_servers.py \
  --execute \
  --report /tmp/requested-mcp-publication.json

remote_stage="publication-report-validation"
python3 - <<'PY'
import json
from pathlib import Path

report = json.loads(Path('/tmp/requested-mcp-publication.json').read_text())
rows = report.get('repositories')
if not isinstance(rows, list) or len(rows) != 5:
    raise SystemExit('publication report must contain exactly five repositories')
for row in rows:
    print(
        'MCP_REPOSITORY_VERIFIED '
        f"{row['full_name']} visibility={row['visibility']} "
        f"main={row['current_main_sha']}"
    )
PY
