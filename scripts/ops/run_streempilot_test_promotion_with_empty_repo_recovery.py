#!/usr/bin/env python3
"""Run the DEN-896 test-first promotion with one bounded empty-repo recovery.

GitHub returns HTTP 409 (`Git Repository is empty.`) for a newly created
repository before its first ref exists. The reviewed publisher treated that as
an unexpected API failure and stopped after creating the first staging repo.

This wrapper changes only two semantics:

1. that exact 409 is interpreted as "main ref absent" while every other API
   error continues to fail closed; and
2. the one empty StreemPilot-test repository created by failed workflow run
   31241585746 may be deleted/recreated, but only if its immutable repository
   identity and empty/private metadata still exactly match the recorded facts.

No production repository is recoverable through this wrapper.
"""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path
import sys


MODULE_PATH = Path(__file__).with_name("publish_streempilot_test_then_promote.py")
SPEC = importlib.util.spec_from_file_location("streempilot_test_then_promote_base", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise SystemExit(f"unable to load {MODULE_PATH}")
BASE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BASE
SPEC.loader.exec_module(BASE)

ORIGINAL_MAIN_REF = BASE.main_ref

RECOVERABLE_EMPTY_STAGE = {
    "full_name": "StreemPilot-test/streempilot-compositor.rs",
    "repository_id": 1327442276,
    "created_at": "2026-08-08T05:25:37Z",
    "expected_sha": "ea7c1c8042122b4e4a7689aee026113fb607421d",
}


def fail(message: str) -> None:
    raise RuntimeError(message)


def safe_main_ref(full_name: str) -> str | None:
    """Treat only GitHub's exact empty-repository 409 as an absent main ref."""
    try:
        return ORIGINAL_MAIN_REF(full_name)
    except RuntimeError as error:
        message = str(error)
        expected_prefix = (
            f"GitHub API 409 for GET /repos/{full_name}/git/ref/heads/main:"
        )
        if (
            expected_prefix in message
            and '"message":"Git Repository is empty."' in message
        ):
            return None
        raise


def _validate_recovery_metadata(payload: object) -> dict[str, object]:
    expected = RECOVERABLE_EMPTY_STAGE
    if not isinstance(payload, dict):
        fail("recoverable test repository response is not an object")
    full_name = payload.get("full_name")
    if (
        not isinstance(full_name, str)
        or full_name.casefold() != str(expected["full_name"]).casefold()
    ):
        fail(f"recoverable test repository identity changed: {full_name!r}")
    if payload.get("id") != expected["repository_id"]:
        fail(
            "recoverable test repository id changed: "
            f"{payload.get('id')!r} != {expected['repository_id']}"
        )
    if payload.get("created_at") != expected["created_at"]:
        fail("recoverable test repository creation timestamp changed")
    if payload.get("private") is not True or payload.get("visibility") != "private":
        fail("recoverable test repository is not private")
    if payload.get("default_branch") != "main":
        fail("recoverable test repository default branch changed")
    if payload.get("size") != 0:
        fail("recoverable test repository is no longer empty by repository size")
    return payload


def recover_failed_empty_stage_repository() -> str:
    """Delete only the exact empty test repo left by failed run 31241585746."""
    full_name = str(RECOVERABLE_EMPTY_STAGE["full_name"])
    status, payload = BASE.api("GET", f"/repos/{full_name}")
    if status == 404:
        return "absent"
    if status != 200:
        fail(f"unable to inspect recoverable test repository: HTTP {status}")
    _validate_recovery_metadata(payload)

    actual = safe_main_ref(full_name)
    expected_sha = str(RECOVERABLE_EMPTY_STAGE["expected_sha"])
    if actual == expected_sha:
        return "already-exact"
    if actual is not None:
        fail(
            "refusing to delete non-empty recoverable test repository: "
            f"{actual} != {expected_sha}"
        )

    status, _ = BASE.api("DELETE", f"/repos/{full_name}")
    if status != 204:
        fail(f"failed to delete exact failed-run test repository: HTTP {status}")
    status, _ = BASE.api("GET", f"/repos/{full_name}")
    if status != 404:
        fail("deleted failed-run test repository is still visible")
    print(
        "RECOVERED_DEN896_EMPTY_TEST_REPOSITORY "
        f"{full_name} id={RECOVERABLE_EMPTY_STAGE['repository_id']}"
    )
    return "deleted"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=("stage", "production"))
    parser.add_argument("--evidence-out", type=Path, required=True)
    parser.add_argument("--stage-evidence", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.target == "stage":
        recover_failed_empty_stage_repository()
    elif args.stage_evidence is None:
        fail("production promotion requires --stage-evidence")

    # Patch the reviewed publisher only after the one bounded recovery check.
    # Existing empty repositories still fail in BASE.existing_repository because
    # `None != expected sealed SHA`; only freshly created repos reach push_exact_main.
    BASE.main_ref = safe_main_ref
    BASE.publish(
        args.target,
        args.evidence_out.resolve(),
        stage_evidence=(
            args.stage_evidence.resolve() if args.stage_evidence is not None else None
        ),
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
