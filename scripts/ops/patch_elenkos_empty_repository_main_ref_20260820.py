#!/usr/bin/env python3
from __future__ import annotations

import argparse
from pathlib import Path

MAIN_REF_OLD = '''    status, document = api.get(f"/repos/{spec.full_name}/git/ref/heads/main", allow=(404,))
    if status == 404:
        return None
'''

MAIN_REF_NEW = '''    status, document = api.get(
        f"/repos/{spec.full_name}/git/ref/heads/main", allow=(404, 409)
    )
    if status == 404:
        return None
    if status == 409:
        if (
            not isinstance(document, dict)
            or document.get("message") != "Git Repository is empty."
        ):
            raise RuntimeError(
                f"unexpected main-ref conflict for {spec.full_name}: {document!r}"
            )
        return None
'''

INITIALIZE_OLD = '''def initialize_repository(
    api: GitHub,
    spec: RepositorySpec,
    files: Mapping[str, str],
) -> str:
    entries: list[dict[str, str]] = []
    for path in sorted(files):
        blob_sha = create_blob(api, spec, files[path])
        entries.append(
            {
                "path": path,
                "mode": "100755" if path.startswith(("scripts/", "bin/")) else "100644",
                "type": "blob",
                "sha": blob_sha,
            }
        )
    status, tree_document = api.post(
        f"/repos/{spec.full_name}/git/trees",
        {"tree": entries},
    )
    tree = require_document(
        status,
        tree_document,
        expected_status=201,
        operation=f"create initial tree for {spec.full_name}",
    )
    tree_sha = tree.get("sha")
    if not isinstance(tree_sha, str) or SHA_RE.fullmatch(tree_sha) is None:
        raise RuntimeError(f"tree SHA invalid for {spec.full_name}")

    status, commit_document = api.post(
        f"/repos/{spec.full_name}/git/commits",
        {
            "message": f"feat: initialize {spec.name} ({BOOTSTRAP_ISSUE})",
            "tree": tree_sha,
            "parents": [],
        },
    )
    commit = require_document(
        status,
        commit_document,
        expected_status=201,
        operation=f"create initial commit for {spec.full_name}",
    )
    commit_sha = commit.get("sha")
    if not isinstance(commit_sha, str) or SHA_RE.fullmatch(commit_sha) is None:
        raise RuntimeError(f"commit SHA invalid for {spec.full_name}")

    status, _ = api.post(
        f"/repos/{spec.full_name}/git/refs",
        {"ref": "refs/heads/main", "sha": commit_sha},
    )
    if status != 201:
        raise RuntimeError(f"create main ref failed for {spec.full_name}: HTTP {status}")
    observed = main_ref(api, spec)
    if observed != commit_sha:
        raise RuntimeError(
            f"main changed during initialization for {spec.full_name}: {observed} != {commit_sha}"
        )
    return commit_sha
'''

INITIALIZE_NEW = '''def initialize_repository(
    api: GitHub,
    spec: RepositorySpec,
    files: Mapping[str, str],
) -> str:
    bootstrap_path = ".elenkos-bootstrap.json"
    bootstrap_content = files.get(bootstrap_path)
    if not isinstance(bootstrap_content, str):
        raise RuntimeError(f"bootstrap marker missing for {spec.full_name}")

    encoded_bootstrap_path = urllib.parse.quote(bootstrap_path, safe="")
    status, bootstrap_document = api.put(
        f"/repos/{spec.full_name}/contents/{encoded_bootstrap_path}",
        {
            "message": f"chore: initialize {spec.name} ({BOOTSTRAP_ISSUE})",
            "content": base64.b64encode(bootstrap_content.encode("utf-8")).decode("ascii"),
        },
    )
    bootstrap = require_document(
        status,
        bootstrap_document,
        expected_status=201,
        operation=f"initialize empty repository {spec.full_name}",
    )
    bootstrap_commit = bootstrap.get("commit")
    bootstrap_sha = (
        bootstrap_commit.get("sha") if isinstance(bootstrap_commit, dict) else None
    )
    if not isinstance(bootstrap_sha, str) or SHA_RE.fullmatch(bootstrap_sha) is None:
        raise RuntimeError(f"bootstrap commit SHA invalid for {spec.full_name}")
    if main_ref(api, spec) != bootstrap_sha:
        raise RuntimeError(f"bootstrap commit not visible on main for {spec.full_name}")

    entries: list[dict[str, str]] = []
    for path in sorted(files):
        blob_sha = create_blob(api, spec, files[path])
        entries.append(
            {
                "path": path,
                "mode": "100755" if path.startswith(("scripts/", "bin/")) else "100644",
                "type": "blob",
                "sha": blob_sha,
            }
        )
    status, tree_document = api.post(
        f"/repos/{spec.full_name}/git/trees",
        {"tree": entries},
    )
    tree = require_document(
        status,
        tree_document,
        expected_status=201,
        operation=f"create initial tree for {spec.full_name}",
    )
    tree_sha = tree.get("sha")
    if not isinstance(tree_sha, str) or SHA_RE.fullmatch(tree_sha) is None:
        raise RuntimeError(f"tree SHA invalid for {spec.full_name}")

    status, commit_document = api.post(
        f"/repos/{spec.full_name}/git/commits",
        {
            "message": f"feat: initialize {spec.name} ({BOOTSTRAP_ISSUE})",
            "tree": tree_sha,
            "parents": [bootstrap_sha],
        },
    )
    commit = require_document(
        status,
        commit_document,
        expected_status=201,
        operation=f"create initial commit for {spec.full_name}",
    )
    commit_sha = commit.get("sha")
    if not isinstance(commit_sha, str) or SHA_RE.fullmatch(commit_sha) is None:
        raise RuntimeError(f"commit SHA invalid for {spec.full_name}")

    status, _ = api.patch(
        f"/repos/{spec.full_name}/git/refs/heads/main",
        {"sha": commit_sha, "force": False},
    )
    if status != 200:
        raise RuntimeError(f"update main ref failed for {spec.full_name}: HTTP {status}")
    observed = main_ref(api, spec)
    if observed != commit_sha:
        raise RuntimeError(
            f"main changed during initialization for {spec.full_name}: {observed} != {commit_sha}"
        )
    return commit_sha
'''


def replace_exact(text: str, old: str, new: str, label: str) -> tuple[str, str]:
    old_count = text.count(old)
    new_count = text.count(new)
    if old_count == 0 and new_count == 1:
        return text, "already-applied"
    if old_count == 1 and new_count == 0:
        return text.replace(old, new, 1), "applied"
    raise RuntimeError(
        f"refusing unexpected Elenkos publisher {label} contract: "
        f"old={old_count} new={new_count}"
    )


def patch(path: Path) -> tuple[str, str]:
    text = path.read_text(encoding="utf-8")
    text, main_ref_result = replace_exact(
        text, MAIN_REF_OLD, MAIN_REF_NEW, "main_ref"
    )
    text, initialize_result = replace_exact(
        text,
        INITIALIZE_OLD,
        INITIALIZE_NEW,
        "empty-repository initializer",
    )
    path.write_text(text, encoding="utf-8")
    compile(text, str(path), "exec")
    return main_ref_result, initialize_result


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Initialize a new empty GitHub repository with the Contents API "
            "before using raw Git-object endpoints for the deterministic fleet tree."
        )
    )
    parser.add_argument(
        "--publisher",
        type=Path,
        default=Path("scripts/ops/publish_elenkos_fleet_20260819.py"),
    )
    args = parser.parse_args()
    main_ref_result, initialize_result = patch(args.publisher)
    print(
        "ELENKOS_EMPTY_REPOSITORY_BOOTSTRAP_PATCHED "
        f"path={args.publisher} main_ref={main_ref_result} "
        f"initializer={initialize_result} contents_api=true"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
