#!/usr/bin/env python3
"""Stage and promote the remaining canonical StreemPilot repository gaps.

The sealed 32-repository manifest is immutable. Only four reviewed StreemPilot
records are selected. Their Git histories are reconstructed from the pinned
coordinator source, then transported first to StreemPilot-test and, only after
a verified same-run staging receipt, to StreemPilot.

All remote creation is private-only and create-only. Existing non-empty
histories are immutable: an exact matching main is preserved, any other state
fails closed, and no force push or visibility patch exists.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Iterable

from private_repository_creation import ensure_private_repository


EXACT_MODULE_PATH = Path(__file__).with_name("publish_exact_private_repository_gaps.py")
SPEC = importlib.util.spec_from_file_location("sealed_exact_gap_support", EXACT_MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise SystemExit(f"unable to load {EXACT_MODULE_PATH}")
EXACT = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = EXACT
SPEC.loader.exec_module(EXACT)

CANONICAL_ORGANIZATION = "StreemPilot"
STAGE_ORGANIZATION = "StreemPilot-test"
PRODUCTION_ORGANIZATION = "StreemPilot"
EXPECTED_REPOSITORIES = (
    "StreemPilot/streempilot-compositor.rs",
    "StreemPilot/streempilot-destinations",
    "StreemPilot/streempilot-recording.rs",
    "StreemPilot/streempilot-webrtc-adapter.rs",
)
_SHA_RE = re.compile(r"^[0-9a-f]{40}$")


def fail(message: str) -> None:
    raise RuntimeError(message)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=("stage", "production"))
    parser.add_argument("--evidence-out", type=Path, required=True)
    parser.add_argument("--stage-evidence", type=Path)
    return parser.parse_args()


def canonical_records(records: Iterable[dict[str, object]]) -> list[dict[str, object]]:
    expected_by_identity = {
        full_name.casefold(): full_name for full_name in EXPECTED_REPOSITORIES
    }
    by_identity: dict[str, dict[str, object]] = {}
    for record in records:
        full_name = record.get("full_name")
        if not isinstance(full_name, str):
            continue
        identity = full_name.casefold()
        if identity in by_identity:
            fail(f"duplicate repository identity in sealed manifest: {full_name}")
        by_identity[identity] = record

    missing = [
        expected_by_identity[identity]
        for identity in expected_by_identity
        if identity not in by_identity
    ]
    if missing:
        fail(f"canonical StreemPilot gaps missing from sealed manifest: {sorted(missing)}")

    selected = [by_identity[name.casefold()] for name in EXPECTED_REPOSITORIES]
    for record, expected_full_name in zip(selected, EXPECTED_REPOSITORIES, strict=True):
        full_name = record.get("full_name")
        if not isinstance(full_name, str) or full_name.casefold() != expected_full_name.casefold():
            fail(f"canonical repository identity changed: {full_name!r} != {expected_full_name}")
        owner, separator, name = full_name.partition("/")
        expected_owner, _, expected_name = expected_full_name.partition("/")
        if (
            separator != "/"
            or owner.casefold() != expected_owner.casefold()
            or expected_owner.casefold() != CANONICAL_ORGANIZATION.casefold()
            or name != expected_name
        ):
            fail(f"canonical repository escaped StreemPilot boundary: {full_name}")
        if record.get("default_branch") != "main":
            fail(f"canonical repository must use main: {full_name}")
        commit = record.get("commit")
        if not isinstance(commit, str) or _SHA_RE.fullmatch(commit) is None:
            fail(f"canonical repository has invalid sealed SHA: {full_name}")
    return selected


def target_organization(target: str) -> str:
    if target == "stage":
        return STAGE_ORGANIZATION
    if target == "production":
        return PRODUCTION_ORGANIZATION
    fail(f"invalid target: {target}")


def canonical_full_name(record: dict[str, object]) -> str:
    name = record.get("name")
    if not isinstance(name, str) or not name:
        fail("sealed record is missing repository name")
    full_name = f"{CANONICAL_ORGANIZATION}/{name}"
    if full_name not in EXPECTED_REPOSITORIES:
        fail(f"repository is outside exact canonical allowlist: {full_name}")
    return full_name


def target_full_name(record: dict[str, object], target: str) -> str:
    name = record.get("name")
    if not isinstance(name, str) or not name:
        fail("sealed record is missing repository name")
    return f"{target_organization(target)}/{name}"


def api(method: str, path: str, body: dict[str, object] | None = None) -> tuple[int, object | None]:
    return EXACT.MODULE.api(method, path, body)


def main_ref(full_name: str) -> str | None:
    return EXACT.MODULE.main_ref(full_name)


def validate_private_metadata(
    payload: object,
    expected_full_name: str,
) -> dict[str, object]:
    if not isinstance(payload, dict):
        fail(f"invalid repository response for {expected_full_name}")
    remote_full_name = payload.get("full_name")
    if (
        not isinstance(remote_full_name, str)
        or remote_full_name.casefold() != expected_full_name.casefold()
    ):
        fail(
            f"repository identity mismatch for {expected_full_name}: "
            f"{remote_full_name!r}"
        )
    if payload.get("private") is not True or payload.get("visibility") != "private":
        fail(f"repository is not private: {expected_full_name}")
    if payload.get("default_branch") != "main":
        fail(f"repository default branch is not main: {expected_full_name}")
    repository_id = payload.get("id")
    if not isinstance(repository_id, int) or repository_id <= 0:
        fail(f"repository id is invalid: {expected_full_name}")
    return payload


def existing_repository(full_name: str, expected_sha: str) -> dict[str, object] | None:
    status, payload = api("GET", f"/repos/{full_name}")
    if status == 404:
        return None
    if status != 200:
        fail(f"failed to inspect {full_name}: HTTP {status}")
    metadata = validate_private_metadata(payload, full_name)
    actual = main_ref(full_name)
    if actual != expected_sha:
        fail(
            f"refusing to mutate existing {full_name}: "
            f"main={actual!r} expected={expected_sha}"
        )
    return metadata


def push_exact_main(local_repository: Path, full_name: str, expected_sha: str) -> None:
    local_sha = subprocess.run(
        ["git", "-C", str(local_repository), "rev-parse", "HEAD"],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    ).stdout.strip()
    if local_sha != expected_sha:
        fail(f"local sealed history drift for {full_name}: {local_sha} != {expected_sha}")

    current = main_ref(full_name)
    if current is not None:
        if current != expected_sha:
            fail(f"refusing to overwrite {full_name}: {current} != {expected_sha}")
        return

    token = os.environ.get("GH_TOKEN", "")
    if len(token) < 20:
        fail("GH_TOKEN is missing or implausibly short")

    askpass_fd, askpass_name = tempfile.mkstemp(prefix="streempilot-publish-", suffix=".sh")
    os.close(askpass_fd)
    askpass = Path(askpass_name)
    remote_name = "bounded-promotion-target"
    try:
        askpass.write_text(
            '#!/bin/sh\n'
            'case "$1" in *Username*) printf "%s\\n" x-access-token;; '
            '*) printf "%s\\n" "$GH_TOKEN";; esac\n',
            encoding="utf-8",
        )
        askpass.chmod(0o700)
        environment = os.environ.copy()
        environment.update(
            {
                "GIT_ASKPASS": str(askpass),
                "GIT_ASKPASS_REQUIRE": "force",
                "GIT_TERMINAL_PROMPT": "0",
            }
        )
        subprocess.run(
            [
                "git",
                "-C",
                str(local_repository),
                "remote",
                "add",
                remote_name,
                f"https://github.com/{full_name}.git",
            ],
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        subprocess.run(
            [
                "git",
                "-C",
                str(local_repository),
                "push",
                "--set-upstream",
                remote_name,
                "HEAD:refs/heads/main",
            ],
            check=True,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    finally:
        subprocess.run(
            ["git", "-C", str(local_repository), "remote", "remove", remote_name],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            askpass.write_text("", encoding="utf-8")
        finally:
            askpass.unlink(missing_ok=True)

    actual = main_ref(full_name)
    if actual != expected_sha:
        fail(f"remote verification failed for {full_name}: {actual} != {expected_sha}")


def stage_expected_rows(
    records: Iterable[dict[str, object]],
) -> dict[str, tuple[str, str]]:
    result: dict[str, tuple[str, str]] = {}
    for record in records:
        canonical = canonical_full_name(record)
        expected_sha = record.get("commit")
        if not isinstance(expected_sha, str) or _SHA_RE.fullmatch(expected_sha) is None:
            fail(f"invalid sealed SHA for {canonical}")
        result[canonical] = (target_full_name(record, "stage"), expected_sha)
    return result


def validate_stage_evidence(
    path: Path,
    records: Iterable[dict[str, object]],
) -> dict[str, object]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("schema_version") != 1:
        fail("stage evidence schema is not version 1")
    if document.get("target") != "stage":
        fail("promotion requires stage evidence")
    if document.get("target_organization") != STAGE_ORGANIZATION:
        fail("stage evidence organization mismatch")
    if document.get("sealed_source_repository") != EXACT.FLEET_SOURCE_REPOSITORY:
        fail("stage evidence source repository mismatch")
    if document.get("sealed_source_sha") != EXACT.FLEET_SOURCE_SHA:
        fail("stage evidence source SHA mismatch")

    rows = document.get("repositories")
    if not isinstance(rows, list) or len(rows) != len(EXPECTED_REPOSITORIES):
        fail("stage evidence repository count mismatch")
    expected = stage_expected_rows(records)
    observed: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            fail("stage evidence contains a malformed row")
        canonical = row.get("canonical_full_name")
        if not isinstance(canonical, str) or canonical not in expected:
            fail(f"stage evidence contains unexpected canonical identity: {canonical!r}")
        if canonical in observed:
            fail(f"stage evidence duplicates canonical identity: {canonical}")
        observed.add(canonical)
        expected_target, expected_sha = expected[canonical]
        if row.get("target_full_name") != expected_target:
            fail(f"stage target mismatch for {canonical}")
        if row.get("expected_sealed_sha") != expected_sha or row.get("main_sha") != expected_sha:
            fail(f"stage SHA mismatch for {canonical}")
        if row.get("visibility") != "private" or row.get("default_branch") != "main":
            fail(f"stage repository state mismatch for {canonical}")
        repository_id = row.get("repository_id")
        if not isinstance(repository_id, int) or repository_id <= 0:
            fail(f"stage repository id mismatch for {canonical}")
    if observed != set(expected):
        fail("stage evidence does not cover the exact canonical allowlist")
    return document


def publish_one(
    source_root: Path,
    record: dict[str, object],
    target: str,
) -> dict[str, object]:
    canonical = canonical_full_name(record)
    remote = target_full_name(record, target)
    expected_sha = record.get("commit")
    if not isinstance(expected_sha, str) or _SHA_RE.fullmatch(expected_sha) is None:
        fail(f"invalid sealed SHA for {canonical}")

    preserved = existing_repository(remote, expected_sha)
    disposition = "preserved"
    metadata = preserved
    if metadata is None:
        name = record.get("name")
        description = record.get("description")
        if not isinstance(name, str) or not isinstance(description, str):
            fail(f"invalid sealed metadata for {canonical}")
        metadata = ensure_private_repository(
            api,
            target_organization(target),
            name,
            description,
        )
        validate_private_metadata(metadata, remote)
        source_org = record.get("org")
        if not isinstance(source_org, str):
            fail(f"invalid sealed source organization for {canonical}")
        source_repository = source_root / source_org / name
        push_exact_main(source_repository, remote, expected_sha)
        disposition = "created-or-reconciled"

    status, verified = api("GET", f"/repos/{remote}")
    if status != 200:
        fail(f"postflight lookup failed for {remote}: HTTP {status}")
    verified_metadata = validate_private_metadata(verified, remote)
    actual = main_ref(remote)
    if actual != expected_sha:
        fail(f"postflight main mismatch for {remote}: {actual} != {expected_sha}")

    return {
        "canonical_full_name": canonical,
        "target_full_name": str(verified_metadata["full_name"]),
        "repository_id": int(verified_metadata["id"]),
        "visibility": verified_metadata["visibility"],
        "default_branch": verified_metadata["default_branch"],
        "main_sha": actual,
        "expected_sealed_sha": expected_sha,
        "disposition": disposition,
        "html_url": verified_metadata.get("html_url"),
    }


def publish(
    target: str,
    evidence_out: Path,
    *,
    stage_evidence: Path | None = None,
) -> None:
    work = Path(tempfile.mkdtemp(prefix=f"streempilot-{target}-promotion-"))
    try:
        source_root, manifest, _ = EXACT.reconstruct_fleet(work)
        records = manifest.get("repositories")
        if not isinstance(records, list) or len(records) != 32:
            fail("sealed fleet repository ledger is malformed")
        selected = canonical_records(records)

        if target == "production":
            if stage_evidence is None:
                fail("production promotion requires --stage-evidence")
            validate_stage_evidence(stage_evidence, selected)
        elif stage_evidence is not None:
            fail("--stage-evidence is valid only for production promotion")

        evidence_rows = [
            publish_one(source_root, record, target)
            for record in selected
        ]
        evidence = {
            "schema_version": 1,
            "target": target,
            "target_organization": target_organization(target),
            "canonical_organization": CANONICAL_ORGANIZATION,
            "sealed_source_repository": EXACT.FLEET_SOURCE_REPOSITORY,
            "sealed_source_sha": EXACT.FLEET_SOURCE_SHA,
            "generator_sha256": EXACT.FLEET_GENERATOR_SHA256,
            "expected_repositories": list(EXPECTED_REPOSITORIES),
            "repositories": evidence_rows,
        }
        evidence_out.parent.mkdir(parents=True, exist_ok=True)
        evidence_out.write_text(
            json.dumps(evidence, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        print(
            "VERIFIED_STREEMPILOT_CANONICAL_GAPS "
            f"target={target} organization={target_organization(target)} "
            f"repositories={len(evidence_rows)}"
        )
    finally:
        shutil.rmtree(work, ignore_errors=True)


def main() -> int:
    args = parse_args()
    publish(
        args.target,
        args.evidence_out.resolve(),
        stage_evidence=(
            args.stage_evidence.resolve() if args.stage_evidence is not None else None
        ),
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
