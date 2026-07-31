#!/usr/bin/env python3
"""Bounded refresh-token rotation/replay/revocation model."""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass, replace

NEW = 0
ACTIVE = 1
REVOKED = 2
NONE = -1
MAX_TOKEN = 4
MAX_DEPTH = 8


@dataclass(frozen=True, slots=True)
class State:
    status: int = NEW
    current: int = NONE
    next_token: int = 0
    consumed_mask: int = 0


def consumed(state: State, token: int) -> bool:
    return token >= 0 and bool(state.consumed_mask & (1 << token))


def refresh(state: State, presented: int) -> tuple[State, bool]:
    if state.status != ACTIVE or presented != state.current:
        return state, False
    if state.next_token > MAX_TOKEN:
        return state, False
    new_mask = state.consumed_mask | (1 << state.current)
    target = replace(
        state,
        current=state.next_token,
        next_token=state.next_token + 1,
        consumed_mask=new_mask,
    )
    return target, True


def successors(state: State):
    if state.status == NEW:
        yield "login", State(status=ACTIVE, current=0, next_token=1)
    elif state.status == ACTIVE:
        for candidate in range(MAX_TOKEN + 1):
            target, accepted = refresh(state, candidate)
            if accepted:
                yield f"refresh({candidate})", target
        yield "logout", replace(state, status=REVOKED, current=NONE)


def assert_invariants(state: State) -> None:
    assert 0 <= state.next_token <= MAX_TOKEN + 1
    if state.status == ACTIVE:
        assert 0 <= state.current <= MAX_TOKEN
        assert state.current < state.next_token
        assert not consumed(state, state.current), "current refresh token was already consumed"
    else:
        assert state.current == NONE

    if state.status == ACTIVE:
        for token in range(state.next_token):
            if token != state.current:
                assert consumed(state, token), "an older issued token was not consumed"


def main() -> None:
    initial = State()
    queue = deque([(initial, 0)])
    seen = {initial}
    transitions = 0

    while queue:
        state, depth = queue.popleft()
        assert_invariants(state)

        # Every rejected refresh is side-effect free; every consumed or revoked
        # token is rejected.
        for candidate in range(MAX_TOKEN + 1):
            target, accepted = refresh(state, candidate)
            expected = (
                state.status == ACTIVE
                and candidate == state.current
                and state.next_token <= MAX_TOKEN
            )
            assert accepted == expected
            if not accepted:
                assert target == state
            if consumed(state, candidate) or state.status == REVOKED:
                assert not accepted

        if depth == MAX_DEPTH:
            continue
        for action, target in successors(state) or ():
            transitions += 1
            assert_invariants(target)
            if action.startswith("refresh"):
                assert consumed(target, state.current)
                assert target.current != state.current
            if target not in seen:
                seen.add(target)
                queue.append((target, depth + 1))

    print(
        f"refresh-session model: {len(seen)} states, "
        f"{transitions} transitions; all invariants hold"
    )


if __name__ == "__main__":
    main()
