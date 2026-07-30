#!/usr/bin/env python3
"""Finite lifecycle specification for deterministic Akrion soccer match traces."""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from collections import deque
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Iterable

MAX_TICKS = 4
SCORE_CAP = 2
ACTIONS = (
    "start", "tick", "home_possession", "away_possession", "loose_ball",
    "home_goal", "away_goal", "restart", "finish",
)


@dataclass(frozen=True, slots=True)
class State:
    phase: str = "pre_kickoff"
    tick: int = 0
    home_score: int = 0
    away_score: int = 0
    possession: str = "none"
    restart_side: str = "home"


def valid(state: State) -> int:
    assert state.phase in {"pre_kickoff", "in_play", "stoppage", "finished"}
    assert 0 <= state.tick <= MAX_TICKS
    assert 0 <= state.home_score <= SCORE_CAP
    assert 0 <= state.away_score <= SCORE_CAP
    assert state.possession in {"none", "home", "away"}
    assert state.restart_side in {"home", "away"}
    if state.phase in {"pre_kickoff", "stoppage", "finished"}:
        assert state.possession == "none"
    return 7


def transition(state: State, action: str) -> State:
    if action not in ACTIONS:
        raise ValueError(f"unsupported action: {action}")
    if state.phase == "finished":
        return state
    if action == "finish":
        return State("finished", state.tick, state.home_score, state.away_score, "none", state.restart_side)
    if action == "start" and state.phase == "pre_kickoff":
        return State("in_play", state.tick, state.home_score, state.away_score, state.restart_side, state.restart_side)
    if action == "restart" and state.phase == "stoppage":
        return State("in_play", state.tick, state.home_score, state.away_score, state.restart_side, state.restart_side)
    if state.phase != "in_play":
        return state
    if action == "tick":
        next_tick = min(MAX_TICKS, state.tick + 1)
        if next_tick >= MAX_TICKS:
            return State("finished", next_tick, state.home_score, state.away_score, "none", state.restart_side)
        return State("in_play", next_tick, state.home_score, state.away_score, state.possession, state.restart_side)
    if action == "home_possession":
        return State("in_play", state.tick, state.home_score, state.away_score, "home", state.restart_side)
    if action == "away_possession":
        return State("in_play", state.tick, state.home_score, state.away_score, "away", state.restart_side)
    if action == "loose_ball":
        return State("in_play", state.tick, state.home_score, state.away_score, "none", state.restart_side)
    if action == "home_goal" and state.home_score < SCORE_CAP:
        return State("stoppage", state.tick, state.home_score + 1, state.away_score, "none", "away")
    if action == "away_goal" and state.away_score < SCORE_CAP:
        return State("stoppage", state.tick, state.home_score, state.away_score + 1, "none", "home")
    return state


def assert_transition(before: State, action: str, after: State) -> int:
    assert after.tick >= before.tick
    assert after.home_score >= before.home_score
    assert after.away_score >= before.away_score
    assert after.home_score - before.home_score <= 1
    assert after.away_score - before.away_score <= 1
    if before.phase == "finished":
        assert after == before
    if after.home_score > before.home_score:
        assert action == "home_goal"
        assert after.phase == "stoppage" and after.possession == "none"
        assert after.restart_side == "away"
    if after.away_score > before.away_score:
        assert action == "away_goal"
        assert after.phase == "stoppage" and after.possession == "none"
        assert after.restart_side == "home"
    return 11


def load_manifest() -> dict[str, Any]:
    with Path(__file__).with_name("fm.toml").open("rb") as handle:
        manifest = tomllib.load(handle)
    assert manifest["schema_version"] == 1
    assert manifest["adapter_protocol"] == "json-stdin/v1"
    assert manifest["limits"]["max_ticks"] == MAX_TICKS
    assert manifest["limits"]["score_cap"] == SCORE_CAP
    assert {item["id"] for item in manifest["invariants"]} == {
        "monotonic-clock-and-score", "goal-enters-stoppage",
        "restart-opponent-kickoff", "single-possession",
        "finished-absorbing", "deterministic-replay",
    }
    return manifest


def replay_events(events: list[str]) -> tuple[State, list[dict[str, Any]]]:
    state = State()
    trace = [asdict(state)]
    for action in events:
        next_state = transition(state, action)
        valid(next_state)
        assert_transition(state, action, next_state)
        state = next_state
        trace.append(asdict(state))
    return state, trace


def verify() -> dict[str, Any]:
    manifest = load_manifest()
    depth = int(manifest["limits"]["transition_depth"])
    initial = State()
    seen = {initial}
    queue = deque([(initial, 0)])
    checks = valid(initial)
    transitions = 0
    while queue:
        state, level = queue.popleft()
        if level >= depth:
            continue
        for action in ACTIONS:
            after = transition(state, action)
            checks += valid(after)
            checks += assert_transition(state, action, after)
            transitions += 1
            if after not in seen:
                seen.add(after)
                queue.append((after, level + 1))

    witnesses = (
        ["start", "home_possession", "home_goal", "restart", "tick"],
        ["start", "away_goal", "restart", "away_possession", "finish"],
        ["start", "tick", "tick", "tick", "tick", "home_goal"],
    )
    for events in witnesses:
        first = replay_events(list(events))
        second = replay_events(list(events))
        assert first == second
        checks += len(first[1]) + 1

    return {"status": "ok", "model": manifest["id"], "claim": manifest["claim"], "reachable_states": len(seen), "transitions": transitions, "checks": checks}


def emit(records: Iterable[dict[str, Any]]) -> None:
    for record in records:
        print(json.dumps(record, sort_keys=True, separators=(",", ":")))


def replay() -> None:
    load_manifest()
    outputs = []
    for line_number, raw in enumerate(sys.stdin, start=1):
        raw = raw.strip()
        if not raw:
            continue
        request = json.loads(raw)
        if request.get("op") != "replay":
            raise ValueError("supported op is replay")
        final, trace = replay_events([str(item) for item in request.get("events", [])])
        outputs.append({"schema_version": 1, "line": line_number, "final": asdict(final), "trace": trace})
    emit(outputs)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json-stdin", action="store_true")
    args = parser.parse_args()
    if args.json_stdin:
        replay()
    else:
        print(json.dumps(verify(), sort_keys=True))


if __name__ == "__main__":
    main()
