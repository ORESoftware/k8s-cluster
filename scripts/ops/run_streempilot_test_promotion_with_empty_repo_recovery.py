#!/usr/bin/env python3
"""Run the DEN-896 test-first promotion with bounded staging recovery.

GitHub returns HTTP 409 (``Git Repository is empty.``) for a newly created
repository before its first ref exists. The reviewed publisher treated that as
an unexpected API failure and stopped after creating the first staging repo.

The failed run originally left one known private repository in
StreemPilot-test. A later bounded attempt installed that repository's exact
sealed ``main`` before a downstream check stopped. This wrapper therefore
permits only two states for that immutable repository identity:

* no ``main`` ref and repository size zero, which may receive its first sealed
  history in place; or
* the exact expected sealed ``main`` SHA, which is treated as an idempotent
  replay regardless of GitHub's derived repository-size value.

A successful first push can also race GitHub's ref API visibility. The wrapper
permits a small bounded sequence of read-only ref checks only for either that
immutable recovery identity or an exact allowlisted StreemPilot-test repository
that this same stage process observed absent before creating. Any wrong SHA
fails immediately, exhaustion fails closed, and production never receives this
exception.

GitHub preserves repository-login identity case-insensitively and may return a
normalized owner spelling that differs from the reviewed constant. Evidence is
canonicalized only after the base publisher has validated the exact repository
identity, private visibility, main branch, immutable repository id, and sealed
SHA. Repository-name drift still fails closed.

The supplied publication credential can create and push repositories but does
not have GitHub's separate delete-repository permission. Deletion is neither
needed nor desirable. If its primary GitHub REST quota is already exhausted,
the wrapper consults the non-counting ``/rate_limit`` endpoint, honors the
returned core reset even when its remaining counter is stale, waits once with a
small leeway and a hard 3,700-second cap, then retries the exact request once.
Every other existing empty repository remains fail-closed, every nonempty
history must exactly match its sealed SHA, and no production repository has a
recovery exception.
"""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path
import sys
import time


MODULE_PATH = Path(__file__).with_name("publish_streempilot_test_then_promote.py")
SPEC = importlib.util.spec_from_file_location(
    "streempilot_test_then_promote_base",
    MODULE_PATH,
)
if SPEC is None or SPEC.loader is None:
    raise SystemExit(f"unable to load {MODULE_PATH}")
BASE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BASE
SPEC.loader.exec_module(BASE)

ORIGINAL_API = BASE.api
ORIGINAL_MAIN_REF = BASE.main_ref
ORIGINAL_EXISTING_REPOSITORY = BASE.existing_repository
ORIGINAL_PUSH_EXACT_MAIN = BASE.push_exact_main
ORIGINAL_PUBLISH_ONE = BASE.publish_one

RECOVERABLE_EMPTY_STAGE = {
    "full_name": "StreemPilot-test/streempilot-compositor.rs",
    "repository_id": 1327442276,
    "created_at": "2026-08-08T05:25:37Z",
    "expected_sha": "ea7c1c8042122b4e4a7689aee026113fb607421d",
}

POST_PUSH_REF_ATTEMPTS = 6
POST_PUSH_REF_DELAY_SECONDS = 1.0
RATE_LIMIT_MAX_WAIT_SECONDS = 3700
RATE_LIMIT_RESET_LEEWAY_SECONDS = 5
_RECOVERABLE_EMPTY_APPROVED = False
_CURRENT_TARGET: str | None = None
_CURRENT_RUN_STAGE_CREATIONS: set[tuple[str, str]] = set()
_RATE_LIMIT_WAIT_USED = False


def fail(message: str) -> None:
    raise RuntimeError(message)


def _core_rate_limit(payload: object) -> tuple[int, int]:
    if not isinstance(payload, dict):
        fail("GitHub rate-limit response is not an object")
    resources = payload.get("resources")
    if not isinstance(resources, dict):
        fail("GitHub rate-limit response is missing resources")
    core = resources.get("core")
    if not isinstance(core, dict):
        fail("GitHub rate-limit response is missing the core resource")
    remaining = core.get("remaining")
    reset = core.get("reset")
    if (
        isinstance(remaining, bool)
        or not isinstance(remaining, int)
        or remaining < 0
    ):
        fail(f"GitHub core rate-limit remaining value is invalid: {remaining!r}")
    if isinstance(reset, bool) or not isinstance(reset, int) or reset <= 0:
        fail(f"GitHub core rate-limit reset value is invalid: {reset!r}")
    return remaining, reset


def rate_limit_aware_api(
    method: str,
    path: str,
    body: dict[str, object] | None = None,
) -> tuple[int, object | None]:
    """Wait once for an explicit primary REST-limit 403, then retry once."""
    global _RATE_LIMIT_WAIT_USED
    try:
        return ORIGINAL_API(method, path, body)
    except RuntimeError as error:
        message = str(error)
        if (
            _RATE_LIMIT_WAIT_USED
            or "GitHub API 403" not in message
            or "API rate limit exceeded" not in message
        ):
            raise

    status, payload = ORIGINAL_API("GET", "/rate_limit")
    if status != 200:
        fail(f"unable to inspect GitHub core rate limit: HTTP {status}")
    remaining, reset = _core_rate_limit(payload)
    now = int(time.time())
    # The triggering request is an explicit authenticated primary-limit 403.
    # GitHub's /rate_limit remaining field can lag that failure, so the reset
    # timestamp—not the stale remaining counter—controls this one bounded wait.
    wait_seconds = max(0, reset - now) + RATE_LIMIT_RESET_LEEWAY_SECONDS
    if wait_seconds > RATE_LIMIT_MAX_WAIT_SECONDS:
        fail(
            "GitHub core rate-limit reset exceeds bounded wait: "
            f"{wait_seconds} > {RATE_LIMIT_MAX_WAIT_SECONDS} seconds"
        )

    _RATE_LIMIT_WAIT_USED = True
    print(
        "WAITING_DEN896_GITHUB_CORE_RATE_LIMIT_RESET "
        f"seconds={wait_seconds} remaining={remaining} reset={reset}"
    )
    time.sleep(wait_seconds)
    return ORIGINAL_API(method, path, body)


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


def _repository_size(payload: dict[str, object]) -> int:
    size = payload.get("size")
    if isinstance(size, bool) or not isinstance(size, int) or size < 0:
        fail(f"recoverable test repository size is invalid: {size!r}")
    return size


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
    _repository_size(payload)
    return payload


def _require_empty_without_main(metadata: dict[str, object]) -> None:
    size = _repository_size(metadata)
    if size != 0:
        fail(
            "recoverable test repository has content without the exact sealed "
            f"main ref: size={size}"
        )


def _stage_creation_key(full_name: str, expected_sha: str) -> tuple[str, str]:
    return full_name.casefold(), expected_sha


def _is_exact_stage_target(full_name: str) -> bool:
    owner, separator, name = full_name.partition("/")
    if separator != "/" or owner.casefold() != str(BASE.STAGE_ORGANIZATION).casefold():
        return False
    canonical = f"{BASE.CANONICAL_ORGANIZATION}/{name}"
    return canonical in BASE.EXPECTED_REPOSITORIES


def _record_current_stage_creation(
    full_name: str,
    expected_sha: str,
    existing: dict[str, object] | None,
) -> dict[str, object] | None:
    if existing is not None or _CURRENT_TARGET != "stage":
        return existing
    if not _is_exact_stage_target(full_name):
        fail(f"refusing to record repository outside exact staging allowlist: {full_name}")
    _CURRENT_RUN_STAGE_CREATIONS.add(
        _stage_creation_key(full_name, expected_sha)
    )
    return None


def prepare_failed_empty_stage_repository() -> str:
    """Approve an exact empty repo or preserve its already-exact sealed main."""
    global _RECOVERABLE_EMPTY_APPROVED
    _RECOVERABLE_EMPTY_APPROVED = False

    full_name = str(RECOVERABLE_EMPTY_STAGE["full_name"])
    status, payload = BASE.api("GET", f"/repos/{full_name}")
    if status == 404:
        return "absent"
    if status != 200:
        fail(f"unable to inspect recoverable test repository: HTTP {status}")
    metadata = _validate_recovery_metadata(payload)

    actual = safe_main_ref(full_name)
    expected_sha = str(RECOVERABLE_EMPTY_STAGE["expected_sha"])
    if actual == expected_sha:
        print(
            "VERIFIED_DEN896_RECOVERABLE_TEST_REPOSITORY_ALREADY_EXACT "
            f"{metadata['full_name']} id={metadata['id']} "
            f"size={_repository_size(metadata)} sha={actual}"
        )
        return "already-exact"
    if actual is not None:
        fail(
            "refusing to initialize non-empty recoverable test repository: "
            f"{actual} != {expected_sha}"
        )

    _require_empty_without_main(metadata)
    _RECOVERABLE_EMPTY_APPROVED = True
    print(
        "APPROVED_DEN896_EMPTY_TEST_REPOSITORY_INITIALIZATION "
        f"{metadata['full_name']} id={metadata['id']}"
    )
    return "approved-empty"


def recovery_existing_repository(
    full_name: str,
    expected_sha: str,
) -> dict[str, object] | None:
    """Preserve exact repos and record only same-run allowlisted stage creation."""
    recoverable_name = str(RECOVERABLE_EMPTY_STAGE["full_name"])
    recoverable_sha = str(RECOVERABLE_EMPTY_STAGE["expected_sha"])
    is_recoverable_identity = (
        full_name.casefold() == recoverable_name.casefold()
        and expected_sha == recoverable_sha
    )

    if not is_recoverable_identity or not _RECOVERABLE_EMPTY_APPROVED:
        existing = ORIGINAL_EXISTING_REPOSITORY(full_name, expected_sha)
        return _record_current_stage_creation(
            full_name,
            expected_sha,
            existing,
        )

    status, payload = BASE.api("GET", f"/repos/{full_name}")
    if status != 200:
        fail(f"approved empty test repository disappeared: HTTP {status}")
    metadata = _validate_recovery_metadata(payload)
    actual = safe_main_ref(full_name)
    if actual == expected_sha:
        return metadata
    if actual is not None:
        fail(
            "approved empty test repository changed before initialization: "
            f"{actual} != {expected_sha}"
        )
    _require_empty_without_main(metadata)

    # Returning None intentionally enters the reviewed create/reconcile path.
    # ensure_private_repository re-reads the exact identity, returns the already
    # private repository without mutation, then push_exact_main installs its
    # first sealed history. No repository deletion is required.
    return None


def _post_push_retry_kind(full_name: str, expected_sha: str) -> str | None:
    recoverable_name = str(RECOVERABLE_EMPTY_STAGE["full_name"])
    recoverable_sha = str(RECOVERABLE_EMPTY_STAGE["expected_sha"])
    if (
        _RECOVERABLE_EMPTY_APPROVED
        and full_name.casefold() == recoverable_name.casefold()
        and expected_sha == recoverable_sha
    ):
        return "immutable-recovery"

    if _CURRENT_TARGET == "stage":
        key = _stage_creation_key(full_name, expected_sha)
        if _is_exact_stage_target(full_name) and key in _CURRENT_RUN_STAGE_CREATIONS:
            return "current-run-stage-creation"
    return None


def recovery_push_exact_main(
    local_repository: Path,
    full_name: str,
    expected_sha: str,
) -> None:
    """Retry only API visibility after an approved successful staging push."""
    try:
        ORIGINAL_PUSH_EXACT_MAIN(local_repository, full_name, expected_sha)
        return
    except RuntimeError as error:
        expected_failure = (
            f"remote verification failed for {full_name}: None != {expected_sha}"
        )
        retry_kind = _post_push_retry_kind(full_name, expected_sha)
        if retry_kind is None or str(error) != expected_failure:
            raise

    for attempt in range(1, POST_PUSH_REF_ATTEMPTS + 1):
        actual = safe_main_ref(full_name)
        if actual == expected_sha:
            marker = (
                "VERIFIED_DEN896_FIRST_PUSH_AFTER_BOUNDED_REF_RETRY"
                if retry_kind == "immutable-recovery"
                else "VERIFIED_DEN896_CURRENT_RUN_STAGE_FIRST_PUSH_AFTER_BOUNDED_REF_RETRY"
            )
            print(f"{marker} {full_name} attempt={attempt}")
            return
        if actual is not None:
            fail(
                "approved staging repository changed after first push: "
                f"{actual} != {expected_sha}"
            )
        if attempt < POST_PUSH_REF_ATTEMPTS:
            time.sleep(POST_PUSH_REF_DELAY_SECONDS)

    fail(
        "remote verification remained absent after successful first push for "
        f"{full_name} after {POST_PUSH_REF_ATTEMPTS} bounded checks"
    )


def canonical_evidence_publish_one(
    source_root: Path,
    record: dict[str, object],
    target: str,
) -> dict[str, object]:
    """Canonicalize only GitHub's case-insensitive owner spelling in evidence."""
    row = ORIGINAL_PUBLISH_ONE(source_root, record, target)
    expected_target = BASE.target_full_name(record, target)
    observed_target = row.get("target_full_name")
    if (
        not isinstance(observed_target, str)
        or observed_target.casefold() != expected_target.casefold()
    ):
        fail(
            "publisher evidence target escaped the exact repository identity: "
            f"{observed_target!r} != {expected_target}"
        )
    normalized = dict(row)
    normalized["target_full_name"] = expected_target
    return normalized


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=("stage", "production"))
    parser.add_argument("--evidence-out", type=Path, required=True)
    parser.add_argument("--stage-evidence", type=Path)
    return parser.parse_args()


def main() -> int:
    global _CURRENT_TARGET, _RATE_LIMIT_WAIT_USED
    args = parse_args()
    _CURRENT_TARGET = args.target
    _CURRENT_RUN_STAGE_CREATIONS.clear()
    _RATE_LIMIT_WAIT_USED = False
    BASE.api = rate_limit_aware_api

    if args.target == "stage":
        prepare_failed_empty_stage_repository()
    elif args.stage_evidence is None:
        fail("production promotion requires --stage-evidence")

    BASE.main_ref = safe_main_ref
    BASE.existing_repository = recovery_existing_repository
    BASE.push_exact_main = recovery_push_exact_main
    BASE.publish_one = canonical_evidence_publish_one
    BASE.publish(
        args.target,
        args.evidence_out.resolve(),
        stage_evidence=(
            args.stage_evidence.resolve()
            if args.stage_evidence is not None
            else None
        ),
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
