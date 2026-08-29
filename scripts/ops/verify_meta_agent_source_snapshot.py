#!/usr/bin/env python3
"""Reconstruct and verify the sealed Meta Agent publication source.

The script performs read-only GitHub Git Database API calls against one exact
commit, walks only the bounded directory path that owns the sealed bundle and
publisher, removes both base64 layers from the bundle carrier, and verifies all
reviewed digests, publishable branches, and declared auxiliary refs before any
repository-administration credential is needed.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

GITHUB_API_VERSION = "2022-11-28"
DEFAULT_REPOSITORY = "ORESoftware/k8s-cluster"
ASSET_NAME_PATTERN = re.compile(r"^meta\.part[^/]+$")
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
PUBLISHER_NAME = "publish_meta_control_plane.py"
ALLOWED_AUXILIARY_REFS = frozenset({"HEAD"})


class VerificationError(RuntimeError):
    """Raised when a fail-closed source verification invariant is violated."""


@dataclass(frozen=True)
class SnapshotResult:
    source_sha: str
    source_tree_sha: str
    asset_count: int
    bundle_sha256: str
    publisher_sha256: str
    heads: Mapping[str, str]
    auxiliary_heads: Mapping[str, str]
    bundle_path: Path
    publisher_path: Path

    def sanitized_json(self) -> str:
        return json.dumps(
            {
                "asset_count": self.asset_count,
                "auxiliary_heads": dict(sorted(self.auxiliary_heads.items())),
                "bundle_path": str(self.bundle_path),
                "bundle_sha256": self.bundle_sha256,
                "heads": dict(sorted(self.heads.items())),
                "publisher_path": str(self.publisher_path),
                "publisher_sha256": self.publisher_sha256,
                "source_sha": self.source_sha,
                "source_tree_sha": self.source_tree_sha,
                "status": "verified",
            },
            sort_keys=True,
        )


class GitHubClient:
    def __init__(self, token: str, repository: str) -> None:
        if not token or any(character.isspace() for character in token):
            raise VerificationError("GitHub read token is missing or malformed")
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
            raise VerificationError("repository must be in owner/name form")
        self._token = token
        self._repository = repository

    def get(self, path: str) -> Mapping[str, Any]:
        if not path.startswith("/"):
            raise VerificationError("GitHub API path must be absolute")
        request = urllib.request.Request(
            "https://api.github.com" + path,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self._token}",
                "User-Agent": "meta-agent-source-snapshot-verifier",
                "X-GitHub-Api-Version": GITHUB_API_VERSION,
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                payload = json.load(response)
        except urllib.error.HTTPError as error:
            raise VerificationError(
                f"GitHub API returned HTTP {error.code} for {path}"
            ) from error
        except (urllib.error.URLError, TimeoutError) as error:
            raise VerificationError(f"GitHub API request failed for {path}") from error
        if not isinstance(payload, dict):
            raise VerificationError(f"GitHub API returned a non-object for {path}")
        return payload

    def commit(self, sha: str) -> Mapping[str, Any]:
        return self.get(f"/repos/{self._repository}/git/commits/{sha}")

    def tree(self, sha: str) -> Mapping[str, Any]:
        return self.get(f"/repos/{self._repository}/git/trees/{sha}")

    def blob(self, sha: str) -> Mapping[str, Any]:
        return self.get(f"/repos/{self._repository}/git/blobs/{sha}")


def require_sha(value: Any, identity: str) -> str:
    if not isinstance(value, str) or SHA_PATTERN.fullmatch(value) is None:
        raise VerificationError(f"{identity} must be a 40-character lowercase SHA")
    return value


def require_sha256(value: str, identity: str) -> str:
    normalized = value.lower()
    if SHA256_PATTERN.fullmatch(normalized) is None:
        raise VerificationError(f"{identity} must be a 64-character SHA-256")
    return normalized


def require_tree_entries(
    payload: Mapping[str, Any], identity: str
) -> Sequence[Mapping[str, Any]]:
    if payload.get("truncated") is True:
        raise VerificationError(f"{identity} tree response is truncated")
    raw_entries = payload.get("tree")
    if not isinstance(raw_entries, list):
        raise VerificationError(f"{identity} tree response is missing entries")
    entries: list[Mapping[str, Any]] = []
    for entry in raw_entries:
        if not isinstance(entry, dict):
            raise VerificationError(f"{identity} tree contains a non-object entry")
        entries.append(entry)
    return entries


def exact_entry(
    entries: Sequence[Mapping[str, Any]],
    *,
    path: str,
    entry_type: str,
    identity: str,
) -> Mapping[str, Any]:
    matches = [
        entry
        for entry in entries
        if entry.get("path") == path and entry.get("type") == entry_type
    ]
    if len(matches) != 1:
        raise VerificationError(
            f"{identity} must contain exactly one {entry_type} entry named {path!r}"
        )
    require_sha(matches[0].get("sha"), f"{identity}/{path} SHA")
    return matches[0]


def decode_blob(payload: Mapping[str, Any], identity: str) -> bytes:
    if payload.get("encoding") != "base64":
        raise VerificationError(f"{identity} blob must use base64 transport encoding")
    content = payload.get("content")
    if not isinstance(content, str) or not content.strip():
        raise VerificationError(f"{identity} blob content is missing")
    try:
        return base64.b64decode(content, validate=False)
    except (ValueError, TypeError) as error:
        raise VerificationError(f"{identity} blob transport base64 is invalid") from error


def parse_expected_heads(values: Iterable[str]) -> Mapping[str, str]:
    result: dict[str, str] = {}
    for value in values:
        if "=" not in value:
            raise VerificationError("expected head must use ref=sha syntax")
        ref, raw_sha = value.split("=", 1)
        if not ref.startswith("refs/heads/") or ref in result:
            raise VerificationError(f"invalid or duplicate expected ref: {ref!r}")
        result[ref] = require_sha(raw_sha, f"expected SHA for {ref}")
    if not result:
        raise VerificationError("at least one expected head is required")
    return result


def parse_expected_auxiliary_heads(values: Iterable[str]) -> Mapping[str, str]:
    result: dict[str, str] = {}
    for value in values:
        if "=" not in value:
            raise VerificationError(
                "expected auxiliary head must use ref=sha syntax"
            )
        ref, raw_sha = value.split("=", 1)
        if ref not in ALLOWED_AUXILIARY_REFS or ref in result:
            raise VerificationError(
                f"invalid or duplicate expected auxiliary ref: {ref!r}"
            )
        result[ref] = require_sha(raw_sha, f"expected SHA for auxiliary ref {ref}")
    return result


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def write_private(path: Path, value: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(value)
    path.chmod(stat.S_IRUSR | stat.S_IWUSR)


def run_git(
    command: Sequence[str], *, cwd: Path | None = None
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            ["git", *command],
            cwd=cwd,
            check=True,
            capture_output=True,
            text=True,
            env={**os.environ, "GIT_TERMINAL_PROMPT": "0"},
        )
    except FileNotFoundError as error:
        raise VerificationError("git executable is unavailable") from error
    except subprocess.CalledProcessError as error:
        stderr = error.stderr.strip()
        suffix = f": {stderr}" if stderr else ""
        raise VerificationError(f"git {' '.join(command)} failed{suffix}") from error


def parse_bundle_heads(bundle_path: Path) -> Mapping[str, str]:
    completed = run_git(["bundle", "list-heads", str(bundle_path)])
    observed: dict[str, str] = {}
    for line in completed.stdout.splitlines():
        fields = line.split(maxsplit=1)
        if len(fields) != 2:
            raise VerificationError("git bundle list-heads returned malformed output")
        sha = require_sha(fields[0], f"bundle ref {fields[1]} SHA")
        ref = fields[1]
        if ref in observed:
            raise VerificationError(f"bundle contains duplicate ref {ref}")
        observed[ref] = sha
    return observed


def validate_bundle_heads(
    observed_heads: Mapping[str, str],
    expected_heads: Mapping[str, str],
    expected_auxiliary_heads: Mapping[str, str],
) -> tuple[Mapping[str, str], Mapping[str, str]]:
    """Validate publishable branches separately from non-pushable pseudo-refs."""

    branch_heads = {
        ref: sha for ref, sha in observed_heads.items() if ref.startswith("refs/heads/")
    }
    auxiliary_heads = {
        ref: sha for ref, sha in observed_heads.items() if not ref.startswith("refs/heads/")
    }
    if branch_heads != dict(expected_heads):
        raise VerificationError(
            "bundle branch refs do not exactly match the reviewed branch inventory"
        )

    unsupported = sorted(set(auxiliary_heads) - ALLOWED_AUXILIARY_REFS)
    if unsupported:
        raise VerificationError(
            "bundle contains unsupported auxiliary refs: " + ", ".join(unsupported)
        )
    if auxiliary_heads != dict(expected_auxiliary_heads):
        raise VerificationError(
            "bundle auxiliary refs do not exactly match the reviewed auxiliary inventory"
        )

    expected_shas = frozenset(expected_heads.values())
    for ref, sha in auxiliary_heads.items():
        if sha not in expected_shas:
            raise VerificationError(
                f"bundle auxiliary ref {ref} points outside the reviewed branch SHAs"
            )
    return branch_heads, auxiliary_heads


def reconstruct_and_verify(
    *,
    client: GitHubClient,
    source_sha: str,
    expected_bundle_sha256: str,
    expected_publisher_sha256: str,
    expected_heads: Mapping[str, str],
    output_dir: Path,
    expected_auxiliary_heads: Mapping[str, str] | None = None,
) -> SnapshotResult:
    expected_auxiliary_heads = dict(expected_auxiliary_heads or {})
    commit = client.commit(source_sha)
    if require_sha(commit.get("sha"), "source commit SHA") != source_sha:
        raise VerificationError("source commit response does not match requested SHA")
    commit_tree = commit.get("tree")
    if not isinstance(commit_tree, dict):
        raise VerificationError("source commit is missing its tree object")
    root_tree_sha = require_sha(commit_tree.get("sha"), "source root tree SHA")

    root_entries = require_tree_entries(client.tree(root_tree_sha), "root")
    scripts_sha = require_sha(
        exact_entry(
            root_entries,
            path="scripts",
            entry_type="tree",
            identity="root",
        ).get("sha"),
        "scripts tree SHA",
    )

    scripts_entries = require_tree_entries(client.tree(scripts_sha), "scripts")
    fleet_sha = require_sha(
        exact_entry(
            scripts_entries,
            path="critical-org-fleet",
            entry_type="tree",
            identity="scripts",
        ).get("sha"),
        "critical-org-fleet tree SHA",
    )

    fleet_entries = require_tree_entries(client.tree(fleet_sha), "critical-org-fleet")
    assets_sha = require_sha(
        exact_entry(
            fleet_entries,
            path="assets",
            entry_type="tree",
            identity="critical-org-fleet",
        ).get("sha"),
        "assets tree SHA",
    )
    publisher_sha = require_sha(
        exact_entry(
            fleet_entries,
            path=PUBLISHER_NAME,
            entry_type="blob",
            identity="critical-org-fleet",
        ).get("sha"),
        "publisher blob SHA",
    )

    asset_entries = require_tree_entries(client.tree(assets_sha), "assets")
    sealed_parts = sorted(
        (
            entry
            for entry in asset_entries
            if entry.get("type") == "blob"
            and isinstance(entry.get("path"), str)
            and ASSET_NAME_PATTERN.fullmatch(str(entry["path"])) is not None
        ),
        key=lambda entry: str(entry["path"]),
    )
    if not sealed_parts:
        raise VerificationError("assets tree contains no sealed meta.part files")

    encoded_bundle = bytearray()
    for entry in sealed_parts:
        path = str(entry["path"])
        blob_sha = require_sha(entry.get("sha"), f"assets/{path} blob SHA")
        encoded_bundle.extend(decode_blob(client.blob(blob_sha), f"assets/{path}"))
    try:
        bundle_bytes = base64.b64decode(bytes(encoded_bundle), validate=False)
    except (ValueError, TypeError) as error:
        raise VerificationError("sealed bundle base64 is invalid") from error
    if not bundle_bytes:
        raise VerificationError("sealed bundle decoded to an empty payload")
    observed_bundle_sha256 = sha256_bytes(bundle_bytes)
    if observed_bundle_sha256 != expected_bundle_sha256:
        raise VerificationError(
            "sealed bundle SHA-256 does not match the reviewed digest"
        )

    publisher_bytes = decode_blob(client.blob(publisher_sha), PUBLISHER_NAME)
    observed_publisher_sha256 = sha256_bytes(publisher_bytes)
    if observed_publisher_sha256 != expected_publisher_sha256:
        raise VerificationError(
            "publisher SHA-256 does not match the reviewed digest"
        )
    try:
        compile(publisher_bytes.decode("utf-8"), PUBLISHER_NAME, "exec")
    except (UnicodeDecodeError, SyntaxError) as error:
        raise VerificationError("publisher is not valid UTF-8 Python source") from error

    output_dir.mkdir(parents=True, exist_ok=True)
    output_dir.chmod(stat.S_IRWXU)
    bundle_path = output_dir / "meta-agent-control-plane-den-1057.bundle"
    publisher_path = output_dir / PUBLISHER_NAME
    write_private(bundle_path, bundle_bytes)
    write_private(publisher_path, publisher_bytes)

    repository_context = output_dir / "bundle-verification.git"
    run_git(["init", "--bare", "--quiet", str(repository_context)])
    run_git(["-C", str(repository_context), "bundle", "verify", str(bundle_path)])
    observed_heads = parse_bundle_heads(bundle_path)
    branch_heads, auxiliary_heads = validate_bundle_heads(
        observed_heads,
        expected_heads,
        expected_auxiliary_heads,
    )

    return SnapshotResult(
        source_sha=source_sha,
        source_tree_sha=root_tree_sha,
        asset_count=len(sealed_parts),
        bundle_sha256=observed_bundle_sha256,
        publisher_sha256=observed_publisher_sha256,
        heads=branch_heads,
        auxiliary_heads=auxiliary_heads,
        bundle_path=bundle_path,
        publisher_path=publisher_path,
    )


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default=DEFAULT_REPOSITORY)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--bundle-sha256", required=True)
    parser.add_argument("--publisher-sha256", required=True)
    parser.add_argument("--expected-head", action="append", default=[])
    parser.add_argument("--expected-auxiliary-head", action="append", default=[])
    parser.add_argument("--output-dir", type=Path, required=True)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN") or ""
    try:
        source_sha = require_sha(args.source_sha, "source SHA")
        bundle_sha256 = require_sha256(args.bundle_sha256, "bundle SHA-256")
        publisher_sha256 = require_sha256(
            args.publisher_sha256, "publisher SHA-256"
        )
        expected_heads = parse_expected_heads(args.expected_head)
        expected_auxiliary_heads = parse_expected_auxiliary_heads(
            args.expected_auxiliary_head
        )
        for ref, sha in expected_auxiliary_heads.items():
            if sha not in frozenset(expected_heads.values()):
                raise VerificationError(
                    f"expected auxiliary ref {ref} points outside expected branch SHAs"
                )
        result = reconstruct_and_verify(
            client=GitHubClient(token, args.repository),
            source_sha=source_sha,
            expected_bundle_sha256=bundle_sha256,
            expected_publisher_sha256=publisher_sha256,
            expected_heads=expected_heads,
            expected_auxiliary_heads=expected_auxiliary_heads,
            output_dir=args.output_dir.resolve(),
        )
    except VerificationError as error:
        print(f"meta-agent-source-snapshot status=failed reason={error}", file=sys.stderr)
        return 1
    print(result.sanitized_json())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
