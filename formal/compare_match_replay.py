#!/usr/bin/env python3
"""Compare the finite model with production Rust in debug and release profiles."""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
from collections import deque
from pathlib import Path
from types import ModuleType
from typing import Any, Sequence

ROOT = Path(__file__).resolve().parents[1]
MODEL_PATH = ROOT / "formal" / "match_model.py"


def load_model() -> ModuleType:
    spec = importlib.util.spec_from_file_location("akrion_match_model", MODEL_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load model from {MODEL_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def bounded_cases(model: ModuleType) -> list[tuple[str, ...]]:
    """Keep one shortest witness per state and exercise every bounded transition."""
    depth = int(model.load_manifest()["limits"]["transition_depth"])
    initial = model.State()
    seen = {initial}
    queue = deque([(initial, tuple())])
    cases: list[tuple[str, ...]] = [tuple()]

    while queue:
        state, witness = queue.popleft()
        if len(witness) >= depth:
            continue
        for action in model.ACTIONS:
            events = witness + (action,)
            cases.append(events)
            after = model.transition(state, action)
            if after not in seen:
                seen.add(after)
                queue.append((after, events))

    return cases


def invoke(command: Sequence[str], cases: Sequence[tuple[str, ...]]) -> list[dict[str, Any]]:
    payload = "".join(
        json.dumps({"op": "replay", "events": list(events)}, separators=(",", ":")) + "\n"
        for events in cases
    )
    environment = os.environ.copy()
    environment.update(
        {
            "CARGO_INCREMENTAL": "0",
            "LC_ALL": "C",
            "RUST_BACKTRACE": "1",
            "TZ": "UTC",
        }
    )
    completed = subprocess.run(
        list(command),
        cwd=ROOT,
        env=environment,
        input=payload,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(command)}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    records = [json.loads(line) for line in completed.stdout.splitlines() if line.strip()]
    if len(records) != len(cases):
        raise RuntimeError(
            f"{' '.join(command)} returned {len(records)} records for {len(cases)} requests"
        )
    return records


def compare(
    expected: Sequence[dict[str, Any]],
    actual: Sequence[dict[str, Any]],
    cases: Sequence[tuple[str, ...]],
    label: str,
) -> None:
    for index, (left, right) in enumerate(zip(expected, actual, strict=True)):
        if left != right:
            events = list(cases[index])
            raise AssertionError(
                f"{label} mismatch for events={events!r}\n"
                f"expected={json.dumps(left, sort_keys=True)}\n"
                f"actual={json.dumps(right, sort_keys=True)}"
            )


def main() -> None:
    model = load_model()
    cases = bounded_cases(model)
    model_records = invoke(["python3", "formal/match_model.py", "--json-stdin"], cases)
    debug_records = invoke(["cargo", "run", "--quiet", "--bin", "fm_match_adapter"], cases)
    debug_repeat = invoke(["cargo", "run", "--quiet", "--bin", "fm_match_adapter"], cases)
    release_records = invoke(
        ["cargo", "run", "--quiet", "--release", "--bin", "fm_match_adapter"], cases
    )

    compare(model_records, debug_records, cases, "model-to-debug")
    compare(debug_records, debug_repeat, cases, "debug deterministic replay")
    compare(debug_records, release_records, cases, "debug-to-release")

    print(
        json.dumps(
            {
                "status": "ok",
                "model": "akrion-match-lifecycle",
                "cases": len(cases),
                "profiles": ["model", "debug", "debug-repeat", "release"],
                "claim": "bounded-production-refinement",
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
