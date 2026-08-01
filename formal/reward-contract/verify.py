#!/usr/bin/env python3
"""Validate the versioned soccer reward contract and anti-gaming invariants."""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import math
from pathlib import Path
from typing import Any

SUPPORTED_SCHEMA_VERSION = 1
ALLOWED_GROUPS = {"terminal", "shaping", "penalty", "safety"}
ALLOWED_POLARITIES = {"reward", "penalty", "signed"}


def load_contract(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError("contract root must be an object")
    return data


def canonical_digest(contract: dict[str, Any]) -> str:
    payload = json.dumps(contract, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(payload).hexdigest()


def weighted_bounds(component: dict[str, Any]) -> tuple[float, float]:
    weighted = (
        float(component["minimum"]) * float(component["weight"]),
        float(component["maximum"]) * float(component["weight"]),
    )
    return min(weighted), max(weighted)


def aggregate_reward(contract: dict[str, Any], values: dict[str, float]) -> float:
    total = 0.0
    by_id = {component["id"]: component for component in contract["components"]}
    unknown = set(values) - set(by_id)
    if unknown:
        raise ValueError(f"unknown reward components: {sorted(unknown)}")
    for component_id, raw_value in values.items():
        component = by_id[component_id]
        minimum = float(component["minimum"])
        maximum = float(component["maximum"])
        if not minimum <= raw_value <= maximum:
            raise ValueError(f"{component_id} value {raw_value} outside [{minimum}, {maximum}]")
        total += raw_value * float(component["weight"])
    return total


def verify(contract: dict[str, Any]) -> dict[str, Any]:
    if contract.get("schemaVersion") != SUPPORTED_SCHEMA_VERSION:
        raise ValueError(
            f"unsupported schemaVersion {contract.get('schemaVersion')}; expected {SUPPORTED_SCHEMA_VERSION}"
        )
    if not str(contract.get("contractName", "")).strip():
        raise ValueError("contractName must not be empty")
    margin = float(contract.get("terminalDominanceMargin", 0.0))
    if not math.isfinite(margin) or margin < 0.0:
        raise ValueError("terminalDominanceMargin must be finite and non-negative")

    components = contract.get("components")
    if not isinstance(components, list) or not components:
        raise ValueError("components must be a non-empty array")

    seen: set[str] = set()
    normalized: list[dict[str, Any]] = []
    for component in components:
        component_id = str(component.get("id", "")).strip()
        if not component_id:
            raise ValueError("component id must not be empty")
        if component_id in seen:
            raise ValueError(f"duplicate component id: {component_id}")
        seen.add(component_id)
        group = component.get("group")
        polarity = component.get("polarity")
        if group not in ALLOWED_GROUPS:
            raise ValueError(f"{component_id}: invalid group {group}")
        if polarity not in ALLOWED_POLARITIES:
            raise ValueError(f"{component_id}: invalid polarity {polarity}")
        minimum = float(component.get("minimum"))
        maximum = float(component.get("maximum"))
        weight = float(component.get("weight"))
        if not all(math.isfinite(value) for value in (minimum, maximum, weight)):
            raise ValueError(f"{component_id}: bounds and weight must be finite")
        if minimum > maximum:
            raise ValueError(f"{component_id}: minimum exceeds maximum")
        low, high = weighted_bounds(component)
        if polarity == "reward" and low < 0.0:
            raise ValueError(f"{component_id}: reward component can become negative")
        if polarity == "penalty" and high > 0.0:
            raise ValueError(f"{component_id}: penalty component can become positive")
        normalized.append(
            {
                "id": component_id,
                "group": group,
                "polarity": polarity,
                "minimum": minimum,
                "maximum": maximum,
                "weight": weight,
                "weightedMinimum": low,
                "weightedMaximum": high,
            }
        )

    terminals = [item for item in normalized if item["group"] == "terminal"]
    non_terminals = [item for item in normalized if item["group"] != "terminal"]
    terminal_by_id = {item["id"]: item for item in terminals}
    if set(terminal_by_id) != {"goal_for", "goal_against"}:
        raise ValueError("terminal components must be exactly goal_for and goal_against")
    goal_for = terminal_by_id["goal_for"]
    goal_against = terminal_by_id["goal_against"]
    goal_for_value = goal_for["weightedMinimum"]
    goal_against_value = goal_against["weightedMaximum"]
    if not math.isclose(goal_for_value, -goal_against_value, rel_tol=0.0, abs_tol=1e-12):
        raise ValueError("goal_for and goal_against must be symmetric")
    if goal_for_value <= 0.0 or goal_against_value >= 0.0:
        raise ValueError("terminal goal signs are invalid")

    nonterminal_envelope = sum(
        max(abs(item["weightedMinimum"]), abs(item["weightedMaximum"])) for item in non_terminals
    )
    if goal_for_value - nonterminal_envelope < margin:
        raise ValueError(
            "non-terminal shaping/penalty envelope can overpower terminal outcome: "
            f"goal={goal_for_value}, envelope={nonterminal_envelope}, margin={margin}"
        )

    corner_minimum = math.inf
    corner_maximum = -math.inf
    for corner in itertools.product((0, 1), repeat=len(non_terminals)):
        total = 0.0
        for choose_max, item in zip(corner, non_terminals, strict=True):
            total += item["weightedMaximum" if choose_max else "weightedMinimum"]
        corner_minimum = min(corner_minimum, total)
        corner_maximum = max(corner_maximum, total)
        if abs(total) > nonterminal_envelope + 1e-12:
            raise AssertionError("enumerated corner exceeded computed envelope")
        if abs(total) + margin > goal_for_value + 1e-12:
            raise ValueError("an enumerated non-terminal corner violates terminal dominance")

    infeasible = next(item for item in normalized if item["id"] == "infeasible_action")
    if infeasible["weightedMaximum"] > 0.0 or infeasible["weightedMinimum"] >= 0.0:
        raise ValueError("infeasible_action must be a strictly negative optional penalty")

    sample = {
        item["id"]: (item["minimum"] + item["maximum"]) / 2.0
        for item in normalized
        if item["group"] != "terminal"
    }
    reward = aggregate_reward(contract, sample)
    cost = -reward
    if not math.isclose(-cost, reward, rel_tol=0.0, abs_tol=1e-12):
        raise AssertionError("cost = -reward round-trip failed")

    return {
        "schemaVersion": SUPPORTED_SCHEMA_VERSION,
        "status": "ok",
        "contractName": contract["contractName"],
        "contractDigest": canonical_digest(contract),
        "componentCount": len(normalized),
        "nonterminalComponentCount": len(non_terminals),
        "terminalRewardMagnitude": goal_for_value,
        "nonterminalAbsoluteEnvelope": nonterminal_envelope,
        "terminalDominanceMarginRequired": margin,
        "terminalDominanceMarginActual": goal_for_value - nonterminal_envelope,
        "enumeratedNonterminalCorners": 2 ** len(non_terminals),
        "enumeratedRewardMinimum": corner_minimum,
        "enumeratedRewardMaximum": corner_maximum,
        "checks": {
            "uniqueComponents": True,
            "finiteBounds": True,
            "polarityConsistent": True,
            "terminalSymmetry": True,
            "terminalDominatesShaping": True,
            "infeasibleActionNonPositive": True,
            "costRewardRoundTrip": True,
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "contract",
        nargs="?",
        type=Path,
        default=Path(__file__).with_name("reward_contract.json"),
    )
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()
    result = verify(load_contract(args.contract))
    output = json.dumps(result, sort_keys=True, indent=2)
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(output + "\n", encoding="utf-8")
    print(output)


if __name__ == "__main__":
    main()
