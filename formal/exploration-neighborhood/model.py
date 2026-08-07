#!/usr/bin/env python3
"""Deterministic nearby-state and symmetry fixtures for soccer policy evaluation.

This is a finite experiment sentinel, not a model of the production policy. It
constructs replayable neighborhoods that a Rust policy adapter can consume:
small coordinate/velocity perturbations, field mirroring, and equivalent role
permutations. The built-in reference scorer exists only to validate the fixture
semantics and action-remapping rules.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import random
from dataclasses import asdict, dataclass, replace
from pathlib import Path
from typing import Iterable, Sequence

SCHEMA_VERSION = 1
FIELD_LENGTH = 105.0
FIELD_WIDTH = 68.0
MAX_POSITION_DELTA = 0.75
MAX_VELOCITY_DELTA = 0.25
DEFAULT_SEED = 20260801
ACTIONS = ("pass_left", "pass_right", "carry", "shoot", "recycle")
MIRRORED_ACTION = {
    "pass_left": "pass_right",
    "pass_right": "pass_left",
    "carry": "carry",
    "shoot": "shoot",
    "recycle": "recycle",
}


@dataclass(frozen=True, slots=True)
class Vector:
    x: float
    y: float


@dataclass(frozen=True, slots=True)
class Player:
    role: str
    team: str
    position: Vector
    velocity: Vector


@dataclass(frozen=True, slots=True)
class Scenario:
    fixture_id: str
    ball: Vector
    ball_velocity: Vector
    players: tuple[Player, ...]


@dataclass(frozen=True, slots=True)
class RankedAction:
    action: str
    score: float
    rank: int


def base_scenario() -> Scenario:
    return Scenario(
        fixture_id="pressured-progressive-choice-v1",
        ball=Vector(51.0, 34.0),
        ball_velocity=Vector(0.4, 0.0),
        players=(
            Player("carrier", "home", Vector(50.5, 34.0), Vector(1.1, 0.0)),
            Player("left_support", "home", Vector(59.0, 24.0), Vector(1.6, 0.4)),
            Player("right_support", "home", Vector(58.5, 44.0), Vector(1.4, -0.3)),
            Player("cover", "home", Vector(43.0, 34.0), Vector(0.7, 0.0)),
            Player("pressing_defender", "away", Vector(52.2, 34.8), Vector(-0.9, -0.1)),
            Player("left_defender", "away", Vector(61.0, 25.5), Vector(-0.5, 0.1)),
            Player("right_defender", "away", Vector(62.0, 42.0), Vector(-0.5, -0.1)),
        ),
    )


def mirror_vector(vector: Vector) -> Vector:
    return Vector(vector.x, FIELD_WIDTH - vector.y)


def mirror_velocity(vector: Vector) -> Vector:
    return Vector(vector.x, -vector.y)


def mirror_scenario(scenario: Scenario) -> Scenario:
    mirrored_players = []
    for player in scenario.players:
        role = player.role
        if role.startswith("left_"):
            role = "right_" + role.removeprefix("left_")
        elif role.startswith("right_"):
            role = "left_" + role.removeprefix("right_")
        mirrored_players.append(
            replace(
                player,
                role=role,
                position=mirror_vector(player.position),
                velocity=mirror_velocity(player.velocity),
            )
        )
    return replace(
        scenario,
        fixture_id=f"{scenario.fixture_id}:mirror",
        ball=mirror_vector(scenario.ball),
        ball_velocity=mirror_velocity(scenario.ball_velocity),
        players=tuple(mirrored_players),
    )


def equivalent_role_permutation(scenario: Scenario) -> Scenario:
    """Swap equivalent left/right support identities without moving entities."""
    swapped: list[Player] = []
    for player in scenario.players:
        role = player.role
        if role == "left_support":
            role = "right_support"
        elif role == "right_support":
            role = "left_support"
        swapped.append(replace(player, role=role))
    return replace(
        scenario,
        fixture_id=f"{scenario.fixture_id}:role-permutation",
        players=tuple(swapped),
    )


def perturb_scenario(scenario: Scenario, seed: int, sample: int) -> Scenario:
    rng = random.Random((seed << 32) ^ sample)

    def perturb_vector(vector: Vector, limit: float) -> Vector:
        return Vector(
            vector.x + rng.uniform(-limit, limit),
            vector.y + rng.uniform(-limit, limit),
        )

    players = tuple(
        replace(
            player,
            position=perturb_vector(player.position, MAX_POSITION_DELTA),
            velocity=perturb_vector(player.velocity, MAX_VELOCITY_DELTA),
        )
        for player in scenario.players
    )
    return replace(
        scenario,
        fixture_id=f"{scenario.fixture_id}:perturb-{sample:03d}",
        ball=perturb_vector(scenario.ball, MAX_POSITION_DELTA),
        ball_velocity=perturb_vector(scenario.ball_velocity, MAX_VELOCITY_DELTA),
        players=players,
    )


def player(scenario: Scenario, role: str) -> Player:
    return next(item for item in scenario.players if item.role == role)


def distance(left: Vector, right: Vector) -> float:
    return math.hypot(left.x - right.x, left.y - right.y)


def lane_clearance(scenario: Scenario, target: Vector) -> float:
    carrier = player(scenario, "carrier").position
    dx = target.x - carrier.x
    dy = target.y - carrier.y
    length_squared = dx * dx + dy * dy
    if length_squared <= 1.0e-9:
        return 0.0
    minimum = 20.0
    for defender in (entry for entry in scenario.players if entry.team == "away"):
        rel_x = defender.position.x - carrier.x
        rel_y = defender.position.y - carrier.y
        progress = (rel_x * dx + rel_y * dy) / length_squared
        if not 0.0 < progress < 1.0:
            continue
        projection = Vector(carrier.x + progress * dx, carrier.y + progress * dy)
        minimum = min(minimum, distance(defender.position, projection))
    return minimum


def reference_scores(scenario: Scenario) -> dict[str, float]:
    carrier = player(scenario, "carrier")
    left = player(scenario, "left_support")
    right = player(scenario, "right_support")
    pressing = player(scenario, "pressing_defender")
    pressure = max(0.0, 5.0 - distance(carrier.position, pressing.position))

    def pass_score(target: Player) -> float:
        progress = target.position.x - carrier.position.x
        clearance = lane_clearance(scenario, target.position)
        return 0.70 * progress + 1.15 * clearance - 0.12 * distance(carrier.position, target.position)

    goal = Vector(FIELD_LENGTH, FIELD_WIDTH / 2.0)
    goal_distance = distance(carrier.position, goal)
    return {
        "pass_left": pass_score(left),
        "pass_right": pass_score(right),
        "carry": 3.0 - 0.8 * pressure,
        "shoot": 8.0 - 0.16 * goal_distance - 0.4 * pressure,
        "recycle": 1.5 + 0.35 * pressure,
    }


def rank_actions(scores: dict[str, float]) -> tuple[RankedAction, ...]:
    ordered = sorted(scores.items(), key=lambda item: (-item[1], item[0]))
    return tuple(RankedAction(action, score, rank) for rank, (action, score) in enumerate(ordered))


def remap_mirrored_ranking(ranking: Sequence[RankedAction]) -> tuple[str, ...]:
    return tuple(MIRRORED_ACTION[item.action] for item in ranking)


def scenario_payload(scenario: Scenario) -> dict[str, object]:
    return asdict(scenario)


def fixture_digest(scenarios: Iterable[Scenario]) -> str:
    payload = [scenario_payload(item) for item in scenarios]
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def assert_scenario_valid(scenario: Scenario) -> None:
    assert scenario.players, "scenario must contain players"
    roles = [entry.role for entry in scenario.players]
    assert len(roles) == len(set(roles)), f"duplicate roles in {scenario.fixture_id}"
    assert {entry.team for entry in scenario.players} <= {"home", "away"}
    for vector in [scenario.ball, *(entry.position for entry in scenario.players)]:
        assert -2.0 <= vector.x <= FIELD_LENGTH + 2.0
        assert -2.0 <= vector.y <= FIELD_WIDTH + 2.0
    for vector in [scenario.ball_velocity, *(entry.velocity for entry in scenario.players)]:
        assert math.isfinite(vector.x) and math.isfinite(vector.y)


def verify(seed: int = DEFAULT_SEED, samples: int = 32) -> dict[str, object]:
    assert samples > 0
    base = base_scenario()
    mirror = mirror_scenario(base)
    round_trip = mirror_scenario(mirror)
    permutation = equivalent_role_permutation(base)
    perturbations = tuple(perturb_scenario(base, seed, index) for index in range(samples))
    scenarios = (base, mirror, permutation, *perturbations)
    for scenario in scenarios:
        assert_scenario_valid(scenario)

    assert round_trip.ball == base.ball
    assert round_trip.ball_velocity == base.ball_velocity
    assert tuple((entry.role, entry.position, entry.velocity) for entry in round_trip.players) == tuple(
        (entry.role, entry.position, entry.velocity) for entry in base.players
    )

    base_ranking = rank_actions(reference_scores(base))
    mirrored_ranking = rank_actions(reference_scores(mirror))
    assert remap_mirrored_ranking(base_ranking) == tuple(item.action for item in mirrored_ranking), (
        base_ranking,
        mirrored_ranking,
    )

    base_role_positions = sorted((entry.team, entry.position.x, entry.position.y) for entry in base.players)
    permuted_role_positions = sorted(
        (entry.team, entry.position.x, entry.position.y) for entry in permutation.players
    )
    assert base_role_positions == permuted_role_positions

    replayed = tuple(perturb_scenario(base, seed, index) for index in range(samples))
    assert perturbations == replayed, "same seed/sample must reproduce identical fixtures"
    for original, perturbed in zip((base,) * samples, perturbations, strict=True):
        assert distance(original.ball, perturbed.ball) <= math.sqrt(2.0) * MAX_POSITION_DELTA + 1e-12
        for before, after in zip(original.players, perturbed.players, strict=True):
            assert distance(before.position, after.position) <= math.sqrt(2.0) * MAX_POSITION_DELTA + 1e-12
            assert distance(before.velocity, after.velocity) <= math.sqrt(2.0) * MAX_VELOCITY_DELTA + 1e-12

    top_action_counts: dict[str, int] = {action: 0 for action in ACTIONS}
    for scenario in perturbations:
        top_action_counts[rank_actions(reference_scores(scenario))[0].action] += 1

    return {
        "schemaVersion": SCHEMA_VERSION,
        "status": "ok",
        "seed": seed,
        "samples": samples,
        "scenarioCount": len(scenarios),
        "fixtureDigest": fixture_digest(scenarios),
        "baseRanking": [asdict(item) for item in base_ranking],
        "mirroredRanking": [asdict(item) for item in mirrored_ranking],
        "topActionCounts": top_action_counts,
        "checks": {
            "mirrorInvolution": True,
            "mirrorActionEquivariance": True,
            "rolePermutationPreservesGeometry": True,
            "boundedPerturbations": True,
            "seedReplay": True,
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED)
    parser.add_argument("--samples", type=int, default=32)
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()
    result = verify(args.seed, args.samples)
    output = json.dumps(result, sort_keys=True, indent=2)
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(output + "\n", encoding="utf-8")
    print(output)


if __name__ == "__main__":
    main()
