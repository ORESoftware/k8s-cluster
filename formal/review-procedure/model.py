#!/usr/bin/env python3
"""Bounded segment upload, mirror, pin, retry, and retention-deletion model."""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass, replace
from typing import NoReturn

NEW = 0
PRESIGNED = 1
UPLOADED = 2
DELETE_CLAIMED = 3
DELETED = 4
MIRROR_NONE = 0
MIRROR_COPYING = 1
MIRRORED = 2
MIRROR_DELETE_CLAIMED = 3
MIRROR_DELETED = 4


class ModelViolation(RuntimeError):
    """Raised when a reachable state or transition violates the model contract."""


def fail(message: str) -> NoReturn:
    raise ModelViolation(message)


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


@dataclass(frozen=True, slots=True)
class State:
    status: int = NEW
    primary_exists: bool = False
    verified: bool = False
    pinned: bool = False
    expired: bool = False
    mirror: int = MIRROR_NONE
    mirror_started: bool = False
    mirror_copy_fenced: bool = False


def successors(state: State):
    if state.status == NEW:
        yield "presign", replace(state, status=PRESIGNED)

    if state.status == PRESIGNED:
        # Retrying an idempotent presign request preserves the abstract state.
        yield "presign-retry", state
        if not state.primary_exists:
            yield "client-put", replace(state, primary_exists=True)
        if state.primary_exists:
            yield "verify-head-and-complete", replace(
                state,
                status=UPLOADED,
                verified=True,
            )

    if state.status == UPLOADED:
        # A repeated completion after the verified transition is a no-op.
        yield "complete-retry", state
        if not state.pinned:
            yield "pin", replace(state, pinned=True)
        if state.mirror == MIRROR_NONE and not state.mirror_started:
            yield "mirror-claim", replace(
                state,
                mirror=MIRROR_COPYING,
                mirror_started=True,
            )
        if state.mirror == MIRROR_COPYING:
            yield "mirror-claim-retry", state
            yield "mirror-complete", replace(state, mirror=MIRRORED)
        if state.mirror == MIRRORED:
            yield "mirror-complete-retry", state
        if not state.expired:
            yield "retention-expire", replace(state, expired=True)
        if state.expired and not state.pinned:
            yield "retention-delete-claim", replace(state, status=DELETE_CLAIMED)

    if state.status == DELETE_CLAIMED:
        yield "retention-delete-claim-retry", state
        if state.primary_exists:
            yield "delete-primary", replace(state, primary_exists=False)
        if state.mirror == MIRROR_COPYING:
            # A mirror operation already in flight must either be observed to
            # completion (becoming an inventoried mirror) or be explicitly
            # abandoned behind a durable fence before erasure can treat it as
            # absent and be finalized.
            yield "mirror-copy-complete-after-delete-claim", replace(
                state,
                mirror=MIRRORED,
            )
            yield "mirror-copy-fence-and-abandon-after-delete-claim", replace(
                state,
                mirror=MIRROR_NONE,
                mirror_copy_fenced=True,
            )
        if state.mirror_copy_fenced:
            # A delayed worker may still report completion, but the durable
            # generation/lease fence rejects publication and preserves state.
            yield "mirror-copy-late-complete-rejected", state
        if state.mirror == MIRRORED:
            yield "mirror-delete-claim", replace(
                state,
                mirror=MIRROR_DELETE_CLAIMED,
            )
        if state.mirror == MIRROR_DELETE_CLAIMED:
            yield "mirror-delete-claim-retry", state
            yield "delete-mirror", replace(state, mirror=MIRROR_DELETED)

        mirror_resolved = state.mirror == MIRROR_DELETED or (
            state.mirror == MIRROR_NONE
            and (not state.mirror_started or state.mirror_copy_fenced)
        )
        if not state.primary_exists and mirror_resolved:
            yield "finalize-delete", replace(state, status=DELETED)

    if state.status == DELETED:
        # Replaying finalization cannot resurrect or duplicate data.
        yield "finalize-delete-retry", state
        if state.mirror_copy_fenced:
            # The fence must remain authoritative after finalization too.
            yield "mirror-copy-late-complete-rejected", state


def assert_invariants(state: State) -> None:
    if state.status == UPLOADED:
        require(state.primary_exists, "uploaded segment lost its primary object")
        require(state.verified, "unverified object became authoritative")
    if state.status in {DELETE_CLAIMED, DELETED}:
        require(state.verified, "unverified segment entered deletion")
        require(state.expired, "unexpired segment entered deletion")
        require(not state.pinned, "retention deleted a pinned segment")
    if state.status == DELETED:
        require(not state.primary_exists, "deleted segment still has a primary object")
        require(
            state.mirror in {MIRROR_NONE, MIRROR_DELETED},
            "deleted segment still has an authoritative mirror",
        )
        if state.mirror_started and state.mirror == MIRROR_NONE:
            require(
                state.mirror_copy_fenced,
                "deleted segment forgot an abandoned in-flight mirror without fencing it",
            )
    if state.pinned:
        require(state.status == UPLOADED, "pinned segment left uploaded state")

    if state.mirror != MIRROR_NONE:
        require(state.mirror_started, "mirror state exists without a mirror claim")
        require(not state.mirror_copy_fenced, "fenced mirror remained publishable")
        require(state.verified, "mirror exists for an unverified segment")
    if state.mirror in {MIRROR_COPYING, MIRRORED}:
        require(
            state.status in {UPLOADED, DELETE_CLAIMED},
            "live mirror exists outside uploaded/deletion state",
        )
    if state.mirror == MIRROR_DELETE_CLAIMED:
        require(state.status == DELETE_CLAIMED, "mirror deletion claim escaped deletion state")
    if state.mirror == MIRROR_DELETED:
        require(
            state.status in {DELETE_CLAIMED, DELETED},
            "deleted mirror exists outside deletion state",
        )

    if state.mirror_started and state.mirror == MIRROR_NONE:
        require(
            state.mirror_copy_fenced,
            "started mirror became absent without a durable fence",
        )
    if state.mirror_copy_fenced:
        require(state.mirror_started, "mirror fence exists without a prior mirror claim")
        require(state.mirror == MIRROR_NONE, "fenced mirror is still represented as present")
        require(
            state.status in {DELETE_CLAIMED, DELETED},
            "mirror fence exists outside deletion state",
        )


def assert_transition(action: str, source: State, target: State) -> None:
    if action == "verify-head-and-complete":
        require(source.primary_exists, "completion ran before object existence")
    if action == "mirror-claim":
        require(not source.mirror_started, "mirror claim reused an exhausted generation")
    if action == "retention-delete-claim":
        require(source.expired and not source.pinned, "illegal retention deletion claim")
    if action == "mirror-copy-fence-and-abandon-after-delete-claim":
        require(source.mirror == MIRROR_COPYING, "fenced a mirror that was not copying")
        require(target.mirror_copy_fenced, "abandoned mirror without durable fencing")
    if action == "mirror-copy-late-complete-rejected":
        require(source.mirror_copy_fenced, "late mirror completion lacked a fence")
        require(target == source, "rejected mirror completion changed abstract state")
    if action == "finalize-delete":
        require(not source.primary_exists, "finalized deletion before primary removal")
        require(
            source.mirror in {MIRROR_NONE, MIRROR_DELETED},
            "finalized deletion before mirror removal",
        )
<<<<<<< HEAD
        if source.mirror_started and source.mirror == MIRROR_NONE:
            require(
                source.mirror_copy_fenced,
                "finalized deletion after mirror abandonment without fencing",
            )
=======
>>>>>>> origin/agent/formal-methods-20260730-segment-lifecycle
    if action.endswith("-retry"):
        require(target == source, f"retry transition {action} changed abstract state")


def main() -> None:
    initial = State()
    queue = deque([initial])
    seen = {initial}
    transitions = 0
    action_names: set[str] = set()

    # The abstraction is finite and monotonic, so exhaust every reachable state
    # instead of relying on an arbitrary traversal-depth cutoff.
    while queue:
        state = queue.popleft()
        assert_invariants(state)
        local_actions: set[str] = set()
        for action, target in successors(state):
            require(action not in local_actions, f"duplicate action {action!r} from {state}")
            local_actions.add(action)
            action_names.add(action)
            transitions += 1
            assert_transition(action, state, target)
            assert_invariants(target)
            if target not in seen:
                seen.add(target)
                queue.append(target)

    require(any(state.status == DELETED for state in seen), "no legal erasure path is reachable")
    require(
        any(state.pinned and state.expired for state in seen),
        "pin-versus-expiry safety state was not explored",
    )
    require(
        "mirror-copy-complete-after-delete-claim" in action_names
<<<<<<< HEAD
        and "mirror-copy-fence-and-abandon-after-delete-claim" in action_names,
        "in-flight mirror resolution paths were not explored",
    )
    require(
        "mirror-copy-late-complete-rejected" in action_names,
        "late mirror completion rejection was not explored",
    )
    require(
        any(
            state.status == DELETED
            and state.mirror_started
            and state.mirror_copy_fenced
            for state in seen
        ),
        "fenced mirror-abandonment erasure path was not explored",
    )
=======
        and "mirror-copy-abort-after-delete-claim" in action_names,
        "in-flight mirror resolution paths were not explored",
    )
>>>>>>> origin/agent/formal-methods-20260730-segment-lifecycle

    print(
        f"segment lifecycle model: {len(seen)} states, "
        f"{transitions} transitions, {len(action_names)} action classes; "
        "all invariants hold"
    )


if __name__ == "__main__":
    try:
        main()
    except ModelViolation as error:
        raise SystemExit(f"segment lifecycle model violation: {error}") from error
