#!/usr/bin/env python3
"""Recover only publisher-owned, partially initialized DEN-3786 repositories.

This is intentionally narrower than a generic tag repair tool. It compares the
remote Git tree with the locally materialized, Zed-validated fleet and permits
only these recovery states:

1. an empty repository, which is left for the normal publisher;
2. a marker-only bootstrap commit, which is completed with the exact expected
   tree and a direct child initial commit;
3. an exact full initial tree with a missing v0.1.0 tag; or
4. an exact full initial tree whose v0.1.0 tag still points at its direct,
   marker-only bootstrap parent.

Any other branch, tree, marker, commit, tag, visibility, or ancestry state fails
closed. No credential is accepted on the command line.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import stat
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

API = "https://api.github.com"
API_VERSION = "2022-11-28"
TRACKING = "DEN-3786"
TAG = "v0.1.0"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
ORGANIZATIONS = ("elenkos-systems", "elenkos-systems-test")
REPOSITORIES = (
    "elenkos-interfaces",
    "elenkos-lib-core",
    "elenkos-sync",
    "elenkos-api-server.rs",
    "elenkos-web-server.rs",
    "elenkos-cli",
    "elenkos-clients",
    "elenkos-flutter",
    "elenkos-desktop-app.rs",
    "elenkos-infra",
    "elenkos-monorepo",
)


class ApiError(RuntimeError):
    def __init__(self, method: str, path: str, status: int, document: Any):
        super().__init__(f"GitHub {method} {path} failed HTTP {status}: {document!r}")
        self.method = method
        self.path = path
        self.status = status
        self.document = document


class GitHub:
    def __init__(self, token: str) -> None:
        self.token = token

    def request(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        allow: Iterable[int] = (),
    ) -> tuple[int, Any]:
        payload = None if body is None else json.dumps(body, separators=(",", ":")).encode()
        request = urllib.request.Request(
            API + path,
            data=payload,
            method=method,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self.token}",
                "X-GitHub-Api-Version": API_VERSION,
                "User-Agent": "elenkos-partial-bootstrap-recovery/1",
                **({"Content-Type": "application/json"} if payload is not None else {}),
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=45) as response:
                raw = response.read()
                return response.status, json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            raw = error.read(32768)
            try:
                document: Any = json.loads(raw) if raw else None
            except json.JSONDecodeError:
                document = raw.decode("utf-8", "replace")[:2000]
            if error.code in set(allow):
                return error.code, document
            raise ApiError(method, path, error.code, document) from None

    def get(self, path: str, allow: Iterable[int] = ()) -> tuple[int, Any]:
        return self.request("GET", path, allow=allow)

    def post(self, path: str, body: dict[str, Any], allow: Iterable[int] = ()) -> tuple[int, Any]:
        return self.request("POST", path, body, allow)

    def patch(self, path: str, body: dict[str, Any], allow: Iterable[int] = ()) -> tuple[int, Any]:
        return self.request("PATCH", path, body, allow)


@dataclass(frozen=True)
class ExpectedRepository:
    organization: str
    name: str
    root: Path
    files: dict[str, tuple[str, str]]
    marker: dict[str, Any]

    @property
    def full_name(self) -> str:
        return f"{self.organization}/{self.name}"

    @property
    def bootstrap_message(self) -> str:
        return f"chore: initialize {self.name} ({TRACKING})"

    @property
    def initial_message(self) -> str:
        return f"feat: initialize {self.name} ({TRACKING})"


def git_blob_sha(content: bytes) -> str:
    digest = hashlib.sha1()
    digest.update(f"blob {len(content)}\0".encode("ascii"))
    digest.update(content)
    return digest.hexdigest()


def load_expected(fleet_root: Path, organization: str, name: str) -> ExpectedRepository:
    root = fleet_root / organization / name
    if not root.is_dir():
        raise RuntimeError(f"missing materialized repository: {root}")
    files: dict[str, tuple[str, str]] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file() or ".git" in path.parts:
            continue
        relative = path.relative_to(root).as_posix()
        content = path.read_bytes()
        mode = "100755" if relative.startswith(("scripts/", "bin/")) else "100644"
        files[relative] = (git_blob_sha(content), mode)
    marker_path = root / ".elenkos-bootstrap.json"
    marker = json.loads(marker_path.read_text(encoding="utf-8"))
    expected_marker = {
        "schema_version": 1,
        "organization": organization,
        "repository": name,
        "visibility": "private",
        "tracking_issue": TRACKING,
        "blind_review_contract": "ai-hidden-until-human-submit",
    }
    drift = {
        key: {"expected": value, "observed": marker.get(key)}
        for key, value in expected_marker.items()
        if marker.get(key) != value
    }
    if drift:
        raise RuntimeError(f"local bootstrap marker drift for {organization}/{name}: {drift}")
    if len(files) <= 8:
        raise RuntimeError(f"materialized repository unexpectedly small: {organization}/{name}")
    return ExpectedRepository(organization, name, root, files, marker)


def require_document(status: int, document: Any, expected: int, operation: str) -> dict[str, Any]:
    if status != expected or not isinstance(document, dict):
        raise RuntimeError(f"{operation} failed: HTTP {status} document={document!r}")
    return document


def read_ref(api: GitHub, full_name: str, ref: str, *, empty_allowed: bool = False) -> str | None:
    encoded = urllib.parse.quote(ref, safe="/")
    status, document = api.get(f"/repos/{full_name}/git/ref/{encoded}", allow=(404, 409))
    if status == 404:
        return None
    if status == 409 and empty_allowed:
        if not isinstance(document, dict) or document.get("message") != "Git Repository is empty.":
            raise RuntimeError(f"unexpected empty-ref conflict for {full_name}: {document!r}")
        return None
    payload = require_document(status, document, 200, f"read {ref} for {full_name}")
    obj = payload.get("object")
    sha = obj.get("sha") if isinstance(obj, dict) else None
    kind = obj.get("type") if isinstance(obj, dict) else None
    if not isinstance(sha, str) or SHA_RE.fullmatch(sha) is None or kind != "commit":
        raise RuntimeError(f"invalid {ref} object for {full_name}: {obj!r}")
    return sha


def poll_ref(api: GitHub, full_name: str, ref: str, expected: str) -> None:
    for _ in range(20):
        observed = read_ref(api, full_name, ref)
        if observed == expected:
            return
        time.sleep(1)
    raise RuntimeError(f"{ref} did not converge for {full_name}: expected {expected}")


def read_commit(api: GitHub, full_name: str, sha: str) -> dict[str, Any]:
    status, document = api.get(f"/repos/{full_name}/git/commits/{sha}")
    commit = require_document(status, document, 200, f"read commit {sha} for {full_name}")
    if commit.get("sha") != sha:
        raise RuntimeError(f"commit SHA mismatch for {full_name}: {commit.get('sha')!r} != {sha}")
    return commit


def read_tree(api: GitHub, full_name: str, tree_sha: str) -> dict[str, tuple[str, str]]:
    status, document = api.get(f"/repos/{full_name}/git/trees/{tree_sha}?recursive=1")
    payload = require_document(status, document, 200, f"read tree {tree_sha} for {full_name}")
    if payload.get("truncated") is True:
        raise RuntimeError(f"remote tree is truncated for {full_name}")
    entries = payload.get("tree")
    if not isinstance(entries, list):
        raise RuntimeError(f"remote tree entries invalid for {full_name}")
    files: dict[str, tuple[str, str]] = {}
    for entry in entries:
        if not isinstance(entry, dict) or entry.get("type") != "blob":
            continue
        path = entry.get("path")
        sha = entry.get("sha")
        mode = entry.get("mode")
        if not isinstance(path, str) or not isinstance(sha, str) or not isinstance(mode, str):
            raise RuntimeError(f"remote tree entry invalid for {full_name}: {entry!r}")
        files[path] = (sha, mode)
    return files


def commit_tree(api: GitHub, expected: ExpectedRepository, sha: str) -> tuple[dict[str, Any], dict[str, tuple[str, str]]]:
    commit = read_commit(api, expected.full_name, sha)
    tree = commit.get("tree")
    tree_sha = tree.get("sha") if isinstance(tree, dict) else None
    if not isinstance(tree_sha, str) or SHA_RE.fullmatch(tree_sha) is None:
        raise RuntimeError(f"commit tree SHA invalid for {expected.full_name}: {tree!r}")
    return commit, read_tree(api, expected.full_name, tree_sha)


def verify_marker_blob(expected: ExpectedRepository, files: dict[str, tuple[str, str]]) -> None:
    marker = files.get(".elenkos-bootstrap.json")
    local_marker = expected.files[".elenkos-bootstrap.json"]
    if marker != local_marker:
        raise RuntimeError(
            f"bootstrap marker blob drift for {expected.full_name}: {marker!r} != {local_marker!r}"
        )


def create_full_commit(api: GitHub, expected: ExpectedRepository, parent_sha: str) -> str:
    entries: list[dict[str, str]] = []
    for relative, (_, mode) in expected.files.items():
        content = (expected.root / relative).read_bytes()
        status, blob_document = api.post(
            f"/repos/{expected.full_name}/git/blobs",
            {"content": base64.b64encode(content).decode("ascii"), "encoding": "base64"},
        )
        blob = require_document(status, blob_document, 201, f"create blob {relative} for {expected.full_name}")
        sha = blob.get("sha")
        if not isinstance(sha, str) or SHA_RE.fullmatch(sha) is None:
            raise RuntimeError(f"blob SHA invalid for {expected.full_name}/{relative}")
        if sha != expected.files[relative][0]:
            raise RuntimeError(f"blob SHA drift for {expected.full_name}/{relative}")
        entries.append({"path": relative, "mode": mode, "type": "blob", "sha": sha})

    status, tree_document = api.post(
        f"/repos/{expected.full_name}/git/trees",
        {"tree": entries},
    )
    tree = require_document(status, tree_document, 201, f"create full tree for {expected.full_name}")
    tree_sha = tree.get("sha")
    if not isinstance(tree_sha, str) or SHA_RE.fullmatch(tree_sha) is None:
        raise RuntimeError(f"full tree SHA invalid for {expected.full_name}")

    status, commit_document = api.post(
        f"/repos/{expected.full_name}/git/commits",
        {"message": expected.initial_message, "tree": tree_sha, "parents": [parent_sha]},
    )
    commit = require_document(status, commit_document, 201, f"create full commit for {expected.full_name}")
    commit_sha = commit.get("sha")
    if not isinstance(commit_sha, str) or SHA_RE.fullmatch(commit_sha) is None:
        raise RuntimeError(f"full commit SHA invalid for {expected.full_name}")

    status, document = api.patch(
        f"/repos/{expected.full_name}/git/refs/heads/main",
        {"sha": commit_sha, "force": False},
        allow=(422,),
    )
    if status == 422:
        observed = read_ref(api, expected.full_name, "heads/main", empty_allowed=True)
        if observed is None:
            raise RuntimeError(f"main disappeared while completing {expected.full_name}: {document!r}")
        _, observed_files = commit_tree(api, expected, observed)
        if observed_files != expected.files:
            raise RuntimeError(f"main raced to unexpected tree for {expected.full_name}: {document!r}")
        return observed
    require_document(status, document, 200, f"advance main for {expected.full_name}")
    poll_ref(api, expected.full_name, "heads/main", commit_sha)
    _, observed_files = commit_tree(api, expected, commit_sha)
    if observed_files != expected.files:
        raise RuntimeError(f"completed main tree drift for {expected.full_name}")
    return commit_sha


def ensure_initial_tag(
    api: GitHub,
    expected: ExpectedRepository,
    main_sha: str,
    main_commit: dict[str, Any],
    tag_sha: str | None,
) -> str:
    if tag_sha == main_sha:
        return "ready"
    if tag_sha is None:
        status, document = api.post(
            f"/repos/{expected.full_name}/git/refs",
            {"ref": f"refs/tags/{TAG}", "sha": main_sha},
            allow=(422,),
        )
        if status not in {201, 422}:
            raise RuntimeError(f"create {TAG} failed for {expected.full_name}: HTTP {status}")
        poll_ref(api, expected.full_name, f"tags/{TAG}", main_sha)
        return "created"

    parents = main_commit.get("parents")
    parent_shas = [item.get("sha") for item in parents if isinstance(item, dict)] if isinstance(parents, list) else []
    if parent_shas != [tag_sha]:
        raise RuntimeError(
            f"refusing non-parent tag repair for {expected.full_name}: tag={tag_sha} parents={parent_shas}"
        )
    tag_commit, tag_files = commit_tree(api, expected, tag_sha)
    if tag_commit.get("message") != expected.bootstrap_message:
        raise RuntimeError(
            f"refusing tag repair from non-bootstrap commit for {expected.full_name}: "
            f"{tag_commit.get('message')!r}"
        )
    if set(tag_files) != {".elenkos-bootstrap.json"}:
        raise RuntimeError(f"refusing tag repair from non-marker tree for {expected.full_name}")
    verify_marker_blob(expected, tag_files)
    status, document = api.patch(
        f"/repos/{expected.full_name}/git/refs/tags/{TAG}",
        {"sha": main_sha, "force": True},
    )
    require_document(status, document, 200, f"repair {TAG} for {expected.full_name}")
    poll_ref(api, expected.full_name, f"tags/{TAG}", main_sha)
    return "moved-from-bootstrap-parent"


def recover_repository(api: GitHub, expected: ExpectedRepository) -> str:
    status, repository_document = api.get(f"/repos/{expected.full_name}", allow=(404,))
    if status == 404:
        return "absent"
    repository = require_document(status, repository_document, 200, f"read repository {expected.full_name}")
    if repository.get("private") is not True or repository.get("visibility") != "private":
        raise RuntimeError(f"repository visibility drift for {expected.full_name}")
    if repository.get("default_branch") not in {"main", None}:
        raise RuntimeError(f"repository default branch drift for {expected.full_name}")

    main_sha = read_ref(api, expected.full_name, "heads/main", empty_allowed=True)
    if main_sha is None:
        return "empty"
    main_commit, main_files = commit_tree(api, expected, main_sha)
    verify_marker_blob(expected, main_files)
    tag_sha = read_ref(api, expected.full_name, f"tags/{TAG}")

    if main_files == expected.files:
        if main_commit.get("message") != expected.initial_message:
            raise RuntimeError(
                f"full tree has unexpected commit message for {expected.full_name}: "
                f"{main_commit.get('message')!r}"
            )
        tag_action = ensure_initial_tag(api, expected, main_sha, main_commit, tag_sha)
        return f"full-tree:{tag_action}"

    if set(main_files) == {".elenkos-bootstrap.json"}:
        if main_commit.get("message") != expected.bootstrap_message:
            raise RuntimeError(
                f"marker-only tree has unexpected commit message for {expected.full_name}: "
                f"{main_commit.get('message')!r}"
            )
        if tag_sha not in {None, main_sha}:
            raise RuntimeError(
                f"marker-only repository has unexpected tag for {expected.full_name}: {tag_sha}"
            )
        completed_sha = create_full_commit(api, expected, main_sha)
        completed_commit, completed_files = commit_tree(api, expected, completed_sha)
        if completed_files != expected.files or completed_commit.get("message") != expected.initial_message:
            raise RuntimeError(f"completed repository verification failed for {expected.full_name}")
        tag_action = ensure_initial_tag(api, expected, completed_sha, completed_commit, tag_sha)
        return f"completed-marker-only:{tag_action}"

    missing = sorted(set(expected.files) - set(main_files))[:10]
    extra = sorted(set(main_files) - set(expected.files))[:10]
    changed = sorted(
        path for path in set(expected.files) & set(main_files) if expected.files[path] != main_files[path]
    )[:10]
    raise RuntimeError(
        f"refusing unexpected main tree for {expected.full_name}: "
        f"missing={missing} extra={extra} changed={changed}"
    )


def load_token(path: Path) -> str:
    mode = stat.S_IMODE(path.stat().st_mode)
    if mode != 0o600:
        raise RuntimeError(f"token file must be mode 0600, observed {mode:04o}")
    token = path.read_text(encoding="utf-8")
    if not token or token != token.strip() or any(character.isspace() for character in token):
        raise RuntimeError("token file is empty or contains whitespace")
    return token


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--fleet-root", type=Path, required=True)
    parser.add_argument("--token-file", type=Path, required=True)
    args = parser.parse_args()

    fleet_root = args.fleet_root.resolve()
    token_file = args.token_file.resolve()
    api = GitHub(load_token(token_file))
    results: dict[str, str] = {}
    for organization in ORGANIZATIONS:
        for name in REPOSITORIES:
            expected = load_expected(fleet_root, organization, name)
            action = recover_repository(api, expected)
            results[expected.full_name] = action
            print(f"ELENKOS_PARTIAL_BOOTSTRAP_RECOVERY repository={expected.full_name} action={action}")

    mutated = sum(
        action not in {"absent", "empty", "full-tree:ready"} for action in results.values()
    )
    print(
        "ELENKOS_PARTIAL_BOOTSTRAP_RECOVERY_COMPLETE "
        f"repositories={len(results)} mutated={mutated} "
        f"absent={sum(action == 'absent' for action in results.values())} "
        f"empty={sum(action == 'empty' for action in results.values())}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
