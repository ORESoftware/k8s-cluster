#!/usr/bin/env python3
"""Validate that protected repository-publication intents have one active carrier.

The validator is deliberately credential-free and works only from public pull-request
metadata. It classifies the two canonical publication intents, validates the safety
shape of their execution carriers, and rejects duplicate open carriers for the same
intent before any owner authorization or repository mutation can start.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

SCHEMA_VERSION = 1
EXPECTED_AUTHOR = "ORESoftware"
EXPECTED_BASE = "main"
META_AGENT_TARGET = "meta-agents-demo/meta-agent-control-plane.rs"


@dataclass(frozen=True)
class Carrier:
    number: int
    intent: str
    title: str
    draft: bool
    author: str
    base_ref: str
    base_repo: str
    head_ref: str
    head_repo: str
    state: str


def _nested(value: dict[str, Any], *path: str, default: Any = "") -> Any:
    current: Any = value
    for part in path:
        if not isinstance(current, dict):
            return default
        current = current.get(part, default)
    return current


def is_execution_carrier_title(title: str) -> bool:
    normalized = title.strip().upper()
    return normalized.startswith("DO NOT MERGE:") or normalized.startswith(
        "[DO NOT MERGE]"
    ) or normalized.startswith("TRIGGER:")


def classify_intent(pull: dict[str, Any]) -> str | None:
    title = str(pull.get("title") or "")
    if not is_execution_carrier_title(title):
        return None

    body = str(pull.get("body") or "")
    head_ref = str(_nested(pull, "head", "ref"))
    text = "\n".join((title, body, head_ref)).lower()

    if (
        META_AGENT_TARGET in text
        or "meta agents repository" in text
        or "meta-agent-control-plane" in text
        or ("meta-agent" in head_ref.lower() and "repository" in text)
    ):
        return "meta-agent-control-plane"

    if (
        "34-repository organization fleet" in text
        or "critical organization fleet" in text
        or "critical-org-fleet" in text
        or "full critical org fleet" in text
    ):
        return "critical-org-fleet"

    return None


def to_carrier(pull: dict[str, Any], intent: str) -> Carrier:
    return Carrier(
        number=int(pull.get("number") or 0),
        intent=intent,
        title=str(pull.get("title") or ""),
        draft=bool(pull.get("draft")),
        author=str(_nested(pull, "user", "login")),
        base_ref=str(_nested(pull, "base", "ref")),
        base_repo=str(_nested(pull, "base", "repo", "full_name")),
        head_ref=str(_nested(pull, "head", "ref")),
        head_repo=str(_nested(pull, "head", "repo", "full_name")),
        state=str(pull.get("state") or ""),
    )


def carrier_violations(carrier: Carrier) -> list[dict[str, Any]]:
    violations: list[dict[str, Any]] = []

    def add(code: str, message: str) -> None:
        violations.append(
            {
                "code": code,
                "intent": carrier.intent,
                "pull_requests": [carrier.number],
                "message": message,
            }
        )

    if carrier.number <= 0:
        add("invalid-number", "carrier pull request number must be positive")
    if carrier.state != "open":
        add("not-open", "execution carrier must remain open while active")
    if not carrier.draft:
        add("not-draft", "execution carrier must remain draft and must not merge")
    if carrier.author != EXPECTED_AUTHOR:
        add("unexpected-author", f"carrier author must be {EXPECTED_AUTHOR}")
    if carrier.base_ref != EXPECTED_BASE:
        add("unexpected-base", f"carrier base must be {EXPECTED_BASE}")
    if not carrier.base_repo or carrier.head_repo != carrier.base_repo:
        add("cross-repository-head", "carrier head must live in the target repository")
    if not carrier.head_ref.startswith(("agent/", "ops/")):
        add("unexpected-head-prefix", "carrier head must use agent/ or ops/")
    return violations


def audit(
    pulls: Iterable[dict[str, Any]], current_pr: int | None = None
) -> dict[str, Any]:
    carriers: list[Carrier] = []
    for pull in pulls:
        intent = classify_intent(pull)
        if intent is not None:
            carriers.append(to_carrier(pull, intent))

    by_intent: dict[str, list[Carrier]] = {}
    for carrier in carriers:
        by_intent.setdefault(carrier.intent, []).append(carrier)
    for values in by_intent.values():
        values.sort(key=lambda carrier: carrier.number)

    current_intent: str | None = None
    if current_pr is not None:
        match = next((carrier for carrier in carriers if carrier.number == current_pr), None)
        if match is None:
            return {
                "schema_version": SCHEMA_VERSION,
                "current_pr": current_pr,
                "current_intent": None,
                "carrier_count": len(carriers),
                "intents": {
                    intent: [carrier.number for carrier in values]
                    for intent, values in sorted(by_intent.items())
                },
                "violations": [],
                "status": "ignored-non-carrier",
            }
        current_intent = match.intent

    violations: list[dict[str, Any]] = []
    for carrier in carriers:
        if current_pr is None or carrier.number == current_pr:
            violations.extend(carrier_violations(carrier))

    for intent, values in sorted(by_intent.items()):
        if current_intent is not None and intent != current_intent:
            continue
        if len(values) > 1:
            numbers = [carrier.number for carrier in values]
            violations.append(
                {
                    "code": "duplicate-active-intent",
                    "intent": intent,
                    "pull_requests": numbers,
                    "message": (
                        f"publication intent {intent} has {len(numbers)} active carriers; "
                        "close or supersede all but one before authorization"
                    ),
                }
            )

    return {
        "schema_version": SCHEMA_VERSION,
        "current_pr": current_pr,
        "current_intent": current_intent,
        "carrier_count": len(carriers),
        "intents": {
            intent: [carrier.number for carrier in values]
            for intent, values in sorted(by_intent.items())
        },
        "violations": violations,
        "status": "rejected" if violations else "accepted",
    }


def load_pulls(path: Path | None) -> list[dict[str, Any]]:
    text = path.read_text(encoding="utf-8") if path else sys.stdin.read()
    stripped = text.strip()
    if not stripped:
        return []
    try:
        value = json.loads(stripped)
    except json.JSONDecodeError:
        pulls = [json.loads(line) for line in stripped.splitlines() if line.strip()]
    else:
        if not isinstance(value, list):
            raise SystemExit("input must be a JSON array or JSON Lines stream")
        pulls = value
    if not all(isinstance(pull, dict) for pull in pulls):
        raise SystemExit("every pull request record must be a JSON object")
    return pulls


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path)
    parser.add_argument("--current-pr", type=int)
    parser.add_argument("--report", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    report = audit(load_pulls(args.input), current_pr=args.current_pr)
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.report:
        args.report.write_text(encoded, encoding="utf-8")
    sys.stdout.write(encoded)
    return 1 if report["violations"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
