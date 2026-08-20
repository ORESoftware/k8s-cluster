#!/usr/bin/env python3
"""Recover only publisher-owned partial DEN-3786 repository bootstraps.

Expected repository trees are rebuilt in the publisher's *live* mode from the
actual predecessor main SHAs observed on GitHub. This is important: local-mode
Zed manifests use path dependencies, while published manifests pin immutable
Git SHAs and therefore have different source fingerprints.

The recovery remains fail closed. It permits only:

* an exact full live tree whose v0.1.0 tag is already correct;
* an exact full live tree with a missing v0.1.0 tag;
* an exact full live tree whose tag points at its direct marker-only bootstrap
  parent; or
* an exact marker-only bootstrap main, completed with the exact live tree.

The walk stops at the first absent or empty repository. The normal idempotent
publisher then creates that repository and everything after it in dependency
order. No credential is accepted on the command line.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import json
import re
import stat
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any, Iterable, Mapping

API = "https://api.github.com"
API_VERSION = "2022-11-28"
TRACKING = "DEN-3786"
TAG = "v0.1.0"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
EXPECTED_ORGANIZATIONS = ("elenkos-systems", "elenkos-systems-test")
EXPECTED_REPOSITORIES = (
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
        body: Mapping[str, Any] | None = None,
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
                "User-Agent": "elenkos-live-partial-bootstrap-recovery/1",
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

    def post(
        self,
        path: str,
        body: Mapping[str, Any],
        allow: Iterable[int] = (),
    ) -> tuple[int, Any]:
        return self.request("POST", path, body, allow)

    def patch(
        self,
        path: str,
        body: Mapping[str, Any],
        allow: Iterable[int] = (),
    ) -> tuple[int, Any]:
        return self.request("PATCH", path, body, allow)


@dataclass(frozen=True)
class ExpectedRepository:
    organization: str
    name: str
    text_files: dict[str, str]
    git_files: dict[str, tuple[str, str]]
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


def load_spec_module(path: Path) -> ModuleType:
    if not path.is_file():
        raise RuntimeError(f"missing materialized fleet specification: {path}")
    spec = importlib.util.spec_from_file_location("elenkos_live_fleet_spec", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load fleet specification: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def validate_spec_inventory(module: ModuleType) -> list[Any]:
    specs = list(getattr(module, "ALL_SPECS", ()))
    expected = {
        f"{organization}/{repository}"
        for organization in EXPECTED_ORGANIZATIONS
        for repository in EXPECTED_REPOSITORIES
    }
    observed = {str(spec.full_name) for spec in specs}
    if observed != expected or len(specs) != 22:
        raise RuntimeError(
            "materialized fleet inventory drift: "
            f"missing={sorted(expected - observed)} extra={sorted(observed - expected)} "
            f"count={len(specs)}"
        )
    return specs


def expected_repository(
    module: ModuleType,
    spec: Any,
    pins: Mapping[str, str],
) -> ExpectedRepository:
    files = dict(module.build_repository_files(spec, pins=pins, mode="live"))
    marker_text = files.get(".elenkos-bootstrap.json")
    if not isinstance(marker_text, str):
        raise RuntimeError(f"live bootstrap marker missing for {spec.full_name}")
    marker = json.loads(marker_text)
    expected_marker = {
        "schema_version": 1,
        "organization": spec.organization,
        "repository": spec.name,
        "visibility": "private",
        "tracking_issue": TRACKING,
        "blind_review_contract": "ai-hidden-until-human-submit",
        "zed_dependency_graph": True,
    }
    drift = {
        key: {"expected": value, "observed": marker.get(key)}
        for key, value in expected_marker.items()
        if marker.get(key) != value
    }
    if drift:
        raise RuntimeError(f"live bootstrap marker drift for {spec.full_name}: {drift}")
    fingerprint = marker.get("source_fingerprint")
    if not isinstance(fingerprint, str) or not re.fullmatch(r"[0-9a-f]{64}", fingerprint):
        raise RuntimeError(f"live source fingerprint invalid for {spec.full_name}")
    git_files: dict[str, tuple[str, str]] = {}
    for relative, text in files.items():
        if not isinstance(relative, str) or not isinstance(text, str) or not text:
            raise RuntimeError(f"invalid live file for {spec.full_name}: {relative!r}")
        mode = "100755" if relative.startswith(("scripts/", "bin/")) else "100644"
        git_files[relative] = (git_blob_sha(text.encode("utf-8")), mode)
    if len(git_files) <= 8:
        raise RuntimeError(f"live repository unexpectedly small: {spec.full_name}")
    return ExpectedRepository(
        organization=str(spec.organization),
        name=str(spec.name),
        text_files=files,
        git_files=git_files,
        marker=marker,
    )


def require_document(
    status: int,
    document: Any,
    expected_status: int,
    operation: str,
) -> dict[str, Any]:
    if status != expected_status or not isinstance(document, dict):
        raise RuntimeError(f"{operation} failed: HTTP {status} document={document!r}")
    return document


def read_ref(
    api: GitHub,
    full_name: str,
    ref: str,
    *,
    empty_allowed: bool = False,
) -> str | None:
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


def commit_tree(
    api: GitHub,
    expected: ExpectedRepository,
    sha: str,
) -> tuple[dict[str, Any], dict[str, tuple[str, str]]]:
    commit = read_commit(api, expected.full_name, sha)
    tree = commit.get("tree")
    tree_sha = tree.get("sha") if isinstance(tree, dict) else None
    if not isinstance(tree_sha, str) or SHA_RE.fullmatch(tree_sha) is None:
        raise RuntimeError(f"commit tree SHA invalid for {expected.full_name}: {tree!r}")
    return commit, read_tree(api, expected.full_name, tree_sha)


def verify_marker_blob(
    expected: ExpectedRepository,
    files: Mapping[str, tuple[str, str]],
) -> None:
    marker = files.get(".elenkos-bootstrap.json")
    expected_marker = expected.git_files[".elenkos-bootstrap.json"]
    if marker != expected_marker:
        raise RuntimeError(
            f"live bootstrap marker blob drift for {expected.full_name}: "
            f"{marker!r} != {expected_marker!r}"
        )


def create_full_commit(api: GitHub, expected: ExpectedRepository, parent_sha: str) -> str:
    entries: list[dict[str, str]] = []
    for relative in sorted(expected.text_files):
        text = expected.text_files[relative]
        expected_sha, mode = expected.git_files[relative]
        status, blob_document = api.post(
            f"/repos/{expected.full_name}/git/blobs",
            {"content": base64.b64encode(text.encode("utf-8")).decode("ascii"), "encoding": "base64"},
        )
        blob = require_document(
            status,
            blob_document,
            201,
            f"create blob {relative} for {expected.full_name}",
        )
        sha = blob.get("sha")
        if sha != expected_sha:
            raise RuntimeError(
                f"blob SHA drift for {expected.full_name}/{relative}: {sha!r} != {expected_sha}"
            )
        entries.append({"path": relative, "mode": mode, "type": "blob", "sha": expected_sha})

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
    commit = require_document(
        status,
        commit_document,
        201,
        f"create full commit for {expected.full_name}",
    )
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
        observed_commit, observed_files = commit_tree(api, expected, observed)
        if observed_files != expected.git_files or observed_commit.get("message") != expected.initial_message:
            raise RuntimeError(f"main raced to unexpected tree for {expected.full_name}: {document!r}")
        return observed
    require_document(status, document, 200, f"advance main for {expected.full_name}")
    poll_ref(api, expected.full_name, "heads/main", commit_sha)
    observed_commit, observed_files = commit_tree(api, expected, commit_sha)
    if observed_files != expected.git_files or observed_commit.get("message") != expected.initial_message:
        raise RuntimeError(f"completed main tree drift for {expected.full_name}")
    return commit_sha


def ensure_initial_tag(
    api: GitHub,
    expected: ExpectedRepository,
    main_sha: str,
    main_commit: Mapping[str, Any],
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
        if status == 422:
            observed = read_ref(api, expected.full_name, f"tags/{TAG}")
            if observed != main_sha:
                raise RuntimeError(
                    f"tag raced to unexpected SHA for {expected.full_name}: {document!r}"
                )
        elif status != 201:
            raise RuntimeError(f"create {TAG} failed for {expected.full_name}: HTTP {status}")
        poll_ref(api, expected.full_name, f"tags/{TAG}", main_sha)
        return "created"

    parents = main_commit.get("parents")
    parent_shas = (
        [item.get("sha") for item in parents if isinstance(item, dict)]
        if isinstance(parents, list)
        else []
    )
    if parent_shas != [tag_sha]:
        raise RuntimeError(
            f"refusing non-parent tag repair for {expected.full_name}: "
            f"tag={tag_sha} parents={parent_shas}"
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


def recover_existing_repository(
    api: GitHub,
    expected: ExpectedRepository,
    main_sha: str,
) -> tuple[str, str]:
    main_commit, main_files = commit_tree(api, expected, main_sha)
    verify_marker_blob(expected, main_files)
    tag_sha = read_ref(api, expected.full_name, f"tags/{TAG}")

    if main_files == expected.git_files:
        if main_commit.get("message") != expected.initial_message:
            raise RuntimeError(
                f"full live tree has unexpected commit message for {expected.full_name}: "
                f"{main_commit.get('message')!r}"
            )
        tag_action = ensure_initial_tag(api, expected, main_sha, main_commit, tag_sha)
        return f"full-live-tree:{tag_action}", main_sha

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
        if completed_files != expected.git_files or completed_commit.get("message") != expected.initial_message:
            raise RuntimeError(f"completed repository verification failed for {expected.full_name}")
        tag_action = ensure_initial_tag(
            api,
            expected,
            completed_sha,
            completed_commit,
            tag_sha,
        )
        return f"completed-marker-only:{tag_action}", completed_sha

    missing = sorted(set(expected.git_files) - set(main_files))[:10]
    extra = sorted(set(main_files) - set(expected.git_files))[:10]
    changed = sorted(
        path
        for path in set(expected.git_files) & set(main_files)
        if expected.git_files[path] != main_files[path]
    )[:10]
    raise RuntimeError(
        f"refusing unexpected live main tree for {expected.full_name}: "
        f"missing={missing} extra={extra} changed={changed}"
    )


def read_token(path: Path) -> str:
    mode = stat.S_IMODE(path.stat().st_mode)
    if mode != 0o600:
        raise RuntimeError(f"token file must be mode 0600, observed {mode:04o}")
    token = path.read_text(encoding="utf-8")
    if not token or token != token.strip() or any(character.isspace() for character in token):
        raise RuntimeError("token file is empty or contains whitespace")
    return token


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--fleet-root",
        type=Path,
        required=True,
        help="Compatibility/output path; live expected trees are rebuilt from the materialized spec.",
    )
    parser.add_argument("--token-file", type=Path, required=True)
    parser.add_argument(
        "--spec-module",
        type=Path,
        default=Path("scripts/ops/elenkos_fleet_spec_20260819.py"),
    )
    args = parser.parse_args()

    module = load_spec_module(args.spec_module.resolve())
    specs = validate_spec_inventory(module)
    api = GitHub(read_token(args.token_file.resolve()))
    pins: dict[str, str] = {}
    inspected = 0
    mutated = 0
    stopped_at: str | None = None

    for spec in specs:
        expected = expected_repository(module, spec, pins)
        status, repository_document = api.get(f"/repos/{expected.full_name}", allow=(404,))
        if status == 404:
            stopped_at = expected.full_name
            print(
                "ELENKOS_LIVE_PARTIAL_RECOVERY_STOP "
                f"repository={expected.full_name} state=absent"
            )
            break
        repository = require_document(
            status,
            repository_document,
            200,
            f"read repository {expected.full_name}",
        )
        if repository.get("private") is not True or repository.get("visibility") != "private":
            raise RuntimeError(f"repository visibility drift for {expected.full_name}")
        if repository.get("default_branch") not in {"main", None}:
            raise RuntimeError(f"repository default branch drift for {expected.full_name}")

        main_sha = read_ref(api, expected.full_name, "heads/main", empty_allowed=True)
        if main_sha is None:
            stopped_at = expected.full_name
            print(
                "ELENKOS_LIVE_PARTIAL_RECOVERY_STOP "
                f"repository={expected.full_name} state=empty"
            )
            break

        action, recovered_sha = recover_existing_repository(api, expected, main_sha)
        pins[expected.full_name] = recovered_sha
        inspected += 1
        if action not in {"full-live-tree:ready"}:
            mutated += 1
        print(
            "ELENKOS_LIVE_PARTIAL_RECOVERY "
            f"repository={expected.full_name} action={action} main={recovered_sha}"
        )

    print(
        "ELENKOS_LIVE_PARTIAL_RECOVERY_COMPLETE "
        f"inspected={inspected} mutated={mutated} "
        f"pins={len(pins)} stopped_at={stopped_at or 'none'}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
