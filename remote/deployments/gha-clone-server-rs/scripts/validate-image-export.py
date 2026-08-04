#!/usr/bin/env python3
"""Validate a BuildKit local export and its SBOM/provenance attestations."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any, Iterable


def walk_json(value: Any) -> Iterable[dict[str, Any]]:
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from walk_json(child)
    elif isinstance(value, list):
        for child in value:
            yield from walk_json(child)


def fail(message: str) -> None:
    raise SystemExit(f"image-export validation failed: {message}")


def main() -> None:
    if len(sys.argv) != 3:
        fail("usage: validate-image-export.py <export-root> <expected-binary>")

    root = Path(sys.argv[1]).resolve()
    expected_binary = sys.argv[2]
    if expected_binary not in {"gha-clone-server", "gha-executor-router"}:
        fail(f"unexpected binary name {expected_binary!r}")
    if not root.is_dir():
        fail(f"export root does not exist: {root}")

    binary = root / "usr" / "local" / "bin" / expected_binary
    if not binary.is_file() or not os.access(binary, os.X_OK):
        fail(f"expected executable is missing: {binary}")

    other = (
        "gha-executor-router"
        if expected_binary == "gha-clone-server"
        else "gha-clone-server"
    )
    if (root / "usr" / "local" / "bin" / other).exists():
        fail(f"runtime target unexpectedly contains {other}")

    forbidden = [
        root / "usr" / "local" / "cargo",
        root / "usr" / "local" / "rustup",
        root / "usr" / "bin" / "cargo",
        root / "usr" / "bin" / "rustc",
        root / "usr" / "bin" / "git",
    ]
    leaked = [str(path.relative_to(root)) for path in forbidden if path.exists()]
    if leaked:
        fail(f"runtime contains build tooling: {', '.join(leaked)}")

    json_files = sorted(root.glob("*.json"))
    if not json_files:
        fail("BuildKit did not export attestation JSON files")

    saw_spdx = False
    saw_provenance = False
    provenance_subject_digest = False
    parsed_files: list[str] = []

    for path in json_files:
        try:
            payload = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError) as error:
            fail(f"cannot parse {path.name}: {error}")
        parsed_files.append(path.name)
        for item in walk_json(payload):
            predicate_type = item.get("predicateType")
            if predicate_type == "https://spdx.dev/Document":
                saw_spdx = True
            if item.get("SPDXID") == "SPDXRef-DOCUMENT":
                saw_spdx = True
            if isinstance(predicate_type, str) and (
                predicate_type.startswith("https://slsa.dev/provenance/")
                or predicate_type.startswith("https://slsa.dev/provenance")
            ):
                saw_provenance = True
            subjects = item.get("subject")
            if isinstance(subjects, list):
                for subject in subjects:
                    if not isinstance(subject, dict):
                        continue
                    digest = subject.get("digest")
                    if isinstance(digest, dict) and any(
                        isinstance(value, str) and value
                        for value in digest.values()
                    ):
                        provenance_subject_digest = True

    if not saw_spdx:
        fail(f"SPDX SBOM not found in {parsed_files}")
    if not saw_provenance:
        fail(f"SLSA provenance not found in {parsed_files}")
    if not provenance_subject_digest:
        fail("attestation subject does not contain a digest")

    print(
        json.dumps(
            {
                "ok": True,
                "binary": expected_binary,
                "attestations": parsed_files,
                "runtimeToolingLeak": False,
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
