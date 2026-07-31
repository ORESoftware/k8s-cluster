#!/usr/bin/env python3
"""Publish and verify the two extracted repositories outside the sealed product fleet.

This entry point deliberately excludes HypeSiege and StreemPilot. Those 32
repositories have their own schema-v2 ledger and trusted-main publisher. Here we
materialize only the two remaining repositories from the fixed July 31, 2026
allowlist, activate their staged CI workflows, and record their final remote
``main`` commits.
"""

from __future__ import annotations

import argparse
import base64
import importlib.util
import json
import os
from datetime import datetime, timezone
from pathlib import Path
import shutil
import sys
import tempfile
from typing import Any, Callable
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request, urlopen

API = "https://api.github.com"
API_VERSION = "2022-11-28"
ROOT = Path(__file__).resolve().parents[2]
PUBLISHER_PATH = ROOT / "scripts/ops/publish_missing_org_repositories.py"

TARGETS: dict[str, dict[str, Any]] = {
    "meta-agents-demo/meta-agent-control-plane.rs": {
        "pending_ci": ".meta-agent-ci.yml.pending",
        "required_paths": (
            "Cargo.toml",
            "README.md",
            "scripts/verify_contract.py",
        ),
        "publisher": "publish_meta_agents",
    },
    "file-tunnel/ftnl-mcp-server.rs": {
        "pending_ci": ".ftnl-mcp-ci.yml.pending",
        "required_paths": (
            "Cargo.toml",
            "README.md",
            "src/main.rs",
        ),
        "publisher": "publish_file_tunnel_mcp",
    },
}


class PublicationError(RuntimeError):
    """A fail-closed publication or verification error."""


def credential() -> str:
    value = os.environ.get("GH_TOKEN", "").strip()
    if not value:
        raise PublicationError("GH_TOKEN is required")
    if "\n" in value or "\r" in value:
        raise PublicationError("GH_TOKEN contains a line break")
    return value


def api(
    method: str,
    path: str,
    *,
    payload: dict[str, Any] | None = None,
    allow_missing: bool = False,
) -> Any:
    body = None if payload is None else json.dumps(payload).encode("utf-8")
    request = Request(
        API + path,
        data=body,
        method=method,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {credential()}",
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": "bounded-extracted-repository-publisher",
            **({"Content-Type": "application/json"} if body is not None else {}),
        },
    )
    try:
        with urlopen(request, timeout=45) as response:
            raw = response.read()
            return json.loads(raw) if raw else None
    except HTTPError as error:
        error.read(4096)
        if allow_missing and error.code == 404:
            return None
        raise PublicationError(
            f"GitHub API {method} {path} failed with HTTP {error.code}"
        ) from None


def repository(slug: str, *, allow_missing: bool = False) -> dict[str, Any] | None:
    result = api("GET", f"/repos/{slug}", allow_missing=allow_missing)
    if result is None:
        return None
    if not isinstance(result, dict):
        raise PublicationError(f"{slug}: malformed repository response")
    return result


def content(slug: str, path: str) -> dict[str, Any] | None:
    result = api(
        "GET",
        f"/repos/{slug}/contents/{quote(path, safe='/')}?ref=main",
        allow_missing=True,
    )
    if result is None:
        return None
    if not isinstance(result, dict):
        raise PublicationError(f"{slug}:{path}: expected a file response")
    return result


def decoded(record: dict[str, Any]) -> bytes:
    encoded = record.get("content")
    if not isinstance(encoded, str):
        raise PublicationError("GitHub Contents response has no inline content")
    try:
        return base64.b64decode(encoded.replace("\n", ""), validate=True)
    except ValueError as error:
        raise PublicationError("GitHub Contents response is not valid base64") from error


def activate_ci(slug: str, pending_path: str) -> str:
    canonical_path = ".github/workflows/ci.yml"
    pending = content(slug, pending_path)
    canonical = content(slug, canonical_path)

    if pending is None:
        if canonical is None:
            raise PublicationError(
                f"{slug}: neither staged nor canonical CI workflow exists"
            )
        return "already-active"

    pending_bytes = decoded(pending)
    if canonical is None:
        api(
            "PUT",
            f"/repos/{slug}/contents/{quote(canonical_path, safe='/')}",
            payload={
                "message": "ci: activate canonical validation after publication",
                "content": base64.b64encode(pending_bytes).decode("ascii"),
                "branch": "main",
            },
        )
        action = "activated"
    else:
        if decoded(canonical) != pending_bytes:
            raise PublicationError(
                f"{slug}: canonical CI conflicts with the staged workflow"
            )
        action = "already-active"

    pending_sha = pending.get("sha")
    if not isinstance(pending_sha, str) or not pending_sha:
        raise PublicationError(f"{slug}: staged CI workflow has no blob SHA")
    api(
        "DELETE",
        f"/repos/{slug}/contents/{quote(pending_path, safe='/')}",
        payload={
            "message": "chore: remove staged CI carrier after activation",
            "sha": pending_sha,
            "branch": "main",
        },
    )
    return action


def main_sha(slug: str) -> str:
    ref = api("GET", f"/repos/{slug}/git/ref/heads/main")
    sha = ((ref or {}).get("object") or {}).get("sha")
    if not isinstance(sha, str) or len(sha) != 40:
        raise PublicationError(f"{slug}: main does not resolve to a full commit SHA")
    return sha


def verify_target(slug: str, definition: dict[str, Any], ci_action: str) -> dict[str, Any]:
    metadata = repository(slug)
    assert metadata is not None
    if metadata.get("visibility") != "public":
        raise PublicationError(
            f"{slug}: visibility is {metadata.get('visibility')!r}, expected 'public'"
        )
    if metadata.get("default_branch") != "main":
        raise PublicationError(
            f"{slug}: default branch is {metadata.get('default_branch')!r}, expected 'main'"
        )
    if content(slug, ".github/workflows/ci.yml") is None:
        raise PublicationError(f"{slug}: canonical CI workflow is missing")
    if content(slug, str(definition["pending_ci"])) is not None:
        raise PublicationError(f"{slug}: staged CI carrier remains after activation")
    for required in definition["required_paths"]:
        if content(slug, str(required)) is None:
            raise PublicationError(f"{slug}: required path {required!r} is missing")

    repository_id = metadata.get("id")
    node_id = metadata.get("node_id")
    if not isinstance(repository_id, int) or not isinstance(node_id, str):
        raise PublicationError(f"{slug}: repository identity is incomplete")
    return {
        "repository": slug,
        "repository_id": repository_id,
        "node_id": node_id,
        "visibility": "public",
        "default_branch": "main",
        "main_sha": main_sha(slug),
        "ci": ci_action,
        "html_url": metadata.get("html_url"),
    }


def load_publisher() -> Any:
    spec = importlib.util.spec_from_file_location(
        "bounded_missing_repository_publisher",
        PUBLISHER_PATH,
    )
    if spec is None or spec.loader is None:
        raise PublicationError(f"cannot load {PUBLISHER_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def publish_absent_targets(module: Any, work: Path) -> dict[str, str]:
    actions: dict[str, str] = {}
    for slug, definition in TARGETS.items():
        existing = repository(slug, allow_missing=True)
        if existing is not None:
            actions[slug] = "already-present"
            continue
        function_name = str(definition["publisher"])
        function: Callable[[Path], None] | None = getattr(module, function_name, None)
        if function is None:
            raise PublicationError(f"publisher function {function_name!r} is missing")
        function(work)
        if repository(slug, allow_missing=True) is None:
            raise PublicationError(f"{slug}: publisher returned without creating the repository")
        actions[slug] = "published"
    return actions


def render_markdown(report: dict[str, Any]) -> str:
    lines = [
        "# Extracted repository publication",
        "",
        "Overall result: **SUCCESS**",
        "",
        f"- Verified repositories: `{report['repository_count']}/2`",
        f"- Verified at: `{report['verified_at']}`",
        "",
    ]
    for item in report["repositories"]:
        lines.extend(
            [
                f"## {item['repository']}",
                "",
                f"- repository ID: `{item['repository_id']}`",
                f"- visibility: `{item['visibility']}`",
                f"- default branch: `{item['default_branch']}`",
                f"- final `main`: `{item['main_sha']}`",
                f"- publication: `{item['publication']}`",
                f"- CI activation: `{item['ci']}`",
                "",
            ]
        )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json-report", type=Path, required=True)
    parser.add_argument("--markdown-report", type=Path, required=True)
    args = parser.parse_args()

    credential()
    module = load_publisher()
    work = Path(tempfile.mkdtemp(prefix="extracted-repository-publication-"))
    try:
        publication_actions = publish_absent_targets(module, work)
        verified: list[dict[str, Any]] = []
        for slug, definition in TARGETS.items():
            ci_action = activate_ci(slug, str(definition["pending_ci"]))
            item = verify_target(slug, definition, ci_action)
            item["publication"] = publication_actions[slug]
            verified.append(item)
    finally:
        shutil.rmtree(work, ignore_errors=True)

    report = {
        "success": True,
        "verified_at": datetime.now(timezone.utc).isoformat(),
        "k8s_cluster_sha": os.environ.get("GITHUB_SHA"),
        "repository_count": len(verified),
        "repositories": verified,
    }
    if report["repository_count"] != 2:
        raise PublicationError("expected exactly two extracted repositories")
    args.json_report.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    args.markdown_report.write_text(render_markdown(report), encoding="utf-8")
    print(args.markdown_report.read_text(encoding="utf-8"), end="")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PublicationError as error:
        print(f"publish-extracted-repositories: {error}", file=sys.stderr)
        raise SystemExit(1)
