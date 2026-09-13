#!/usr/bin/env python3
"""Materialize the audited NCC source archive into the exact ten private repos."""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import os
import stat
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path, PurePosixPath
from typing import Any

API = "https://api.github.com"
OWNER = "networking-components"
EXPECTED_LOGIN = "ORESoftware"
EXPECTED_REPOSITORIES = (
    "ncc-dhcp-server",
    "ncc-ipam",
    "ncc-firewall",
    "ncc-forward-proxy",
    "ncc-ntp",
    "ncc-stun-turn",
    "ncc-service-discovery",
    "ncc-network-controller",
    "ncc-observability",
    "ncc-pki",
)
EXPECTED_FILES_PER_REPOSITORY = 25
MAX_ARCHIVE_BYTES = 64 * 1024
MAX_EXPANDED_BYTES = 2 * 1024 * 1024
MAX_FILE_BYTES = 128 * 1024


class GitHubError(RuntimeError):
    def __init__(self, method: str, path: str, status: int, message: str):
        super().__init__(f"GitHub {method} {path} returned {status}: {message[:500]}")
        self.status = status


def request(
    token: str,
    method: str,
    path: str,
    payload: Any | None = None,
    allow: tuple[int, ...] = (),
) -> tuple[int, Any | None]:
    body = None if payload is None else json.dumps(payload, separators=(",", ":")).encode()
    headers = {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "User-Agent": "networking-components-source-materializer/1",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if body is not None:
        headers["Content-Type"] = "application/json"
    for attempt in range(7):
        req = urllib.request.Request(API + path, data=body, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=90) as response:
                raw = response.read()
                return response.status, json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            raw = error.read(8192)
            try:
                message = json.loads(raw).get("message", "unknown error")
            except Exception:
                message = raw.decode(errors="replace")
            if error.code in allow:
                return error.code, None
            if error.code in (429, 500, 502, 503, 504) and attempt < 6:
                time.sleep(min(2 ** (attempt + 1), 30))
                continue
            raise GitHubError(method, path, error.code, str(message)) from error
        except urllib.error.URLError as error:
            if attempt < 6:
                time.sleep(min(2 ** (attempt + 1), 30))
                continue
            raise RuntimeError(f"GitHub transport failed: {error}") from error
    raise AssertionError("unreachable")


def get(token: str, path: str, allow: tuple[int, ...] = ()) -> tuple[int, Any | None]:
    return request(token, "GET", path, allow=allow)


def post(token: str, path: str, payload: Any) -> tuple[int, Any | None]:
    return request(token, "POST", path, payload)


def patch(token: str, path: str, payload: Any) -> tuple[int, Any | None]:
    return request(token, "PATCH", path, payload)


def encoded(value: str) -> str:
    return urllib.parse.quote(value, safe="")


def load_request(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema_version") != 1 or data.get("execute") is not True:
        raise RuntimeError("request is not an executable schema-v1 source request")
    if data.get("organization") != OWNER:
        raise RuntimeError("organization is outside the bounded source request")
    if tuple(data.get("repositories", ())) != EXPECTED_REPOSITORIES:
        raise RuntimeError("repository allowlist does not exactly match the materializer")
    if data.get("repository_count") != len(EXPECTED_REPOSITORIES):
        raise RuntimeError("repository count is invalid")
    if data.get("files_per_repository") != EXPECTED_FILES_PER_REPOSITORY:
        raise RuntimeError("expected file count is invalid")
    if data.get("total_file_count") != len(EXPECTED_REPOSITORIES) * EXPECTED_FILES_PER_REPOSITORY:
        raise RuntimeError("total file count is invalid")
    carrier = data.get("carrier")
    if not isinstance(carrier, dict):
        raise RuntimeError("carrier metadata is missing")
    if carrier.get("repository") != "networking-components/ncc-e2e":
        raise RuntimeError("carrier repository is outside the bounded request")
    if carrier.get("branch") != "agent/ncc-source-carrier-20260805":
        raise RuntimeError("carrier branch is outside the bounded request")
    if carrier.get("commit_sha") != "53791f1137063935b60ca49224d20dd7145c1c28":
        raise RuntimeError("carrier commit is not the reviewed source commit")
    expected_parts = [
        {"path": ".private-payloads/ncc-source-20260805.part-00.bin", "sha": "8f31cb8f077cba5d58ec01fc21bb8caea6389982"},
        {"path": ".private-payloads/ncc-source-20260805.part-01.bin", "sha": "c5a501e82766f3012ab63a410ce3950d40894101"},
        {"path": ".private-payloads/ncc-source-20260805.part-02.bin", "sha": "8b96aed220b64da1a69a90df4cf6cbdf86d64f83"},
        {"path": ".private-payloads/ncc-source-20260805.part-03.bin", "sha": "82973b00029be4d3683cba522b0826f124327fa1"},
    ]
    if carrier.get("parts") != expected_parts:
        raise RuntimeError("carrier blob allowlist does not exactly match the materializer")
    if carrier.get("archive_sha256") != "a6fe66dff5e36d8d5eaee09f3ccd188b3e60e8a26b121e01a7fdf3148f17dfa1":
        raise RuntimeError("archive digest is not the reviewed source digest")
    if carrier.get("archive_bytes") != 20636:
        raise RuntimeError("archive byte count is invalid")
    return data


def verify_identity(token: str) -> None:
    _, profile = get(token, "/user")
    if not isinstance(profile, dict) or profile.get("login") != EXPECTED_LOGIN:
        login = profile.get("login") if isinstance(profile, dict) else None
        raise RuntimeError(f"unexpected publisher identity: {login!r}")
    _, membership = get(token, f"/user/memberships/orgs/{OWNER}")
    observed = (
        membership.get("role") if isinstance(membership, dict) else None,
        membership.get("state") if isinstance(membership, dict) else None,
    )
    if observed != ("admin", "active"):
        raise RuntimeError(f"{OWNER} owner membership is {observed!r}")


def fetch_carrier(token: str, request_data: dict[str, Any]) -> bytes:
    carrier = request_data["carrier"]
    repo = carrier["repository"]
    owner, name = repo.split("/", 1)
    branch_ref = encoded(f"heads/{carrier['branch']}")
    _, ref = get(token, f"/repos/{owner}/{name}/git/ref/{branch_ref}")
    ref_sha = ref.get("object", {}).get("sha") if isinstance(ref, dict) else None
    if ref_sha != carrier["commit_sha"]:
        raise RuntimeError("carrier branch moved away from the reviewed commit")

    _, commit = get(token, f"/repos/{owner}/{name}/git/commits/{carrier['commit_sha']}")
    tree_sha = commit.get("tree", {}).get("sha") if isinstance(commit, dict) else None
    if not isinstance(tree_sha, str):
        raise RuntimeError("carrier commit has no tree")
    _, tree = get(token, f"/repos/{owner}/{name}/git/trees/{tree_sha}?recursive=1")
    entries = tree.get("tree") if isinstance(tree, dict) else None
    if not isinstance(entries, list):
        raise RuntimeError("carrier tree is malformed")
    observed = [
        {"path": item.get("path"), "sha": item.get("sha")}
        for item in entries
        if item.get("type") == "blob"
    ]
    if observed != carrier["parts"]:
        raise RuntimeError("carrier tree does not exactly match the reviewed blob sequence")

    chunks: list[bytes] = []
    for part in carrier["parts"]:
        _, blob = get(token, f"/repos/{owner}/{name}/git/blobs/{part['sha']}")
        if not isinstance(blob, dict) or blob.get("encoding") != "base64":
            raise RuntimeError(f"carrier blob {part['sha']} has an unexpected encoding")
        content = blob.get("content")
        if not isinstance(content, str):
            raise RuntimeError(f"carrier blob {part['sha']} has no content")
        chunks.append(base64.b64decode(content, validate=False))
    archive = b"".join(chunks)
    if len(archive) != carrier["archive_bytes"] or len(archive) > MAX_ARCHIVE_BYTES:
        raise RuntimeError("carrier archive byte count failed validation")
    if hashlib.sha256(archive).hexdigest() != carrier["archive_sha256"]:
        raise RuntimeError("carrier archive digest failed validation")
    return archive


def safe_extract(archive: bytes, destination: Path) -> None:
    total = 0
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:xz") as bundle:
        members = bundle.getmembers()
        for member in members:
            path = PurePosixPath(member.name)
            if path.is_absolute() or not path.parts or ".." in path.parts:
                raise RuntimeError(f"unsafe archive path: {member.name!r}")
            if path.parts[0] != "repos":
                raise RuntimeError(f"archive path escapes the repos root: {member.name!r}")
            if member.issym() or member.islnk() or member.isdev() or member.isfifo():
                raise RuntimeError(f"unsupported archive member: {member.name!r}")
            target = destination.joinpath(*path.parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            if not member.isfile():
                raise RuntimeError(f"unsupported archive member type: {member.name!r}")
            if member.size < 0 or member.size > MAX_FILE_BYTES:
                raise RuntimeError(f"archive member has an invalid size: {member.name!r}")
            total += member.size
            if total > MAX_EXPANDED_BYTES:
                raise RuntimeError("expanded archive exceeds the bounded size")
            target.parent.mkdir(parents=True, exist_ok=True)
            source = bundle.extractfile(member)
            if source is None:
                raise RuntimeError(f"unable to read archive member: {member.name!r}")
            data = source.read(MAX_FILE_BYTES + 1)
            if len(data) != member.size or len(data) > MAX_FILE_BYTES:
                raise RuntimeError(f"archive member read failed validation: {member.name!r}")
            target.write_bytes(data)
            target.chmod(0o755 if member.mode & 0o111 else 0o644)


def source_entries(repo_root: Path) -> list[dict[str, str]]:
    entries: list[dict[str, str]] = []
    for path in sorted(repo_root.rglob("*")):
        if path.is_symlink():
            raise RuntimeError(f"source contains a symlink: {path}")
        if not path.is_file():
            continue
        relative = path.relative_to(repo_root).as_posix()
        if relative.startswith(".git/") or relative == ".git":
            raise RuntimeError("source contains Git metadata")
        data = path.read_bytes()
        if len(data) > MAX_FILE_BYTES:
            raise RuntimeError(f"source file exceeds the bounded size: {relative}")
        try:
            content = data.decode("utf-8")
        except UnicodeDecodeError as error:
            raise RuntimeError(f"source file is not UTF-8 text: {relative}") from error
        entries.append(
            {
                "path": relative,
                "mode": "100755" if path.stat().st_mode & stat.S_IXUSR else "100644",
                "type": "blob",
                "content": content,
            }
        )
    if len(entries) != EXPECTED_FILES_PER_REPOSITORY:
        raise RuntimeError(
            f"{repo_root.name} has {len(entries)} files, expected {EXPECTED_FILES_PER_REPOSITORY}"
        )
    return entries


def get_ref_sha(token: str, repository: str, branch: str) -> str:
    _, data = get(token, f"/repos/{OWNER}/{repository}/git/ref/{encoded(f'heads/{branch}')}")
    sha = data.get("object", {}).get("sha") if isinstance(data, dict) else None
    if not isinstance(sha, str) or len(sha) != 40:
        raise RuntimeError(f"{OWNER}/{repository} has no valid {branch} ref")
    return sha


def get_commit_tree(token: str, repository: str, commit_sha: str) -> str:
    _, data = get(token, f"/repos/{OWNER}/{repository}/git/commits/{commit_sha}")
    sha = data.get("tree", {}).get("sha") if isinstance(data, dict) else None
    if not isinstance(sha, str) or len(sha) != 40:
        raise RuntimeError(f"{OWNER}/{repository} commit {commit_sha} has no valid tree")
    return sha


def is_placeholder_tree(token: str, repository: str, tree_sha: str) -> bool:
    _, data = get(token, f"/repos/{OWNER}/{repository}/git/trees/{tree_sha}?recursive=1")
    entries = data.get("tree") if isinstance(data, dict) else None
    if not isinstance(entries, list):
        raise RuntimeError(f"{OWNER}/{repository} tree {tree_sha} is malformed")
    blobs = [entry for entry in entries if entry.get("type") == "blob"]
    non_blobs = [entry for entry in entries if entry.get("type") not in ("blob", "tree")]
    return (
        not non_blobs
        and len(blobs) == 1
        and blobs[0].get("path") == "README.md"
        and blobs[0].get("mode") == "100644"
    )


def update_ref(token: str, repository: str, branch: str, sha: str) -> None:
    patch(
        token,
        f"/repos/{OWNER}/{repository}/git/refs/heads/{branch}",
        {"sha": sha, "force": False},
    )


def materialize_repository(token: str, repository: str, repo_root: Path) -> str:
    entries = source_entries(repo_root)
    _, tree = post(token, f"/repos/{OWNER}/{repository}/git/trees", {"tree": entries})
    expected_tree = tree.get("sha") if isinstance(tree, dict) else None
    if not isinstance(expected_tree, str) or len(expected_tree) != 40:
        raise RuntimeError(f"GitHub did not create a valid source tree for {repository}")

    main_sha = get_ref_sha(token, repository, "main")
    dev_sha = get_ref_sha(token, repository, "dev")
    main_tree = get_commit_tree(token, repository, main_sha)
    dev_tree = get_commit_tree(token, repository, dev_sha)

    if main_tree == expected_tree and dev_tree == expected_tree:
        if main_sha != dev_sha:
            raise RuntimeError(f"{OWNER}/{repository} has divergent commits for the same source tree")
        disposition = "PRESERVED"
        target_sha = main_sha
    elif main_tree == expected_tree and is_placeholder_tree(token, repository, dev_tree):
        target_sha = main_sha
        update_ref(token, repository, "dev", target_sha)
        disposition = "REPAIRED"
    elif dev_tree == expected_tree and is_placeholder_tree(token, repository, main_tree):
        target_sha = dev_sha
        update_ref(token, repository, "main", target_sha)
        disposition = "REPAIRED"
    else:
        if not is_placeholder_tree(token, repository, main_tree):
            raise RuntimeError(f"refusing to replace non-placeholder main tree in {OWNER}/{repository}")
        if not is_placeholder_tree(token, repository, dev_tree):
            raise RuntimeError(f"refusing to replace non-placeholder dev tree in {OWNER}/{repository}")
        if main_sha != dev_sha:
            raise RuntimeError(f"placeholder branches diverged in {OWNER}/{repository}")
        _, commit = post(
            token,
            f"/repos/{OWNER}/{repository}/git/commits",
            {
                "message": "Initial component scaffold",
                "tree": expected_tree,
                "parents": [main_sha],
            },
        )
        target_sha = commit.get("sha") if isinstance(commit, dict) else None
        if not isinstance(target_sha, str) or len(target_sha) != 40:
            raise RuntimeError(f"GitHub did not create a valid commit for {repository}")
        update_ref(token, repository, "main", target_sha)
        update_ref(token, repository, "dev", target_sha)
        disposition = "POPULATED"

    verified_main = get_ref_sha(token, repository, "main")
    verified_dev = get_ref_sha(token, repository, "dev")
    if verified_main != target_sha or verified_dev != target_sha:
        raise RuntimeError(f"branch verification failed for {OWNER}/{repository}")
    if get_commit_tree(token, repository, target_sha) != expected_tree:
        raise RuntimeError(f"tree verification failed for {OWNER}/{repository}")
    print(f"{disposition} {OWNER}/{repository} commit={target_sha} files={len(entries)}", flush=True)
    return disposition


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", type=Path, required=True)
    args = parser.parse_args()

    token = os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN") or os.environ.get("GH_TOKEN")
    if not token or any(character.isspace() for character in token):
        raise RuntimeError("a non-whitespace protected GitHub token is required")
    request_data = load_request(args.request)
    verify_identity(token)
    archive = fetch_carrier(token, request_data)

    counts = {"POPULATED": 0, "PRESERVED": 0, "REPAIRED": 0}
    with tempfile.TemporaryDirectory(prefix="ncc-source-") as temp:
        root = Path(temp)
        safe_extract(archive, root)
        repos_root = root / "repos"
        observed_repositories = tuple(sorted(path.name for path in repos_root.iterdir() if path.is_dir()))
        if observed_repositories != tuple(sorted(EXPECTED_REPOSITORIES)):
            raise RuntimeError(f"archive repository set is invalid: {observed_repositories!r}")
        for repository in EXPECTED_REPOSITORIES:
            disposition = materialize_repository(token, repository, repos_root / repository)
            counts[disposition] += 1

    print(
        "SOURCE_POPULATION_COMPLETE "
        f"request_id={request_data['request_id']} populated={counts['POPULATED']} "
        f"repaired={counts['REPAIRED']} preserved={counts['PRESERVED']} "
        f"total={len(EXPECTED_REPOSITORIES)} files={len(EXPECTED_REPOSITORIES) * EXPECTED_FILES_PER_REPOSITORY}",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"SOURCE_POPULATION_FAILED {type(error).__name__}: {error}", file=sys.stderr, flush=True)
        raise
