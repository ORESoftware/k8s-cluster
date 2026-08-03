#!/usr/bin/env python3
from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from typing import Any, Iterable
import urllib.error
import urllib.request

SOURCE_REPOSITORY = "ORESoftware/k8s-cluster"
SOURCE_SHA = "55ee15c190b7cfa4e075f6984c7cb551acd4b9d3"
BUNDLE_SHA256 = "1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031"
PUBLISHER_SHA256 = "e2fe6eaa622db02a54f83e27a822f64ad4b54971c883f97bbda4ac0a4db5d278"
EXPECTED_MAIN = "4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1"
EXPECTED_FEATURE = "789d48039da232faed985d4f8de176959f117e08"
EXPECTED_FEATURE_REF = "refs/heads/agent/den-1057-meta-agent-control-plane"
EXPECTED_HEADS = {
    "HEAD": EXPECTED_FEATURE,
    "refs/heads/main": EXPECTED_MAIN,
    EXPECTED_FEATURE_REF: EXPECTED_FEATURE,
}
ASSET_PATTERN = re.compile(r"^scripts/critical-org-fleet/assets/meta\.part[^/]+$")
PUBLISHER_PATH = "scripts/critical-org-fleet/publish_meta_control_plane.py"
HEX_SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
API_ROOT = "https://api.github.com"
API_VERSION = "2022-11-28"


class SnapshotError(RuntimeError):
    """Fail-closed source snapshot error with a bounded stage identifier."""

    def __init__(self, stage: str, message: str) -> None:
        super().__init__(message)
        self.stage = stage


class GitHubApi:
    def __init__(self, token: str) -> None:
        if not token or any(character.isspace() for character in token):
            raise SnapshotError("credential-preflight", "missing or malformed workflow token")
        self._token = token

    def get_json(self, path: str, *, stage: str) -> dict[str, Any]:
        request = urllib.request.Request(
            API_ROOT + path,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self._token}",
                "X-GitHub-Api-Version": API_VERSION,
                "User-Agent": "meta-agent-source-snapshot-verifier",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                payload = json.load(response)
        except urllib.error.HTTPError as error:
            error.read(4096)
            raise SnapshotError(stage, f"GitHub API returned HTTP {error.code} for {path}") from error
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            raise SnapshotError(stage, f"GitHub API request failed for {path}") from error
        if not isinstance(payload, dict):
            raise SnapshotError(stage, f"GitHub API returned a non-object for {path}")
        return payload


def require_sha(value: object, *, stage: str, label: str) -> str:
    if not isinstance(value, str) or HEX_SHA_PATTERN.fullmatch(value) is None:
        raise SnapshotError(stage, f"{label} is not a lowercase 40-character Git SHA")
    return value


def select_asset_entries(tree_payload: dict[str, Any]) -> list[tuple[str, str]]:
    stage = "select-source-assets"
    if tree_payload.get("truncated") is not False:
        raise SnapshotError(stage, "recursive source tree is truncated or missing its flag")
    tree = tree_payload.get("tree")
    if not isinstance(tree, list):
        raise SnapshotError(stage, "recursive source tree is not a list")

    selected: list[tuple[str, str]] = []
    seen_paths: set[str] = set()
    for entry in tree:
        if not isinstance(entry, dict):
            continue
        path = entry.get("path")
        if not isinstance(path, str) or ASSET_PATTERN.fullmatch(path) is None:
            continue
        if path in seen_paths:
            raise SnapshotError(stage, f"duplicate sealed asset path: {path}")
        seen_paths.add(path)
        if entry.get("type") != "blob":
            raise SnapshotError(stage, f"sealed asset is not a blob: {path}")
        sha = require_sha(entry.get("sha"), stage=stage, label=f"blob SHA for {path}")
        selected.append((path, sha))

    selected.sort(key=lambda item: item[0])
    if not selected:
        raise SnapshotError(stage, "no sealed meta.part assets were found")
    return selected


def select_publisher_entry(tree_payload: dict[str, Any]) -> tuple[str, str]:
    stage = "select-publisher-blob"
    tree = tree_payload.get("tree")
    if not isinstance(tree, list):
        raise SnapshotError(stage, "recursive source tree is not a list")
    matches: list[dict[str, Any]] = [
        entry
        for entry in tree
        if isinstance(entry, dict) and entry.get("path") == PUBLISHER_PATH
    ]
    if len(matches) != 1:
        raise SnapshotError(stage, f"expected one publisher blob, found {len(matches)}")
    entry = matches[0]
    if entry.get("type") != "blob":
        raise SnapshotError(stage, "publisher path is not a blob")
    sha = require_sha(entry.get("sha"), stage=stage, label="publisher blob SHA")
    return PUBLISHER_PATH, sha


def git_blob_sha(content: bytes) -> str:
    header = f"blob {len(content)}\0".encode("ascii")
    return hashlib.sha1(header + content).hexdigest()  # noqa: S324 - Git object identity


def decode_github_blob(
    payload: dict[str, Any],
    *,
    expected_sha: str,
    stage: str,
    label: str,
) -> bytes:
    if payload.get("encoding") != "base64":
        raise SnapshotError(stage, f"{label} is not base64 encoded by GitHub")
    encoded = payload.get("content")
    if not isinstance(encoded, str):
        raise SnapshotError(stage, f"{label} has no string content")
    compact = "".join(encoded.split())
    try:
        content = base64.b64decode(compact, validate=True)
    except (ValueError, binascii.Error) as error:
        raise SnapshotError(stage, f"{label} has invalid GitHub transport base64") from error
    if not content:
        raise SnapshotError(stage, f"{label} decoded to empty content")
    observed_sha = git_blob_sha(content)
    if observed_sha != expected_sha:
        raise SnapshotError(
            stage,
            f"{label} Git blob identity mismatch: {observed_sha} != {expected_sha}",
        )
    return content


def decode_bundle_parts(parts: Iterable[bytes]) -> bytes:
    stage = "decode-sealed-bundle"
    combined = b"".join(parts)
    if not combined:
        raise SnapshotError(stage, "sealed bundle text is empty")
    try:
        compact = b"".join(combined.split())
        bundle = base64.b64decode(compact, validate=True)
    except (ValueError, binascii.Error) as error:
        raise SnapshotError(stage, "sealed bundle text is not valid base64") from error
    if not bundle:
        raise SnapshotError(stage, "sealed bundle decoded to empty bytes")
    return bundle


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def run_command(
    arguments: list[str],
    *,
    stage: str,
    cwd: Path | None = None,
) -> str:
    process = subprocess.run(
        arguments,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        env={**os.environ, "GIT_TERMINAL_PROMPT": "0"},
    )
    if process.returncode != 0:
        detail = (process.stderr or process.stdout).strip().replace("\n", " ")[-1000:]
        raise SnapshotError(
            stage,
            f"command failed ({process.returncode}): {' '.join(arguments)}; {detail}",
        )
    return process.stdout.strip()


def parse_bundle_heads(output: str) -> dict[str, str]:
    stage = "verify-bundle-heads"
    observed: dict[str, str] = {}
    for line in output.splitlines():
        fields = line.split()
        if len(fields) != 2:
            raise SnapshotError(stage, f"malformed bundle head line: {line!r}")
        sha, ref = fields
        require_sha(sha, stage=stage, label=f"bundle head SHA for {ref}")
        if ref in observed:
            raise SnapshotError(stage, f"duplicate bundle ref: {ref}")
        observed[ref] = sha
    if observed != EXPECTED_HEADS:
        raise SnapshotError(stage, f"bundle heads differ: {observed!r}")
    if observed["HEAD"] != observed[EXPECTED_FEATURE_REF]:
        raise SnapshotError(stage, "bundle HEAD does not resolve to the reviewed feature SHA")
    return observed


def verify_bundle(bundle: bytes, work: Path) -> dict[str, str]:
    stage = "verify-bundle-digest"
    observed_digest = sha256_bytes(bundle)
    if observed_digest != BUNDLE_SHA256:
        raise SnapshotError(
            stage,
            f"bundle SHA-256 mismatch: {observed_digest} != {BUNDLE_SHA256}",
        )

    bundle_path = work / "meta-agent-control-plane.bundle"
    bundle_path.write_bytes(bundle)
    source_repository = work / "source-repository"
    run_command(["git", "init", "--quiet", str(source_repository)], stage="initialize-bundle-context")
    inside = run_command(
        ["git", "-C", str(source_repository), "rev-parse", "--is-inside-work-tree"],
        stage="initialize-bundle-context",
    )
    if inside != "true":
        raise SnapshotError("initialize-bundle-context", "source context is not a Git work tree")
    run_command(
        ["git", "-C", str(source_repository), "bundle", "verify", str(bundle_path)],
        stage="verify-bundle-context",
    )
    heads_output = run_command(
        ["git", "bundle", "list-heads", str(bundle_path)],
        stage="verify-bundle-heads",
    )
    return parse_bundle_heads(heads_output)


def verify_publisher(content: bytes, work: Path) -> str:
    stage = "verify-publisher"
    observed_digest = sha256_bytes(content)
    if observed_digest != PUBLISHER_SHA256:
        raise SnapshotError(
            stage,
            f"publisher SHA-256 mismatch: {observed_digest} != {PUBLISHER_SHA256}",
        )
    publisher = work / "publish_meta_control_plane.py"
    publisher.write_bytes(content)
    run_command([sys.executable, "-m", "py_compile", str(publisher)], stage=stage)
    return observed_digest


def verify_snapshot(api: GitHubApi) -> dict[str, Any]:
    print("source-snapshot-stage=load-source-commit status=running", flush=True)
    commit = api.get_json(
        f"/repos/{SOURCE_REPOSITORY}/git/commits/{SOURCE_SHA}",
        stage="load-source-commit",
    )
    observed_commit = require_sha(
        commit.get("sha"), stage="load-source-commit", label="source commit SHA"
    )
    if observed_commit != SOURCE_SHA:
        raise SnapshotError(
            "load-source-commit", f"source commit mismatch: {observed_commit} != {SOURCE_SHA}"
        )
    tree = commit.get("tree")
    if not isinstance(tree, dict):
        raise SnapshotError("load-source-commit", "source commit has no tree object")
    tree_sha = require_sha(
        tree.get("sha"), stage="load-source-commit", label="source tree SHA"
    )
    print(f"source-snapshot-stage=load-source-commit status=passed tree={tree_sha}", flush=True)

    print("source-snapshot-stage=load-source-tree status=running", flush=True)
    tree_payload = api.get_json(
        f"/repos/{SOURCE_REPOSITORY}/git/trees/{tree_sha}?recursive=1",
        stage="load-source-tree",
    )
    assets = select_asset_entries(tree_payload)
    publisher_path, publisher_sha = select_publisher_entry(tree_payload)
    print(
        "source-snapshot-stage=load-source-tree status=passed "
        f"asset_count={len(assets)} publisher_blob={publisher_sha}",
        flush=True,
    )

    print("source-snapshot-stage=decode-source-assets status=running", flush=True)
    part_contents: list[bytes] = []
    for index, (path, sha) in enumerate(assets, start=1):
        blob = api.get_json(
            f"/repos/{SOURCE_REPOSITORY}/git/blobs/{sha}",
            stage="decode-source-assets",
        )
        part_contents.append(
            decode_github_blob(
                blob,
                expected_sha=sha,
                stage="decode-source-assets",
                label=f"asset {index}/{len(assets)} ({path})",
            )
        )
    bundle = decode_bundle_parts(part_contents)
    print(
        "source-snapshot-stage=decode-source-assets status=passed "
        f"encoded_bytes={sum(len(part) for part in part_contents)} bundle_bytes={len(bundle)}",
        flush=True,
    )

    with tempfile.TemporaryDirectory(prefix="meta-agent-source-snapshot-") as temporary:
        work = Path(temporary)
        print("source-snapshot-stage=verify-bundle status=running", flush=True)
        heads = verify_bundle(bundle, work)
        print(
            "source-snapshot-stage=verify-bundle status=passed "
            f"bundle_sha256={BUNDLE_SHA256} heads={len(heads)} symbolic_head={heads['HEAD']}",
            flush=True,
        )

        print("source-snapshot-stage=verify-publisher status=running", flush=True)
        publisher_blob = api.get_json(
            f"/repos/{SOURCE_REPOSITORY}/git/blobs/{publisher_sha}",
            stage="load-publisher-blob",
        )
        publisher_content = decode_github_blob(
            publisher_blob,
            expected_sha=publisher_sha,
            stage="load-publisher-blob",
            label=publisher_path,
        )
        publisher_digest = verify_publisher(publisher_content, work)
        print(
            "source-snapshot-stage=verify-publisher status=passed "
            f"publisher_sha256={publisher_digest}",
            flush=True,
        )

    return {
        "source_repository": SOURCE_REPOSITORY,
        "source_sha": SOURCE_SHA,
        "source_tree_sha": tree_sha,
        "asset_count": len(assets),
        "asset_paths": [path for path, _ in assets],
        "bundle_sha256": BUNDLE_SHA256,
        "bundle_bytes": len(bundle),
        "heads": heads,
        "symbolic_head": heads["HEAD"],
        "publisher_path": publisher_path,
        "publisher_blob_sha": publisher_sha,
        "publisher_sha256": publisher_digest,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Verify the immutable Meta Agent source snapshot without a repository-admin credential."
    )
    parser.add_argument(
        "--json-report",
        type=Path,
        help="Optional path for a credential-free JSON report.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN") or ""
    try:
        report = verify_snapshot(GitHubApi(token))
    except SnapshotError as error:
        detail = str(error).replace("\n", " ")[:1200]
        print(
            f"source-snapshot-stage={error.stage} status=failed detail={detail}",
            file=sys.stderr,
            flush=True,
        )
        return 1

    if args.json_report is not None:
        args.json_report.parent.mkdir(parents=True, exist_ok=True)
        args.json_report.write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    print("source-snapshot-stage=complete status=passed", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
