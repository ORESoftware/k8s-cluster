"""GitHub publication orchestration for the co-living repository fleet."""
from __future__ import annotations
import json
import os
from pathlib import Path
from coliving_github_api import GitHubApi
from coliving_repository_mutations import (
    TOKEN_SHAPE_RE, create_payload, patch_payload, publish_bootstrap, ref_sha, validate_repository,
)
from coliving_repository_specs import BRANCH, EXPECTED_COUNTS, SPECS, RepoSpec

ACTOR = "ORESoftware"
TRACKING = "DEN-1950"
EVIDENCE_SCHEMA = "ores.coliving-repository-publication/v1"

def token_from_environment() -> str:
    token = (
        os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN", "").strip()
        or os.environ.get("ORG_GITOPS_TOKEN", "").strip()
        or os.environ.get("GH_TOKEN", "").strip()
    )
    if not token or any(character.isspace() for character in token):
        raise RuntimeError("protected repository-administration credential is missing or malformed")
    return token


def preflight(api: GitHubApi) -> None:
    actor = api.require_object(api.request("GET", "/user"), "actor preflight", {200})
    if actor.get("login") != ACTOR:
        raise RuntimeError("protected credential does not authenticate the required ORESoftware actor")
    for org in sorted({spec.org for spec in SPECS}):
        membership = api.require_object(
            api.request("GET", f"/user/memberships/orgs/{org}"),
            f"organization membership preflight for {org}",
            {200},
        )
        if membership.get("role") != "admin" or membership.get("state") != "active":
            raise RuntimeError(f"protected credential is not an active administrator of {org}")


def ensure_repository(api: GitHubApi, spec: RepoSpec) -> dict[str, object]:
    inspected = api.request("GET", f"/repos/{spec.full_name}")
    created = False
    if inspected.status == 404:
        created_response = api.request("POST", f"/orgs/{spec.org}/repos", create_payload(spec))
        if created_response.status == 201:
            created = True
        elif created_response.status not in {409, 422}:
            message = created_response.payload.get("message") if isinstance(created_response.payload, dict) else None
            raise RuntimeError(f"repository creation failed for {spec.full_name}: HTTP {created_response.status}: {message}")
    elif inspected.status != 200:
        raise RuntimeError(f"repository inspection failed for {spec.full_name}: HTTP {inspected.status}")

    metadata = api.require_object(
        api.request("PATCH", f"/repos/{spec.full_name}", patch_payload(spec)),
        f"normalize repository policy for {spec.full_name}",
        {200},
    )
    repository_id, default_branch = validate_repository(metadata, spec)
    main_sha = ref_sha(api, spec, default_branch)
    if main_sha is None:
        raise RuntimeError(f"main branch missing after repository initialization: {spec.full_name}")

    topics = ["coliving", "hhaus", "resident-operations", "den-1950", spec.kind.replace("-", "")]
    api.require_object(
        api.request("PUT", f"/repos/{spec.full_name}/topics", {"names": topics}),
        f"set repository topics for {spec.full_name}",
        {200},
    )

    head_sha, pr_number, pr_url, pr_created = publish_bootstrap(api, spec, main_sha)
    issue_title = f"[{TRACKING}] Complete {spec.name} resident-operations rollout"
    issues = api.require_list(api.request("GET", f"/repos/{spec.full_name}/issues?state=open&per_page=100"), f"read implementation issue for {spec.full_name}", {200})
    issue = next((item for item in issues if isinstance(item, dict) and item.get("title") == issue_title and "pull_request" not in item), None)
    if not isinstance(issue, dict) or not isinstance(issue.get("number"), int) or not isinstance(issue.get("html_url"), str):
        raise RuntimeError(f"implementation issue was not retained for {spec.full_name}")

    return {
        "full_name": spec.full_name,
        "organization": spec.org,
        "name": spec.name,
        "kind": spec.kind,
        "repository_id": repository_id,
        "created": created,
        "visibility": "private",
        "default_branch": default_branch,
        "main_sha": main_sha,
        "bootstrap_branch": BRANCH,
        "bootstrap_head_sha": head_sha,
        "pull_request_number": pr_number,
        "pull_request_url": pr_url,
        "pull_request_created": pr_created,
        "issue_number": issue["number"],
        "issue_url": issue["html_url"],
    }


def publish(evidence_path: Path) -> int:
    api = GitHubApi(token_from_environment())
    preflight(api)
    records: list[dict[str, object]] = []
    for spec in SPECS:
        record = ensure_repository(api, spec)
        records.append(record)
        print(
            "COLIVING_REPOSITORY_READY "
            f"repository={record['full_name']} created={str(record['created']).lower()} "
            f"pr={record['pull_request_number']} issue={record['issue_number']} head={record['bootstrap_head_sha']}"
        )

    counts = {org: sum(record["organization"] == org for record in records) for org in EXPECTED_COUNTS}
    if counts != dict(EXPECTED_COUNTS):
        raise RuntimeError(f"published count mismatch: {counts!r}")
    evidence = {
        "schema": EVIDENCE_SCHEMA,
        "tracking": TRACKING,
        "actor": ACTOR,
        "repository_count": len(records),
        "counts": counts,
        "repositories": records,
        "credential_source": "protected-actions-secret",
        "credential_persisted": False,
        "credential_revoked": False,
    }
    evidence_path.parent.mkdir(parents=True, exist_ok=True)
    evidence_path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    evidence_path.chmod(0o600)
    print(f"COLIVING_REPOSITORY_FLEET_READY total={len(records)} counts={json.dumps(counts, sort_keys=True)}")
    return 0
