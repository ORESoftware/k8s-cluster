#!/usr/bin/env python3
"""Validate the repository's machine-readable formal review obligations."""

from __future__ import annotations

import json
import re
from pathlib import Path, PurePosixPath
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
REGISTER = Path(__file__).with_name("obligations.json")
ID_RE = re.compile(r"^[A-Z][A-Z0-9_-]{2,63}$")
KINDS = {"safety", "liveness", "refinement"}
TOP_LEVEL_FIELDS = {"schema_version", "repository", "procedure", "obligations"}
OBLIGATION_FIELDS = {
    "id",
    "kind",
    "statement",
    "trigger_paths",
    "model",
    "evidence",
    "commands",
    "assumptions",
    "bounded",
}


class ContractError(ValueError):
    """Raised when the formal obligation register is malformed or incomplete."""


def fail(message: str) -> NoReturn:
    raise ContractError(message)


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def require_string(value: object, label: str) -> str:
    require(isinstance(value, str) and bool(value.strip()), f"{label} must be a non-empty string")
    return value.strip()


def require_string_list(
    value: object,
    label: str,
    *,
    allow_empty: bool = False,
) -> list[str]:
    require(isinstance(value, list), f"{label} must be a list")
    if not allow_empty:
        require(bool(value), f"{label} must not be empty")
    result = [require_string(item, f"{label}[]") for item in value]
    require(len(result) == len(set(result)), f"{label} contains duplicates")
    return result


def require_safe_repo_path(value: object, label: str) -> PurePosixPath:
    raw = require_string(value, label)
    require("\\" not in raw, f"{label} must use POSIX separators")
    path = PurePosixPath(raw)
    require(not path.is_absolute(), f"{label} must be repository-relative")
    require(".." not in path.parts, f"{label} must not traverse outside the repository")
    require(raw not in {".", ""}, f"{label} must name a repository file")
    require(raw == path.as_posix(), f"{label} must use normalized POSIX syntax")
    return path


def require_existing_repo_file(path: PurePosixPath, label: str) -> None:
    candidate = ROOT.joinpath(*path.parts)
    try:
        root = ROOT.resolve(strict=True)
        resolved = candidate.resolve(strict=True)
    except (OSError, RuntimeError) as error:
        fail(f"{label} file is missing or cannot be resolved safely: {path}: {error}")
    try:
        resolved.relative_to(root)
    except ValueError:
        fail(f"{label} resolves outside the repository: {path}")
    require(resolved.is_file(), f"{label} is not a regular file: {path}")


def require_safe_trigger(value: str, label: str) -> None:
    require("\\" not in value, f"{label} must use POSIX separators")
    path = PurePosixPath(value)
    require(not path.is_absolute(), f"{label} must be repository-relative")
    require(".." not in path.parts, f"{label} must not traverse outside the repository")
    require(value == path.as_posix(), f"{label} must use normalized POSIX syntax")


def validate_document(document: object) -> tuple[str, int]:
    require(isinstance(document, dict), "register root must be an object")
    unknown = sorted(set(document) - TOP_LEVEL_FIELDS)
    require(not unknown, f"unknown top-level fields: {unknown}")
    require(document.get("schema_version") == 1, "unsupported schema_version")

    repository = require_string(document.get("repository"), "repository")
    require(
        repository == "sonus-auris/sonus-auris-backend.rs",
        f"repository must be the canonical backend repository, got {repository!r}",
    )

    procedure_path = require_safe_repo_path(document.get("procedure"), "procedure")
    require_existing_repo_file(procedure_path, "procedure")

    obligations = document.get("obligations")
    require(isinstance(obligations, list) and bool(obligations), "obligations must be non-empty")
    identifiers: set[str] = set()
    safety_count = 0

    for index, item in enumerate(obligations):
        require(isinstance(item, dict), f"obligations[{index}] must be an object")
        unknown_fields = sorted(set(item) - OBLIGATION_FIELDS)
        require(
            not unknown_fields,
            f"obligations[{index}] contains unknown fields: {unknown_fields}",
        )

        obligation_id = require_string(item.get("id"), f"obligations[{index}].id")
        require(bool(ID_RE.fullmatch(obligation_id)), f"invalid obligation id: {obligation_id}")
        require(obligation_id not in identifiers, f"duplicate obligation id: {obligation_id}")
        identifiers.add(obligation_id)

        kind = require_string(item.get("kind"), f"{obligation_id}.kind")
        require(kind in KINDS, f"{obligation_id}.kind must be one of {sorted(KINDS)}")
        safety_count += int(kind == "safety")

        statement = require_string(item.get("statement"), f"{obligation_id}.statement")
        require(len(statement) >= 20, f"{obligation_id}.statement is too vague")

        triggers = require_string_list(item.get("trigger_paths"), f"{obligation_id}.trigger_paths")
        for trigger_index, trigger in enumerate(triggers):
            require_safe_trigger(trigger, f"{obligation_id}.trigger_paths[{trigger_index}]")

        require_string_list(item.get("evidence"), f"{obligation_id}.evidence")
        require_string_list(item.get("commands"), f"{obligation_id}.commands")
        require_string_list(
            item.get("assumptions"),
            f"{obligation_id}.assumptions",
            allow_empty=True,
        )

        bounded = item.get("bounded")
        require(isinstance(bounded, bool), f"{obligation_id}.bounded must be boolean")

        model = item.get("model")
        if model is not None:
            model_path = require_safe_repo_path(model, f"{obligation_id}.model")
<<<<<<< HEAD
            require_existing_repo_file(model_path, f"{obligation_id}.model")
=======
            require((ROOT / model_path).is_file(), f"model file is missing: {model_path}")
>>>>>>> origin/agent/formal-methods-20260730-segment-lifecycle
        if bounded:
            require(model is not None, f"{obligation_id} is bounded but does not name a model")

    require(safety_count > 0, "at least one safety obligation is required")
    return repository, len(obligations)


def main() -> None:
    try:
        document = json.loads(REGISTER.read_text(encoding="utf-8"))
        repository, obligation_count = validate_document(document)
    except (OSError, json.JSONDecodeError, ContractError) as error:
        raise SystemExit(f"formal obligation register invalid: {error}") from error
    print(f"formal obligation register: {repository}: {obligation_count} obligations validated")


if __name__ == "__main__":
    main()
