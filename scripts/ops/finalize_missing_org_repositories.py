#!/usr/bin/env python3
"""Finalize and verify the fixed critical cross-organization repository fleet.

The 32 HypeSiege/StreemPilot repositories are public and must exactly match the
schema-v2 manifest pinned in ``ORESoftware/ai-agent-coordinator.rs``. The two
extracted repositories remain private. Verification is read-first and
fail-closed; CI carrier activation happens only after every non-mutating fleet
preflight succeeds.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import json
import os
import re
import sys
import tempfile
from collections import Counter
from pathlib import Path
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

API = "https://api.github.com"
API_VERSION = "2022-11-28"
FLEET_SOURCE_REPOSITORY = "ORESoftware/ai-agent-coordinator.rs"
FLEET_SOURCE_SHA = "5d9a0c2cb44dff607bc3953954ce4b9af08e5789"
FLEET_MANIFEST_PATH = "repository-fleets/hypesiege-streempilot.json"
EXPECTED_GENERATOR_SHA256 = (
    "a57b00961ee57ae09bf3bb2e2d09afbdd1ddbbbde832b027802f82a1fc5dfa84"
)
EXPECTED_ORGS = {"hypesiege": 15, "streempilot": 17}
REPORT_ORG_KEYS = {"hypesiege": "hypesiege", "streempilot": "StreemPilot"}
EXTRACTED = {
    "meta-agents-demo/meta-agent-control-plane.rs": ".meta-agent-ci.yml.pending",
    "file-tunnel/ftnl-mcp-server.rs": ".ftnl-mcp-ci.yml.pending",
}
TRIGGER_PULLS = (227, 229, 230, 231)
SHA_RE = re.compile(r"[0-9a-f]{40}")
MAX_API_BYTES = 2 * 1024 * 1024


class PublicationError(RuntimeError):
    """Fail-closed publication or verification error."""


def token() -> str:
    value = os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN", "")
    if not value:
        raise PublicationError("GITHUB_REPOSITORY_ADMIN_TOKEN is required")
    if value != value.strip() or any(character.isspace() for character in value):
        raise PublicationError("GITHUB_REPOSITORY_ADMIN_TOKEN contains whitespace")
    return value


def api(
    method: str,
    path: str,
    credential: str,
    payload: dict[str, Any] | None = None,
    *,
    allow_missing: bool = False,
) -> Any:
    body = None
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {credential}",
        "X-GitHub-Api-Version": API_VERSION,
        "User-Agent": "oresoftware-critical-org-publication-finalizer",
    }
    if payload is not None:
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = Request(f"{API}{path}", data=body, headers=headers, method=method)
    try:
        with urlopen(request, timeout=45) as response:
            raw = response.read(MAX_API_BYTES + 1)
            if len(raw) > MAX_API_BYTES:
                raise PublicationError(
                    f"GitHub API {method} {path} response exceeded {MAX_API_BYTES} bytes"
                )
            if not raw:
                return None
            try:
                return json.loads(raw.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                raise PublicationError(
                    f"GitHub API {method} {path} returned invalid JSON"
                ) from exc
    except HTTPError as exc:
        raw = exc.read(4096).decode("utf-8", errors="replace")
        if allow_missing and exc.code == 404:
            return None
        raise PublicationError(
            f"GitHub API {method} {path} failed ({exc.code}): {raw}"
        ) from exc
    except URLError as exc:
        raise PublicationError(
            f"GitHub API {method} {path} was unavailable: {exc.reason}"
        ) from exc


def get_content(
    slug: str,
    path: str,
    credential: str,
    *,
    ref: str = "main",
) -> dict[str, Any] | None:
    result = api(
        "GET",
        (
            f"/repos/{slug}/contents/{quote(path, safe='/')}"
            f"?ref={quote(ref, safe='')}"
        ),
        credential,
        allow_missing=True,
    )
    if result is not None and not isinstance(result, dict):
        raise PublicationError(f"unexpected content response for {slug}:{path}@{ref}")
    return result


def decoded_content(record: dict[str, Any]) -> bytes:
    encoded = record.get("content")
    if not isinstance(encoded, str):
        raise PublicationError("Contents API response lacks inline content")
    try:
        return base64.b64decode(encoded.replace("\n", ""), validate=True)
    except (ValueError, binascii.Error) as exc:
        raise PublicationError("Contents API response contains invalid base64") from exc


def load_fleet_manifest(credential: str) -> dict[str, Any]:
    record = get_content(
        FLEET_SOURCE_REPOSITORY,
        FLEET_MANIFEST_PATH,
        credential,
        ref=FLEET_SOURCE_SHA,
    )
    if record is None:
        raise PublicationError("pinned fleet manifest is missing")
    try:
        manifest = json.loads(decoded_content(record).decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise PublicationError("pinned fleet manifest is not valid UTF-8 JSON") from exc
    if not isinstance(manifest, dict):
        raise PublicationError("pinned fleet manifest must be a JSON object")

    repositories = manifest.get("repositories")
    if manifest.get("schema_version") != 2 or not isinstance(repositories, list):
        raise PublicationError("unsupported or malformed pinned fleet manifest")
    if manifest.get("generator_sha256") != EXPECTED_GENERATOR_SHA256:
        raise PublicationError("pinned fleet generator identity changed")
    if manifest.get("default_branch") != "main":
        raise PublicationError("pinned fleet default branch changed")
    if manifest.get("repository_count") != 32 or len(repositories) != 32:
        raise PublicationError("pinned fleet must contain exactly 32 repositories")
    if manifest.get("total_tracked_files") != 888:
        raise PublicationError("pinned fleet tracked-file total changed")
    if manifest.get("total_gitlinks") != 30:
        raise PublicationError("pinned fleet gitlink total changed")
    if manifest.get("organizations") != EXPECTED_ORGS:
        raise PublicationError("pinned fleet organization counts changed")

    seen: set[str] = set()
    observed_orgs: Counter[str] = Counter()
    for index, repository in enumerate(repositories):
        if not isinstance(repository, dict):
            raise PublicationError(f"fleet record {index} is not an object")
        org = repository.get("org")
        name = repository.get("name")
        full_name = repository.get("full_name")
        if org not in EXPECTED_ORGS or not isinstance(name, str):
            raise PublicationError(
                f"fleet record {index} has an invalid organization/name"
            )
        if (
            not isinstance(full_name, str)
            or full_name.casefold() != f"{org}/{name}".casefold()
        ):
            raise PublicationError(f"fleet record {index} has an inconsistent full_name")
        identity = full_name.casefold()
        if identity in seen:
            raise PublicationError(f"fleet manifest duplicates {full_name}")
        seen.add(identity)
        observed_orgs[org] += 1
        if repository.get("visibility") != "public":
            raise PublicationError(f"{full_name}: fleet visibility must remain public")
        if repository.get("default_branch") != "main":
            raise PublicationError(f"{full_name}: default branch must remain main")
        commit = repository.get("commit")
        if not isinstance(commit, str) or SHA_RE.fullmatch(commit) is None:
            raise PublicationError(f"{full_name}: commit must be a full lowercase SHA")
    if dict(observed_orgs) != EXPECTED_ORGS:
        raise PublicationError("fleet records do not match the approved organization counts")
    return manifest


def org_repositories(org: str, credential: str) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for page in range(1, 20):
        batch = api(
            "GET",
            f"/orgs/{org}/repos?type=all&per_page=100&page={page}",
            credential,
        )
        if not isinstance(batch, list):
            raise PublicationError(f"unexpected repository list for {org}")
        if any(not isinstance(item, dict) for item in batch):
            raise PublicationError(f"repository list for {org} contains a non-object")
        records.extend(batch)
        if len(batch) < 100:
            return records
    raise PublicationError(f"repository pagination for {org} exceeded the safety bound")


def verify_repository(
    slug: str,
    repository: dict[str, Any],
    credential: str,
    *,
    expected_visibility: str,
    expected_sha: str | None = None,
) -> dict[str, Any]:
    if expected_visibility not in {"public", "private"}:
        raise PublicationError(f"{slug}: invalid expected visibility")
    full_name = repository.get("full_name")
    if not isinstance(full_name, str) or full_name.casefold() != slug.casefold():
        raise PublicationError(f"{slug}: GitHub returned an unexpected repository identity")
    repository_id = repository.get("id")
    if not isinstance(repository_id, int) or repository_id <= 0:
        raise PublicationError(f"{slug}: repository ID is missing or invalid")
    expected_owner, expected_name = slug.split("/", 1)
    owner = repository.get("owner")
    if (
        not isinstance(owner, dict)
        or str(owner.get("login", "")).casefold() != expected_owner.casefold()
    ):
        raise PublicationError(f"{slug}: repository owner does not match")
    if str(repository.get("name", "")).casefold() != expected_name.casefold():
        raise PublicationError(f"{slug}: repository name does not match")
    if repository.get("fork") is not False:
        raise PublicationError(f"{slug}: repository must not be a fork")
    if repository.get("archived") is not False:
        raise PublicationError(f"{slug}: repository must not be archived")
    if repository.get("disabled") is not False:
        raise PublicationError(f"{slug}: repository must not be disabled")

    expected_private = expected_visibility == "private"
    if repository.get("private") is not expected_private:
        raise PublicationError(
            f"{slug}: private={repository.get('private')!r}, expected {expected_private!r}"
        )
    if repository.get("visibility") != expected_visibility:
        raise PublicationError(
            f"{slug}: visibility={repository.get('visibility')!r}, "
            f"expected {expected_visibility!r}"
        )
    if repository.get("default_branch") != "main":
        raise PublicationError(
            f"{slug}: default branch is {repository.get('default_branch')!r}, "
            "expected 'main'"
        )

    ref = api("GET", f"/repos/{slug}/git/ref/heads/main", credential)
    sha = ((ref or {}).get("object") or {}).get("sha")
    if not isinstance(sha, str) or SHA_RE.fullmatch(sha) is None:
        raise PublicationError(f"{slug}: main does not resolve to a full commit SHA")
    if expected_sha is not None and sha != expected_sha:
        raise PublicationError(f"{slug}: main {sha} != approved {expected_sha}")
    return {
        "id": repository_id,
        "slug": full_name,
        "visibility": expected_visibility,
        "private": expected_private,
        "default_branch": "main",
        "main_sha": sha,
        "archived": False,
        "disabled": False,
        "fork": False,
    }


def verify_public_fleet(
    manifest: dict[str, Any], credential: str
) -> dict[str, dict[str, Any]]:
    repositories = manifest["repositories"]
    result: dict[str, dict[str, Any]] = {}
    for org, expected_count in EXPECTED_ORGS.items():
        approved = [record for record in repositories if record["org"] == org]
        expected_by_name = {
            str(record["full_name"]).casefold(): record for record in approved
        }
        actual = org_repositories(org, credential)
        actual_by_name = {
            str(repository.get("full_name", "")).casefold(): repository
            for repository in actual
        }
        expected_names = set(expected_by_name)
        actual_names = set(actual_by_name)
        if actual_names != expected_names:
            missing = sorted(expected_names - actual_names)
            unexpected = sorted(actual_names - expected_names)
            raise PublicationError(
                f"{org}: repository inventory differs from the approved fleet; "
                f"missing={missing}, unexpected={unexpected}"
            )
        if len(actual) != expected_count:
            raise PublicationError(
                f"{org}: expected exactly {expected_count} repositories, found {len(actual)}"
            )
        verified = [
            verify_repository(
                str(expected_by_name[identity]["full_name"]),
                actual_by_name[identity],
                credential,
                expected_visibility="public",
                expected_sha=str(expected_by_name[identity]["commit"]),
            )
            for identity in sorted(expected_names)
        ]
        result[REPORT_ORG_KEYS[org]] = {
            "expected": expected_count,
            "count": len(verified),
            "visibility": "public",
            "repositories": verified,
        }
    return result


def inspect_ci_state(
    slug: str, pending_path: str, credential: str
) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    pending = get_content(slug, pending_path, credential)
    current = get_content(slug, ".github/workflows/ci.yml", credential)
    if pending is None and current is None:
        raise PublicationError(f"{slug}: neither staged nor canonical CI exists")
    if pending is not None and current is not None:
        if decoded_content(pending) != decoded_content(current):
            raise PublicationError(f"{slug}: canonical CI conflicts with staged carrier")
    return pending, current


def activate_ci(slug: str, pending_path: str, credential: str) -> str:
    pending, current = inspect_ci_state(slug, pending_path, credential)
    if pending is None:
        return "already-active"
    pending_bytes = decoded_content(pending)
    if current is None:
        api(
            "PUT",
            f"/repos/{slug}/contents/{quote('.github/workflows/ci.yml', safe='/')}",
            credential,
            {
                "message": "ci: activate canonical validation after repository publication",
                "content": base64.b64encode(pending_bytes).decode("ascii"),
                "branch": "main",
            },
        )
        action = "activated"
    else:
        action = "already-active"
    pending_sha = pending.get("sha")
    if not isinstance(pending_sha, str) or not pending_sha:
        raise PublicationError(f"{slug}: staged CI carrier lacks a blob SHA")
    api(
        "DELETE",
        f"/repos/{slug}/contents/{quote(pending_path, safe='/')}",
        credential,
        {
            "message": "chore: remove staged CI carrier after activation",
            "sha": pending_sha,
            "branch": "main",
        },
    )
    return action


def close_pull(repo: str, number: int, credential: str) -> None:
    pull = api("GET", f"/repos/{repo}/pulls/{number}", credential, allow_missing=True)
    if not pull or pull.get("state") == "closed":
        return
    api("PATCH", f"/repos/{repo}/pulls/{number}", credential, {"state": "closed"})


def build_report(credential: str) -> dict[str, Any]:
    manifest = load_fleet_manifest(credential)
    organizations = verify_public_fleet(manifest, credential)

    extracted_preflight: dict[str, dict[str, Any]] = {}
    for slug, pending_path in EXTRACTED.items():
        repository = api("GET", f"/repos/{slug}", credential)
        if not isinstance(repository, dict):
            raise PublicationError(f"{slug}: repository metadata is missing")
        verified = verify_repository(
            slug,
            repository,
            credential,
            expected_visibility="private",
        )
        inspect_ci_state(slug, pending_path, credential)
        extracted_preflight[slug] = verified

    unreal = org_repositories("unreal-unity-poc", credential)
    if len(unreal) < 25:
        raise PublicationError(
            f"unreal-unity-poc: expected at least 25 repositories, found {len(unreal)}"
        )

    extracted: dict[str, dict[str, Any]] = {}
    for slug, pending_path in EXTRACTED.items():
        action = activate_ci(slug, pending_path, credential)
        if get_content(slug, ".github/workflows/ci.yml", credential) is None:
            raise PublicationError(f"{slug}: canonical CI is missing after activation")
        if get_content(slug, pending_path, credential) is not None:
            raise PublicationError(f"{slug}: staged CI carrier remains after activation")
        extracted[slug] = extracted_preflight[slug] | {
            "ci": "active",
            "ci_action": action,
            "pending_carrier": False,
        }

    return {
        "success": True,
        "source": {
            "repository": FLEET_SOURCE_REPOSITORY,
            "sha": FLEET_SOURCE_SHA,
            "manifest": FLEET_MANIFEST_PATH,
            "generator_sha256": EXPECTED_GENERATOR_SHA256,
        },
        "summary": {
            "hypesiege": 15,
            "streempilot": 17,
            "public_fleet": 32,
            "extracted_private": 2,
            "total": 34,
        },
        "organizations": organizations
        | {
            "unreal-unity-poc": {
                "minimum": 25,
                "count": len(unreal),
                "repositories": sorted(
                    str(item.get("full_name")) for item in unreal
                ),
            }
        },
        "extracted_repositories": extracted,
    }


def markdown(report: dict[str, Any]) -> str:
    lines = [
        "# Canonical organization repository publication",
        "",
        "Overall result: **SUCCESS**",
        "",
        (
            "Approved source: "
            f"`{report['source']['repository']}@{report['source']['sha']}`"
        ),
        "",
    ]
    for org in ("hypesiege", "StreemPilot"):
        group = report["organizations"][org]
        lines.extend([f"## {org}: {group['count']}/{group['expected']}", ""])
        for repository in group["repositories"]:
            lines.append(
                f"- `{repository['slug']}` (ID `{repository['id']}`) — "
                f"`main` `{repository['main_sha']}`; public"
            )
        lines.append("")
    for slug, repository in report["extracted_repositories"].items():
        lines.extend(
            [
                f"## {slug}",
                "",
                f"- repository ID: `{repository['id']}`",
                f"- `main`: `{repository['main_sha']}`",
                "- visibility: private",
                f"- canonical CI: {repository['ci']} ({repository['ci_action']})",
                "- staged CI carrier: removed",
                "",
            ]
        )
    unreal = report["organizations"]["unreal-unity-poc"]
    lines.extend(
        [
            f"## unreal-unity-poc: {unreal['count']} repositories",
            "",
            "The previously published Unreal/Unity fleet remains visible to the owner session.",
            "",
            "All 34 approved repositories passed the publication and verification gates.",
        ]
    )
    return "\n".join(lines) + "\n"


def write_atomic(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=path.parent,
        prefix=f".{path.name}.",
        delete=False,
    ) as handle:
        handle.write(content)
        handle.flush()
        os.fsync(handle.fileno())
        temporary = Path(handle.name)
    temporary.replace(path)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json-report", type=Path, required=True)
    parser.add_argument("--markdown-report", type=Path, required=True)
    parser.add_argument("--close-carriers", action="store_true")
    args = parser.parse_args()

    credential = token()
    report = build_report(credential)
    write_atomic(
        args.json_report,
        json.dumps(report, indent=2, sort_keys=True) + "\n",
    )
    write_atomic(args.markdown_report, markdown(report))

    if args.close_carriers:
        for number in TRIGGER_PULLS:
            close_pull("ORESoftware/k8s-cluster", number, credential)
        close_pull("ORESoftware/ai-agent-coordinator.rs", 35, credential)
    print(args.markdown_report.read_text(encoding="utf-8"), end="")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PublicationError as exc:
        print(f"finalize-missing-org-repositories: {exc}", file=sys.stderr)
        raise SystemExit(1)
