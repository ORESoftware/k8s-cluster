#!/usr/bin/env python3
"""Fail-closed three-way reconciliation for the reviewed Messaging Intel product."""

# Reconcile from the immutable atomic-capacity dev commit. Later dev movement
# is allowed only when this reviewed commit remains its ancestor.
# The branch-local StreemPilot fixtures and ratchets are synchronized to dev.

from __future__ import annotations

import pathlib
import subprocess
import tempfile

from den977_lib_resolver_fix import LIB_PATH, PLANNER_PATH, resolve_lib, resolve_planner
from den977_semantic_resolvers import RESOLVERS, WORKFLOW_PATH
from den977_workflow_resolver_fix import resolve_workflow

RESOLVERS[WORKFLOW_PATH] = resolve_workflow
RESOLVERS[LIB_PATH] = resolve_lib

CURRENT_DEV_SHA = "a14072064f25d7b49807656d4231f93d335a6d55"
REVIEWED_MSGINT_SHA = "bf4fca2e22937caf18a07dc1bd7c4494fff4b95c"
EXPECTED_MERGE_BASE = "4e701d8c9208956fe0890df1107168e032f335c3"

PRODUCT_PATHS = [
    ".github/workflows/gha-clone-server.yml",
    "docs/gha-profile-repository-admission.md",
    "remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml",
    "remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml",
    "remote/deployments/build-server-rs/readme.md",
    "remote/deployments/build-server-rs/src/profiles.rs",
    "remote/deployments/build-server-rs/src/validation.rs",
    "remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/package-lock.json",
    "remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/package.json",
    "remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/src/operator-config.mjs",
    "remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/test/operator-config.test.mjs",
    "remote/deployments/gha-clone-server-rs/README.md",
    LIB_PATH,
    "remote/deployments/gha-clone-server-rs/src/msgint_contract.rs",
    PLANNER_PATH,
    "remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml",
    "remote/deployments/gha-clone-server-rs/tests/msgint_exact_contract.rs",
    "remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs",
    "remote/deployments/gha-clone-server-rs/tests/planner_adversarial.rs",
    "remote/tests/general/gha-clone-msgint-config.test.ts",
    "remote/tests/general/gha-clone-server-config.test.ts",
]


def git(*args: str, check: bool = True) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        ["git", *args],
        check=check,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def show(ref: str, path: str) -> bytes | None:
    result = git("show", f"{ref}:{path}", check=False)
    if result.returncode == 0:
        return result.stdout
    if result.returncode == 128:
        return None
    raise SystemExit(
        f"git show failed for {ref}:{path}: "
        + result.stderr.decode("utf-8", errors="replace")
    )


def conflict_excerpt(data: bytes) -> str:
    lines = data.decode("utf-8", errors="replace").splitlines()
    excerpts: list[str] = []
    for index, line in enumerate(lines):
        if line.startswith("<<<<<<<"):
            excerpts.append("\n".join(lines[max(0, index - 6) : index + 60]))
    return "\n--- conflict excerpt ---\n".join(excerpts)[:24000]


def main() -> None:
    latest_dev = git("rev-parse", "refs/remotes/origin/dev").stdout.decode().strip()
    if git(
        "merge-base",
        "--is-ancestor",
        CURRENT_DEV_SHA,
        latest_dev,
        check=False,
    ).returncode != 0:
        raise SystemExit(
            f"reviewed dev commit {CURRENT_DEV_SHA} is not an ancestor of {latest_dev}"
        )
    reviewed = git(
        "rev-parse", "refs/remotes/origin/agent/den-1550-msgint-stable-final"
    ).stdout.decode().strip()
    if reviewed != REVIEWED_MSGINT_SHA:
        raise SystemExit(f"reviewed Messaging Intel source moved to {reviewed}")
    merge_base = git("merge-base", CURRENT_DEV_SHA, REVIEWED_MSGINT_SHA).stdout.decode().strip()
    if merge_base != EXPECTED_MERGE_BASE:
        raise SystemExit(f"unexpected semantic merge base: {merge_base}")

    merged: dict[str, bytes | None] = {}
    conflicts: list[tuple[str, str]] = []

    with tempfile.TemporaryDirectory(prefix="den-977-msgint-") as temp:
        root = pathlib.Path(temp)
        for index, path in enumerate(PRODUCT_PATHS):
            base_data = show(merge_base, path)
            current_data = show(CURRENT_DEV_SHA, path)
            reviewed_data = show(REVIEWED_MSGINT_SHA, path)

            if path == PLANNER_PATH:
                merged[path] = resolve_planner(
                    show(CURRENT_DEV_SHA, LIB_PATH),
                    show(merge_base, LIB_PATH),
                    reviewed_data,
                )
                continue

            if reviewed_data is None:
                if current_data == base_data:
                    merged[path] = None
                else:
                    conflicts.append(
                        (path, "reviewed deletion overlaps a current-dev modification")
                    )
                continue

            if base_data is None:
                if current_data is None or current_data == reviewed_data:
                    merged[path] = reviewed_data
                else:
                    conflicts.append((path, "independent add/add conflict"))
                continue

            if current_data is None:
                if reviewed_data == base_data:
                    merged[path] = None
                else:
                    conflicts.append(
                        (path, "current-dev deletion overlaps a reviewed modification")
                    )
                continue

            current_file = root / f"{index}.current"
            base_file = root / f"{index}.base"
            reviewed_file = root / f"{index}.reviewed"
            current_file.write_bytes(current_data)
            base_file.write_bytes(base_data)
            reviewed_file.write_bytes(reviewed_data)
            result = git(
                "merge-file",
                "-p",
                "--diff3",
                str(current_file),
                str(base_file),
                str(reviewed_file),
                check=False,
            )
            if result.returncode == 0:
                merged[path] = result.stdout
            elif result.returncode > 0 and path in RESOLVERS:
                merged[path] = RESOLVERS[path](current_data, reviewed_data)
            elif result.returncode > 0:
                conflicts.append((path, conflict_excerpt(result.stdout)))
            else:
                raise SystemExit(
                    f"git merge-file failed for {path}: "
                    + result.stderr.decode("utf-8", errors="replace")
                )

    if conflicts:
        print("Semantic reconciliation stopped; no product file was written.")
        for path, detail in conflicts:
            print(f"::group::CONFLICT {path}")
            print(detail)
            print("::endgroup::")
        raise SystemExit(f"{len(conflicts)} unresolved semantic conflict(s)")

    if set(merged) != set(PRODUCT_PATHS):
        missing = sorted(set(PRODUCT_PATHS) - set(merged))
        extra = sorted(set(merged) - set(PRODUCT_PATHS))
        raise SystemExit(f"reconciliation path-set drift: missing={missing}, extra={extra}")

    for path, data in merged.items():
        target = pathlib.Path(path)
        if data is None:
            target.unlink(missing_ok=True)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)


if __name__ == "__main__":
    main()
