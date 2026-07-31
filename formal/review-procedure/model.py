#!/usr/bin/env python3
"""Bounded match-rule model for deterministic soccer-engine transitions."""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass, replace

NONE = -1
TEAMS = (0, 1)
KICKOFF = 0
IN_PLAY = 1
STOPPAGE = 2
FINISHED = 3
MAX_TICKS = 4
MAX_GOALS = 2
MAX_DEPTH = 10


@dataclass(frozen=True, slots=True)
class State:
    tick: int = 0
    phase: int = KICKOFF
    possession: int = NONE
    score_a: int = 0
    score_b: int = 0
    restart_team: int = 0


def score(state: State, team: int) -> int:
    return state.score_a if team == 0 else state.score_b


def successors(state: State):
    if state.phase == FINISHED:
        return

    if state.tick < MAX_TICKS:
        next_tick = state.tick + 1
        if next_tick == MAX_TICKS:
            yield "clock-finish", replace(
                state, tick=next_tick, phase=FINISHED, possession=NONE
            )
        else:
            yield "clock", replace(state, tick=next_tick)

    if state.phase == KICKOFF:
        yield "kickoff", replace(
            state, phase=IN_PLAY, possession=state.restart_team
        )

    elif state.phase == IN_PLAY:
        team = state.possession
        other = 1 - team
        yield "pass-complete", state
        yield "turnover", replace(state, possession=other)
        yield "shot-miss", replace(state, possession=other)
        yield "ball-out", replace(
            state, phase=STOPPAGE, possession=NONE, restart_team=other
        )
        if score(state, team) < MAX_GOALS:
            if team == 0:
                target = replace(
                    state,
                    phase=KICKOFF,
                    possession=NONE,
                    score_a=state.score_a + 1,
                    restart_team=other,
                )
            else:
                target = replace(
                    state,
                    phase=KICKOFF,
                    possession=NONE,
                    score_b=state.score_b + 1,
                    restart_team=other,
                )
            yield f"goal({team})", target

    elif state.phase == STOPPAGE:
        yield "restart", replace(
            state, phase=IN_PLAY, possession=state.restart_team
        )


def assert_invariants(state: State) -> None:
    assert 0 <= state.tick <= MAX_TICKS
    assert 0 <= state.score_a <= MAX_GOALS
    assert 0 <= state.score_b <= MAX_GOALS
    assert state.restart_team in TEAMS

    if state.phase == IN_PLAY:
        assert state.possession in TEAMS
    else:
        assert state.possession == NONE

    if state.phase == FINISHED:
        assert state.tick == MAX_TICKS


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
            assert target.tick >= state.tick, "match time moved backwards"
            if action.startswith("goal"):
                delta_a = target.score_a - state.score_a
                delta_b = target.score_b - state.score_b
                assert (delta_a, delta_b) in {(1, 0), (0, 1)}
                scoring_team = 0 if delta_a else 1
                assert target.restart_team == 1 - scoring_team
                assert target.phase == KICKOFF
            else:
                assert target.score_a == state.score_a
                assert target.score_b == state.score_b
            if state.phase == FINISHED:
                raise AssertionError("finished state emitted a transition")
            if target not in seen:
                seen.add(target)
                queue.append((target, depth + 1))

    print(
        f"soccer transition model: {len(seen)} states, "
        f"{transitions} transitions; all invariants hold"
    )


if __name__ == "__main__":
    main()
