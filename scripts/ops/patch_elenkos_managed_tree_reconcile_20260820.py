#!/usr/bin/env python3
from __future__ import annotations

import argparse
from pathlib import Path

ANCHOR = '''def commit_tree(
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
'''

HELPERS = r'''

MARKER_PATH = ".elenkos-bootstrap.json"


def read_blob_bytes(api: GitHub, full_name: str, sha: str) -> bytes:
    status, document = api.get(f"/repos/{full_name}/git/blobs/{sha}")
    payload = require_document(status, document, 200, f"read blob {sha} for {full_name}")
    content = payload.get("content")
    encoding = payload.get("encoding")
    if not isinstance(content, str) or encoding != "base64":
        raise RuntimeError(f"blob encoding invalid for {full_name}: {sha}")
    try:
        return base64.b64decode(content, validate=False)
    except ValueError as error:
        raise RuntimeError(f"blob base64 invalid for {full_name}: {sha}") from error


def managed_marker_document(
    api: GitHub,
    expected: ExpectedRepository,
    files: Mapping[str, tuple[str, str]],
) -> dict[str, Any]:
    marker_entry = files.get(MARKER_PATH)
    if marker_entry is None:
        raise RuntimeError(f"managed marker missing for {expected.full_name}")
    marker_sha, marker_mode = marker_entry
    if marker_mode != "100644":
        raise RuntimeError(
            f"managed marker mode invalid for {expected.full_name}: {marker_mode}"
        )
    try:
        marker = json.loads(
            read_blob_bytes(api, expected.full_name, marker_sha).decode("utf-8")
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError(
            f"managed marker JSON invalid for {expected.full_name}: {error}"
        ) from None
    if not isinstance(marker, dict):
        raise RuntimeError(f"managed marker must be an object for {expected.full_name}")
    if set(marker) != set(expected.marker):
        raise RuntimeError(
            f"managed marker keys drift for {expected.full_name}: "
            f"missing={sorted(set(expected.marker) - set(marker))} "
            f"extra={sorted(set(marker) - set(expected.marker))}"
        )
    stable_drift = {
        key: {"expected": value, "observed": marker.get(key)}
        for key, value in expected.marker.items()
        if key != "source_fingerprint" and marker.get(key) != value
    }
    if stable_drift:
        raise RuntimeError(
            f"managed marker identity drift for {expected.full_name}: "
            f"{json.dumps(stable_drift, sort_keys=True)}"
        )
    fingerprint = marker.get("source_fingerprint")
    if not isinstance(fingerprint, str) or not re.fullmatch(r"[0-9a-f]{64}", fingerprint):
        raise RuntimeError(
            f"managed marker fingerprint invalid for {expected.full_name}: {fingerprint!r}"
        )
    return marker


def observed_source_fingerprint(
    api: GitHub,
    expected: ExpectedRepository,
    files: Mapping[str, tuple[str, str]],
) -> str:
    if set(files) != set(expected.git_files):
        raise RuntimeError(
            f"managed full tree path drift for {expected.full_name}: "
            f"missing={sorted(set(expected.git_files) - set(files))[:10]} "
            f"extra={sorted(set(files) - set(expected.git_files))[:10]}"
        )
    digest = hashlib.sha256()
    for relative in sorted(files):
        actual_sha, actual_mode = files[relative]
        expected_mode = expected.git_files[relative][1]
        if actual_mode != expected_mode:
            raise RuntimeError(
                f"managed full tree mode drift for {expected.full_name}/{relative}: "
                f"{actual_mode} != {expected_mode}"
            )
        if relative == MARKER_PATH:
            continue
        content = read_blob_bytes(api, expected.full_name, actual_sha)
        try:
            content.decode("utf-8")
        except UnicodeDecodeError as error:
            raise RuntimeError(
                f"managed full tree contains non-UTF8 blob for "
                f"{expected.full_name}/{relative}"
            ) from error
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(hashlib.sha256(content).digest())
        digest.update(b"\0")
    return digest.hexdigest()


def verify_managed_full_tree(
    api: GitHub,
    expected: ExpectedRepository,
    commit: Mapping[str, Any],
    files: Mapping[str, tuple[str, str]],
) -> str:
    if commit.get("message") != expected.initial_message:
        raise RuntimeError(
            f"managed full tree commit message drift for {expected.full_name}: "
            f"{commit.get('message')!r}"
        )
    parents = commit.get("parents")
    if not isinstance(parents, list) or len(parents) > 1:
        raise RuntimeError(
            f"managed full tree parent shape invalid for {expected.full_name}: {parents!r}"
        )
    marker = managed_marker_document(api, expected, files)
    observed = observed_source_fingerprint(api, expected, files)
    if marker.get("source_fingerprint") != observed:
        raise RuntimeError(
            f"managed full tree fingerprint mismatch for {expected.full_name}: "
            f"marker={marker.get('source_fingerprint')!r} observed={observed}"
        )
    return observed


def ref_commit_is_managed(
    api: GitHub,
    expected: ExpectedRepository,
    sha: str,
) -> str:
    commit, files = commit_tree(api, expected, sha)
    if set(files) == {MARKER_PATH}:
        if commit.get("message") != expected.bootstrap_message:
            raise RuntimeError(
                f"tag ancestor marker message drift for {expected.full_name}: "
                f"{commit.get('message')!r}"
            )
        managed_marker_document(api, expected, files)
        return "marker-only"
    verify_managed_full_tree(api, expected, commit, files)
    return "managed-full-tree"


def ensure_managed_tag(
    api: GitHub,
    expected: ExpectedRepository,
    main_sha: str,
    main_commit: Mapping[str, Any],
    tag_sha: str | None,
) -> str:
    if tag_sha in {None, main_sha}:
        return ensure_initial_tag(api, expected, main_sha, main_commit, tag_sha)
    parents = main_commit.get("parents")
    parent_shas = (
        [item.get("sha") for item in parents if isinstance(item, dict)]
        if isinstance(parents, list)
        else []
    )
    if parent_shas != [tag_sha]:
        raise RuntimeError(
            f"refusing non-parent managed tag repair for {expected.full_name}: "
            f"tag={tag_sha} parents={parent_shas}"
        )
    source_kind = ref_commit_is_managed(api, expected, tag_sha)
    status, document = api.patch(
        f"/repos/{expected.full_name}/git/refs/tags/{TAG}",
        {"sha": main_sha, "force": True},
    )
    require_document(status, document, 200, f"repair {TAG} for {expected.full_name}")
    poll_ref(api, expected.full_name, f"tags/{TAG}", main_sha)
    return f"moved-from-{source_kind}-parent"
'''

OLD_RECOVER = '''def recover_existing_repository(
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
'''

NEW_RECOVER = '''def recover_existing_repository(
    api: GitHub,
    expected: ExpectedRepository,
    main_sha: str,
) -> tuple[str, str]:
    main_commit, main_files = commit_tree(api, expected, main_sha)
    tag_sha = read_ref(api, expected.full_name, f"tags/{TAG}")

    if main_files == expected.git_files:
        verify_marker_blob(expected, main_files)
        if main_commit.get("message") != expected.initial_message:
            raise RuntimeError(
                f"full live tree has unexpected commit message for {expected.full_name}: "
                f"{main_commit.get('message')!r}"
            )
        tag_action = ensure_managed_tag(api, expected, main_sha, main_commit, tag_sha)
        return f"full-live-tree:{tag_action}", main_sha

    if set(main_files) == {MARKER_PATH}:
        if main_commit.get("message") != expected.bootstrap_message:
            raise RuntimeError(
                f"marker-only tree has unexpected commit message for {expected.full_name}: "
                f"{main_commit.get('message')!r}"
            )
        managed_marker_document(api, expected, main_files)
        if tag_sha not in {None, main_sha}:
            raise RuntimeError(
                f"marker-only repository has unexpected tag for {expected.full_name}: {tag_sha}"
            )
        completed_sha = create_full_commit(api, expected, main_sha)
        completed_commit, completed_files = commit_tree(api, expected, completed_sha)
        if completed_files != expected.git_files or completed_commit.get("message") != expected.initial_message:
            raise RuntimeError(f"completed repository verification failed for {expected.full_name}")
        tag_action = ensure_managed_tag(
            api,
            expected,
            completed_sha,
            completed_commit,
            tag_sha,
        )
        return f"completed-marker-only:{tag_action}", completed_sha

    old_fingerprint = verify_managed_full_tree(api, expected, main_commit, main_files)
    completed_sha = create_full_commit(api, expected, main_sha)
    completed_commit, completed_files = commit_tree(api, expected, completed_sha)
    if completed_files != expected.git_files or completed_commit.get("message") != expected.initial_message:
        raise RuntimeError(f"managed tree reconciliation failed for {expected.full_name}")
    tag_action = ensure_managed_tag(
        api,
        expected,
        completed_sha,
        completed_commit,
        tag_sha,
    )
    return (
        f"reconciled-managed-full-tree:{old_fingerprint[:12]}:{tag_action}",
        completed_sha,
    )
'''


def apply(path: Path) -> str:
    text = path.read_text(encoding="utf-8")
    helper_count = text.count("def verify_managed_full_tree(")
    if helper_count == 0:
        count = text.count(ANCHOR)
        if count != 1:
            raise RuntimeError(f"commit_tree anchor count={count}")
        text = text.replace(ANCHOR, ANCHOR + HELPERS, 1)
    elif helper_count != 1:
        raise RuntimeError(f"managed helper count={helper_count}")

    old_count = text.count(OLD_RECOVER)
    new_count = text.count(NEW_RECOVER)
    if old_count == 1 and new_count == 0:
        text = text.replace(OLD_RECOVER, NEW_RECOVER, 1)
        result = "applied"
    elif old_count == 0 and new_count == 1:
        result = "already-applied"
    else:
        raise RuntimeError(f"recovery contract old={old_count} new={new_count}")
    path.write_text(text, encoding="utf-8")
    compile(text, str(path), "exec")
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--recovery",
        type=Path,
        default=Path("scripts/ops/recover_elenkos_partial_bootstrap_20260820.py"),
    )
    args = parser.parse_args()
    result = apply(args.recovery)
    print(
        "ELENKOS_MANAGED_TREE_RECONCILE_PATCHED "
        f"path={args.recovery} result={result} fail_closed=true"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
