"""Repository policy, Git data, bootstrap PR, and issue mutations."""
from __future__ import annotations
import re
import textwrap
import urllib.parse
from typing import Any, Iterable, Mapping
from coliving_bootstrap import bootstrap_files
from coliving_github_api import GitHubApi
from coliving_repository_specs import BRANCH, TRACKING, RepoSpec

SHA_RE = re.compile(r"^[0-9a-f]{40}$")
TOKEN_SHAPE_RE = re.compile(
    r"(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|"
    r"lin_api_[A-Za-z0-9]{20,}|-----BEGIN [A-Z ]*PRIVATE KEY-----)"
)

def create_payload(spec: RepoSpec) -> dict[str, object]:
    return {
        "name": spec.name,
        "description": spec.description,
        "private": True,
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "is_template": False,
        "auto_init": True,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "delete_branch_on_merge": True,
    }


def patch_payload(spec: RepoSpec) -> dict[str, object]:
    return {
        "description": spec.description,
        "private": True,
        "visibility": "private",
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "allow_auto_merge": False,
        "delete_branch_on_merge": True,
    }


def validate_repository(value: Mapping[str, Any], spec: RepoSpec) -> tuple[int, str]:
    expected = {
        "full_name": spec.full_name,
        "private": True,
        "visibility": "private",
        "default_branch": "main",
        "description": spec.description,
        "archived": False,
        "disabled": False,
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "allow_auto_merge": False,
        "delete_branch_on_merge": True,
    }
    for key, expected_value in expected.items():
        if value.get(key) != expected_value:
            raise RuntimeError(f"repository invariant mismatch for {spec.full_name}: {key}={value.get(key)!r} expected {expected_value!r}")
    repository_id = value.get("id")
    if not isinstance(repository_id, int) or repository_id <= 0:
        raise RuntimeError(f"repository id missing for {spec.full_name}")
    default_branch = value.get("default_branch")
    if not isinstance(default_branch, str):
        raise RuntimeError(f"default branch missing for {spec.full_name}")
    return repository_id, default_branch


def ref_sha(api: GitHubApi, spec: RepoSpec, branch: str) -> str | None:
    quoted = urllib.parse.quote(branch, safe="")
    response = api.request("GET", f"/repos/{spec.full_name}/git/ref/heads/{quoted}")
    if response.status == 404:
        return None
    value = api.require_object(response, f"read branch {spec.full_name}:{branch}", {200})
    target = value.get("object")
    sha = target.get("sha") if isinstance(target, dict) else None
    if not isinstance(sha, str) or SHA_RE.fullmatch(sha) is None:
        raise RuntimeError(f"invalid branch SHA for {spec.full_name}:{branch}")
    return sha


def commit_tree_sha(api: GitHubApi, spec: RepoSpec, commit_sha: str) -> str:
    value = api.require_object(
        api.request("GET", f"/repos/{spec.full_name}/git/commits/{commit_sha}"),
        f"read commit {spec.full_name}@{commit_sha}",
        {200},
    )
    tree = value.get("tree")
    tree_sha = tree.get("sha") if isinstance(tree, dict) else None
    if not isinstance(tree_sha, str) or SHA_RE.fullmatch(tree_sha) is None:
        raise RuntimeError(f"invalid tree SHA for {spec.full_name}@{commit_sha}")
    return tree_sha


def publish_bootstrap(api: GitHubApi, spec: RepoSpec, base_sha: str) -> tuple[str, int, str, bool]:
    branch_sha = ref_sha(api, spec, BRANCH)
    parent_sha = branch_sha or base_sha
    base_tree_sha = commit_tree_sha(api, spec, parent_sha)

    tree_entries: list[dict[str, object]] = []
    for path, content in bootstrap_files(spec).items():
        if TOKEN_SHAPE_RE.search(content):
            raise RuntimeError(f"bootstrap content contains a credential-shaped literal: {spec.full_name}:{path}")
        blob = api.require_object(
            api.request("POST", f"/repos/{spec.full_name}/git/blobs", {"content": content, "encoding": "utf-8"}),
            f"create blob {spec.full_name}:{path}",
            {201},
        )
        blob_sha = blob.get("sha")
        if not isinstance(blob_sha, str) or SHA_RE.fullmatch(blob_sha) is None:
            raise RuntimeError(f"invalid blob SHA for {spec.full_name}:{path}")
        tree_entries.append({"path": path, "mode": "100755" if path.startswith("scripts/") else "100644", "type": "blob", "sha": blob_sha})

    tree = api.require_object(
        api.request("POST", f"/repos/{spec.full_name}/git/trees", {"base_tree": base_tree_sha, "tree": tree_entries}),
        f"create bootstrap tree for {spec.full_name}",
        {201},
    )
    new_tree_sha = tree.get("sha")
    if not isinstance(new_tree_sha, str) or SHA_RE.fullmatch(new_tree_sha) is None:
        raise RuntimeError(f"invalid bootstrap tree SHA for {spec.full_name}")

    if new_tree_sha == base_tree_sha:
        head_sha = parent_sha
    else:
        commit = api.require_object(
            api.request(
                "POST",
                f"/repos/{spec.full_name}/git/commits",
                {"message": f"feat({TRACKING}): bootstrap {spec.name} resident operations", "tree": new_tree_sha, "parents": [parent_sha]},
            ),
            f"create bootstrap commit for {spec.full_name}",
            {201},
        )
        head_sha = commit.get("sha")
        if not isinstance(head_sha, str) or SHA_RE.fullmatch(head_sha) is None:
            raise RuntimeError(f"invalid bootstrap commit SHA for {spec.full_name}")
        if branch_sha is None:
            api.require_object(
                api.request("POST", f"/repos/{spec.full_name}/git/refs", {"ref": f"refs/heads/{BRANCH}", "sha": head_sha}),
                f"create bootstrap branch for {spec.full_name}",
                {201},
            )
        else:
            quoted = urllib.parse.quote(BRANCH, safe="")
            api.require_object(
                api.request("PATCH", f"/repos/{spec.full_name}/git/refs/heads/{quoted}", {"sha": head_sha, "force": False}),
                f"advance bootstrap branch for {spec.full_name}",
                {200},
            )

    pulls = api.require_list(
        api.request("GET", f"/repos/{spec.full_name}/pulls?state=open&head={urllib.parse.quote(spec.org + ':' + BRANCH, safe='')}&base=main&per_page=100"),
        f"list bootstrap PRs for {spec.full_name}",
        {200},
    )
    if pulls:
        pull = pulls[0]
        pr_created = False
    else:
        pull = api.require_object(
            api.request(
                "POST",
                f"/repos/{spec.full_name}/pulls",
                {
                    "title": f"[{TRACKING}] Bootstrap {spec.name} for co-living operations",
                    "head": BRANCH,
                    "base": "main",
                    "draft": True,
                    "maintainer_can_modify": True,
                    "body": bootstrap_pr_body(spec, head_sha),
                },
            ),
            f"create bootstrap PR for {spec.full_name}",
            {201},
        )
        pr_created = True
    pr_number = pull.get("number")
    pr_url = pull.get("html_url")
    if not isinstance(pr_number, int) or not isinstance(pr_url, str):
        raise RuntimeError(f"invalid bootstrap PR response for {spec.full_name}")

    issue_title = f"[{TRACKING}] Complete {spec.name} resident-operations rollout"
    issues = api.require_list(
        api.request("GET", f"/repos/{spec.full_name}/issues?state=open&per_page=100"),
        f"list implementation issues for {spec.full_name}",
        {200},
    )
    issue = next((item for item in issues if isinstance(item, dict) and item.get("title") == issue_title and "pull_request" not in item), None)
    if issue is None:
        issue = api.require_object(
            api.request(
                "POST",
                f"/repos/{spec.full_name}/issues",
                {"title": issue_title, "body": implementation_issue_body(spec, pr_url)},
            ),
            f"create implementation issue for {spec.full_name}",
            {201},
        )
    issue_number = issue.get("number")
    issue_url = issue.get("html_url")
    if not isinstance(issue_number, int) or not isinstance(issue_url, str):
        raise RuntimeError(f"invalid implementation issue response for {spec.full_name}")

    return head_sha, pr_number, pr_url, pr_created


def bootstrap_pr_body(spec: RepoSpec, head_sha: str) -> str:
    return textwrap.dedent(
        f"""\
        ## Summary

        Initializes `{spec.full_name}` as part of the co-living repository expansion tracked by `{TRACKING}`.

        The bootstrap includes executable policy checks, immutable CI action pins, semantic conflict rules,
        no-secret validation, and a first deterministic behavior slice for repository kind `{spec.kind}`.

        ## Integration gates

        Follow-on work must wire the shared contract and appropriate subset of Shared Auth, ores-middleware,
        ores-rate-limit, ores-sops, opto-sync, zed-pkg, ores-redis-lru-cache, ores-otel, ores-chat,
        ores-edge-router, ores-wasm-loaders, Stripe, Supabase, Neon, and the MIP solver adapter.

        Head evidence: `{head_sha}`. This PR is intentionally draft until hosted CI passes and the owning
        implementation slice has an independent semantic review.
        """
    )


def implementation_issue_body(spec: RepoSpec, pr_url: str) -> str:
    return textwrap.dedent(
        f"""\
        Bootstrap PR: {pr_url}

        ## Required completion

        - consume the resident-operations v1 peer-authority contract without copying generated artifacts;
        - preserve TypeSpec and authored JSON Schema as independent authorities and run
          `ORESoftware/typespec-json-schema-validator` where this repository consumes declarations;
        - use Shared Auth for subject/tenant/role identity and ordered ores-middleware for request policy;
        - add route-specific ores-rate-limit policy and ores-redis-lru-cache-backed runtime configuration;
        - keep encrypted configuration under `env/enc` through ores-sops and emit redacted ores-otel telemetry;
        - use opto-sync for offline/client/Supabase/Neon reconciliation where state crosses layers;
        - add ores-chat integration only through authorized space/membership references;
        - ensure WASM/service-worker/background preload is side-effect free;
        - add sibling test-org coverage, exact dependency pins, failure/rollback behavior, and operator docs.

        ## Product acceptance

        Cover the relevant guest, reservation, meal, poll, rent/refund, legal onboarding, assignment,
        chat-group, manager/owner/admin, and developer-access workflows. Browser storage is never legal or
        financial authority. Solver jobs are pseudonymous and exclude protected payloads.
        """
    )
