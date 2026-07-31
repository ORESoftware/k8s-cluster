#!/usr/bin/env python3
"""Validate the repository's machine-readable formal review obligations."""

from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REGISTER = Path(__file__).with_name("obligations.json")
ID_RE = re.compile(r"^[A-Z][A-Z0-9_-]{2,63}$")
KINDS = {"safety", "liveness", "refinement"}


def require_string(value: object, label: str) -> str:
    assert isinstance(value, str) and value.strip(), f"{label} must be a non-empty string"
    return value.strip()


def require_string_list(value: object, label: str, *, allow_empty: bool = False) -> list[str]:
    assert isinstance(value, list), f"{label} must be a list"
    if not allow_empty:
        assert value, f"{label} must not be empty"
    result = [require_string(item, f"{label}[]") for item in value]
    assert len(result) == len(set(result)), f"{label} contains duplicates"
    return result


def main() -> None:
    document = json.loads(REGISTER.read_text(encoding="utf-8"))
    assert document.get("schema_version") == 1, "unsupported schema_version"
    repository = require_string(document.get("repository"), "repository")
    procedure = require_string(document.get("procedure"), "procedure")
    assert (ROOT / procedure).is_file(), f"procedure file is missing: {procedure}"

    obligations = document.get("obligations")
    assert isinstance(obligations, list) and obligations, "obligations must be non-empty"
    identifiers: set[str] = set()
    safety_count = 0

    for index, item in enumerate(obligations):
        assert isinstance(item, dict), f"obligations[{index}] must be an object"
        obligation_id = require_string(item.get("id"), f"obligations[{index}].id")
        assert ID_RE.fullmatch(obligation_id), f"invalid obligation id: {obligation_id}"
        assert obligation_id not in identifiers, f"duplicate obligation id: {obligation_id}"
        identifiers.add(obligation_id)

        kind = require_string(item.get("kind"), f"{obligation_id}.kind")
        assert kind in KINDS, f"{obligation_id}.kind must be one of {sorted(KINDS)}"
        safety_count += kind == "safety"

        statement = require_string(item.get("statement"), f"{obligation_id}.statement")
        assert len(statement) >= 20, f"{obligation_id}.statement is too vague"
        require_string_list(item.get("trigger_paths"), f"{obligation_id}.trigger_paths")
        require_string_list(item.get("evidence"), f"{obligation_id}.evidence")
        require_string_list(item.get("commands"), f"{obligation_id}.commands")
        require_string_list(item.get("assumptions"), f"{obligation_id}.assumptions", allow_empty=True)

        bounded = item.get("bounded")
        assert isinstance(bounded, bool), f"{obligation_id}.bounded must be boolean"
        model = item.get("model")
        if model is not None:
            model_path = require_string(model, f"{obligation_id}.model")
            if model_path.startswith("formal/review-procedure/"):
                assert (ROOT / model_path).is_file(), f"model file is missing: {model_path}"

    assert safety_count > 0, "at least one safety obligation is required"
    print(f"formal obligation register: {repository}: {len(obligations)} obligations validated")


if __name__ == "__main__":
    main()
