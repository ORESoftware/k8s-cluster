#!/usr/bin/env python3
"""Bounded case-admission and verdict model for the USACC simulation/API."""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass, replace

PETITION = 0
SCREENING = 1
ADMISSION = 2
TRIAL = 3
FINAL = 4
TARGET_SIGNATURES = 2
ADMISSION_PANEL = 3
TRIAL_PANEL = 3
ADMISSION_THRESHOLD = 2
CONVICTION_THRESHOLD = 2
MAX_DEPTH = 12


@dataclass(frozen=True, slots=True)
class State:
    stage: int = PETITION
    signatures: int = 0
    admission_for: int = 0
    admission_against: int = 0
    guilty: int = 0
    not_guilty: int = 0
    admitted: bool = False
    convicted: bool = False


def successors(state: State):
    if state.stage == PETITION:
        if state.signatures < TARGET_SIGNATURES:
            count = state.signatures + 1
            stage = SCREENING if count == TARGET_SIGNATURES else PETITION
            yield "signature", replace(state, signatures=count, stage=stage)

    elif state.stage == SCREENING:
        yield "screening-pass", replace(state, stage=ADMISSION)

    elif state.stage == ADMISSION:
        cast = state.admission_for + state.admission_against
        if cast < ADMISSION_PANEL:
            for vote_for in (False, True):
                new_for = state.admission_for + int(vote_for)
                new_against = state.admission_against + int(not vote_for)
                new_cast = new_for + new_against
                if new_cast == ADMISSION_PANEL:
                    admitted = new_for >= ADMISSION_THRESHOLD
                    stage = TRIAL if admitted else FINAL
                else:
                    admitted = False
                    stage = ADMISSION
                yield "admission-vote", replace(
                    state,
                    admission_for=new_for,
                    admission_against=new_against,
                    admitted=admitted,
                    stage=stage,
                )

    elif state.stage == TRIAL:
        cast = state.guilty + state.not_guilty
        if cast < TRIAL_PANEL:
            for guilty_vote in (False, True):
                guilty = state.guilty + int(guilty_vote)
                not_guilty = state.not_guilty + int(not guilty_vote)
                new_cast = guilty + not_guilty
                final = new_cast == TRIAL_PANEL
                yield "trial-vote", replace(
                    state,
                    guilty=guilty,
                    not_guilty=not_guilty,
                    convicted=final and guilty >= CONVICTION_THRESHOLD,
                    stage=FINAL if final else TRIAL,
                )


def assert_invariants(state: State) -> None:
    admission_cast = state.admission_for + state.admission_against
    trial_cast = state.guilty + state.not_guilty
    assert 0 <= state.signatures <= TARGET_SIGNATURES
    assert 0 <= admission_cast <= ADMISSION_PANEL
    assert 0 <= trial_cast <= TRIAL_PANEL

    if state.admitted:
        assert admission_cast == ADMISSION_PANEL
        assert state.admission_for >= ADMISSION_THRESHOLD
        assert state.stage in {TRIAL, FINAL}

    if state.stage == TRIAL:
        assert state.admitted
        assert admission_cast == ADMISSION_PANEL

    if state.convicted:
        assert state.stage == FINAL
        assert state.admitted
        assert trial_cast == TRIAL_PANEL
        assert state.guilty >= CONVICTION_THRESHOLD

    if state.stage == FINAL:
        assert admission_cast == ADMISSION_PANEL
        if state.admitted:
            assert trial_cast == TRIAL_PANEL
        else:
            assert trial_cast == 0
            assert not state.convicted


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
            assert target.stage >= state.stage, "case procedure moved backwards"
            assert_invariants(target)
            if action == "trial-vote":
                assert state.admitted, "trial vote occurred before admission"
            if target not in seen:
                seen.add(target)
                queue.append((target, depth + 1))

    print(
        f"USACC procedure model: {len(seen)} states, "
        f"{transitions} transitions; all invariants hold"
    )


if __name__ == "__main__":
    main()
