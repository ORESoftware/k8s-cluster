#!/usr/bin/env python3
"""Bounded segment upload, mirror, pin, and retention-deletion model."""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass, replace

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
MAX_DEPTH = 12


@dataclass(frozen=True, slots=True)
class State:
    status: int = NEW
    primary_exists: bool = False
    verified: bool = False
    pinned: bool = False
    expired: bool = False
    mirror: int = MIRROR_NONE


def successors(state: State):
    if state.status == NEW:
        yield "presign", replace(state, status=PRESIGNED)

    if state.status == PRESIGNED and not state.primary_exists:
        yield "client-put", replace(state, primary_exists=True)
    if state.status == PRESIGNED and state.primary_exists:
        yield "verify-head-and-complete", replace(
            state, status=UPLOADED, verified=True
        )

    if state.status == UPLOADED:
        if not state.pinned:
            yield "pin", replace(state, pinned=True)
        if state.mirror == MIRROR_NONE:
            yield "mirror-claim", replace(state, mirror=MIRROR_COPYING)
        if state.mirror == MIRROR_COPYING:
            yield "mirror-complete", replace(state, mirror=MIRRORED)
        if not state.expired:
            yield "retention-expire", replace(state, expired=True)
        if state.expired and not state.pinned:
            yield "retention-delete-claim", replace(state, status=DELETE_CLAIMED)

    if state.status == DELETE_CLAIMED:
        if state.primary_exists:
            yield "delete-primary", replace(state, primary_exists=False)
        if state.mirror == MIRRORED:
            yield "mirror-delete-claim", replace(
                state, mirror=MIRROR_DELETE_CLAIMED
            )
        if state.mirror == MIRROR_DELETE_CLAIMED:
            yield "delete-mirror", replace(state, mirror=MIRROR_DELETED)
        if not state.primary_exists and state.mirror in {MIRROR_NONE, MIRROR_DELETED}:
            yield "finalize-delete", replace(state, status=DELETED)


def assert_invariants(state: State) -> None:
    if state.status == UPLOADED:
        assert state.primary_exists
        assert state.verified, "unverified object became authoritative"
    if state.status in {DELETE_CLAIMED, DELETED}:
        assert state.verified
        assert state.expired
        assert not state.pinned, "retention deleted a pinned segment"
    if state.status == DELETED:
        assert not state.primary_exists
        assert state.mirror in {MIRROR_NONE, MIRROR_DELETED}
    if state.pinned:
        assert state.status == UPLOADED
    if state.mirror in {MIRROR_COPYING, MIRRORED, MIRROR_DELETE_CLAIMED}:
        assert state.verified
        assert state.status in {UPLOADED, DELETE_CLAIMED}


def main() -> None:
    initial = State()
    queue = deque([(initial, 0)])
    seen = {initial}
    transitions = 0

    while queue:
        state, depth = queue.popleft()
        assert_invariants(state)
        if depth == MAX_DEPTH:
            continue
        for action, target in successors(state) or ():
            transitions += 1
            assert_invariants(target)
            if action == "verify-head-and-complete":
                assert state.primary_exists
            if action == "retention-delete-claim":
                assert state.expired and not state.pinned
            if action == "finalize-delete":
                assert not state.primary_exists
                assert state.mirror in {MIRROR_NONE, MIRROR_DELETED}
            if target not in seen:
                seen.add(target)
                queue.append((target, depth + 1))

    print(
        f"segment lifecycle model: {len(seen)} states, "
        f"{transitions} transitions; all invariants hold"
    )


if __name__ == "__main__":
    main()
