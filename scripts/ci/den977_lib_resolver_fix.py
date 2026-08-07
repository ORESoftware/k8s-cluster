"""Semantic lib/planner split for DEN-977 plus the later DEN-1606 hardening."""

from __future__ import annotations

import pathlib
import subprocess
import tempfile

LIB_PATH = "remote/deployments/gha-clone-server-rs/src/lib.rs"
PLANNER_PATH = "remote/deployments/gha-clone-server-rs/src/planner.rs"


def _required(source: str, markers: tuple[str, ...], label: str) -> None:
    for marker in markers:
        if marker not in source:
            raise SystemExit(f"{label} lost required marker: {marker}")


def resolve_lib(current_data: bytes | None, reviewed_data: bytes | None) -> bytes:
    if current_data is None or reviewed_data is None:
        raise SystemExit("lib.rs semantic inputs are incomplete")
    current = current_data.decode("utf-8")
    reviewed = reviewed_data.decode("utf-8")
    _required(
        current,
        (
            "fn validate_workflow_document",
            "maxWorkflowBytes must be greater than zero",
            "workflowPath must stay under .github/workflows as one direct ASCII",
            "mod den_1606_planner_input_tests",
            "mod den_1606_planner_input_followup_tests",
        ),
        "current DEN-1606 generic planner",
    )
    _required(
        reviewed,
        (
            "mod planner;",
            "mod msgint_contract;",
            "classify_msgint_workflow",
            "ContractMatch::Reject",
            "reviewed Messaging Intel contract and generic planner produced different job sets",
        ),
        "reviewed Messaging Intel wrapper",
    )
    if "fn validate_workflow_document" in reviewed:
        raise SystemExit("reviewed wrapper unexpectedly embeds the generic planner")
    return reviewed_data


def resolve_planner(
    current_generic: bytes | None,
    base_generic: bytes | None,
    reviewed_planner: bytes | None,
) -> bytes:
    if current_generic is None or base_generic is None or reviewed_planner is None:
        raise SystemExit("planner.rs cross-path semantic inputs are incomplete")

    with tempfile.TemporaryDirectory(prefix="den977-planner-") as temp:
        root = pathlib.Path(temp)
        current_file = root / "current-lib.rs"
        base_file = root / "base-lib.rs"
        reviewed_file = root / "reviewed-planner.rs"
        current_file.write_bytes(current_generic)
        base_file.write_bytes(base_generic)
        reviewed_file.write_bytes(reviewed_planner)
        result = subprocess.run(
            [
                "git",
                "merge-file",
                "-p",
                "--diff3",
                str(current_file),
                str(base_file),
                str(reviewed_file),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    if result.returncode != 0:
        output = result.stdout.decode("utf-8", errors="replace")
        lines = output.splitlines()
        excerpts: list[str] = []
        for index, line in enumerate(lines):
            if line.startswith("<<<<<<<"):
                excerpts.append("\n".join(lines[max(0, index - 8) : index + 80]))
        detail = "\n--- cross-path planner conflict ---\n".join(excerpts)[:30000]
        raise SystemExit(
            "cross-path merge of current lib.rs, base lib.rs, and reviewed planner.rs "
            f"reported {result.returncode} conflict(s):\n{detail}"
        )

    merged = result.stdout.decode("utf-8")
    _required(
        merged,
        (
            "fn validate_workflow_document",
            "maxWorkflowBytes must be greater than zero",
            "workflowPath must stay under .github/workflows as one direct ASCII",
            "mod den_1606_planner_input_tests",
            "mod den_1606_planner_input_followup_tests",
            "secret-bearing setup inputs are unsupported",
            "secret-bearing step environments are unsupported",
            "fixed profiles do not forward caller-selected variables",
        ),
        "conceptually merged generic planner",
    )
    for marker in ("<<<<<<<", "|||||||", "=======", ">>>>>>>"):
        if marker in merged:
            raise SystemExit(f"conceptually merged planner retained {marker!r}")
    if "classify_msgint_workflow" in merged:
        raise SystemExit("generic planner unexpectedly contains the reserved contract wrapper")
    return result.stdout
