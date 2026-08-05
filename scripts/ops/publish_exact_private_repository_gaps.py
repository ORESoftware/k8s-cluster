#!/usr/bin/env python3
"""Publish only the four reviewed HypeSiege/StreemPilot repository gaps.

The caller supplies a short-lived organization-scoped GitHub App installation
credential through ``GH_TOKEN``.  The script reconstructs the exact sealed
32-repository source fleet, projects its reviewed histories to private
execution records, selects only the organization-specific exact allowlist, and
creates only missing repositories.  Existing private repositories and their
``main`` histories are preserved byte-for-byte.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from typing import Iterable

from repository_fleet_remote_state import (
    RemoteFleetStateError,
    classify_remote_fleet,
    verify_created_repositories,
    verify_preserved_existing,
)
from repository_fleet_visibility import project_private_execution_manifest


MODULE_PATH = Path(__file__).with_name("publish_missing_org_repositories.py")
SPEC = importlib.util.spec_from_file_location("bounded_missing_repo_publisher", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise SystemExit(f"unable to load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

FLEET_SOURCE_REPOSITORY = "ORESoftware/ai-agent-coordinator.rs"
FLEET_SOURCE_SHA = "5d9a0c2cb44dff607bc3953954ce4b9af08e5789"
FLEET_GENERATOR_SHA256 = (
    "a57b00961ee57ae09bf3bb2e2d09afbdd1ddbbbde832b027802f82a1fc5dfa84"
)
EXPECTED_REPOSITORIES: dict[str, tuple[str, ...]] = {
    "hypesiege": (
        "hypesiege/hypesiege-analytics.rs",
        "hypesiege/hypesiege-publishing-worker.rs",
        "hypesiege/hypesiege-scheduler.rs",
    ),
    "StreemPilot": ("StreemPilot/streempilot-media-router.rs",),
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--organization", required=True, choices=tuple(EXPECTED_REPOSITORIES))
    parser.add_argument("--evidence-out", type=Path, required=True)
    return parser.parse_args()


def fail(message: str) -> None:
    raise RuntimeError(message)


def selected_records(
    records: Iterable[dict[str, object]], organization: str
) -> list[dict[str, object]]:
    expected = EXPECTED_REPOSITORIES.get(organization)
    if expected is None:
        fail(f"organization is not in the exact allowlist: {organization}")
    expected_set = set(expected)
    by_name: dict[str, dict[str, object]] = {}
    for record in records:
        full_name = record.get("full_name")
        if isinstance(full_name, str):
            if full_name in by_name:
                fail(f"duplicate repository identity in sealed manifest: {full_name}")
            by_name[full_name] = record
    missing = expected_set - set(by_name)
    if missing:
        fail(f"exact repository allowlist is missing from sealed manifest: {sorted(missing)}")
    selected = [by_name[full_name] for full_name in expected]
    for record, full_name in zip(selected, expected, strict=True):
        if record.get("full_name") != full_name:
            fail(f"selected record identity changed: {record.get('full_name')} != {full_name}")
        owner, separator, _ = full_name.partition("/")
        if separator != "/" or owner.casefold() != organization.casefold():
            fail(f"repository escaped organization boundary: {full_name}")
        if record.get("default_branch") != "main":
            fail(f"reviewed repository must use main: {full_name}")
        commit = record.get("commit")
        if not isinstance(commit, str) or len(commit) != 40 or commit.lower() != commit:
            fail(f"reviewed repository has invalid commit identity: {full_name}")
    return selected


def repository_lookup(full_name: str) -> tuple[int, dict[str, object] | None]:
    return MODULE.api("GET", f"/repos/{full_name}")


def reconstruct_fleet(work: Path) -> tuple[Path, dict[str, object], Path]:
    carrier = work / "fleet-carrier"
    MODULE.run(
        [
            "git",
            "clone",
            "--filter=blob:none",
            "--no-checkout",
            f"https://github.com/{FLEET_SOURCE_REPOSITORY}.git",
            str(carrier),
        ]
    )
    MODULE.run(
        [
            "git",
            "-C",
            str(carrier),
            "fetch",
            "--depth=1",
            "origin",
            FLEET_SOURCE_SHA,
        ]
    )
    MODULE.run(["git", "-C", str(carrier), "checkout", "--detach", FLEET_SOURCE_SHA])
    actual_source_sha = MODULE.run(
        ["git", "-C", str(carrier), "rev-parse", "HEAD"]
    ).strip()
    if actual_source_sha != FLEET_SOURCE_SHA:
        fail(f"fleet source checkout mismatch: {actual_source_sha} != {FLEET_SOURCE_SHA}")

    payload_dir = carrier / "repository-fleets/hypesiege-streempilot"
    checked_manifest_path = carrier / "repository-fleets/hypesiege-streempilot.json"
    reconstructor = carrier / "scripts/reconstruct_hypesiege_streempilot_fleet.py"
    publisher = carrier / "scripts/publish_hypesiege_streempilot_fleet.py"
    source_root = work / "hypesiege-streempilot-fleet"
    generated_manifest_path = work / "hypesiege-streempilot-manifest.json"

    MODULE.run(
        [sys.executable, "-m", "py_compile", str(reconstructor), str(publisher)]
    )
    MODULE.run(
        [
            sys.executable,
            str(reconstructor),
            "--payload-dir",
            str(payload_dir),
            "--output-root",
            str(source_root),
            "--manifest-out",
            str(generated_manifest_path),
        ]
    )
    checked_manifest = json.loads(checked_manifest_path.read_text(encoding="utf-8"))
    generated_manifest = json.loads(generated_manifest_path.read_text(encoding="utf-8"))
    if generated_manifest != checked_manifest:
        fail("reconstructed fleet does not exactly match the checked-in schema-v2 ledger")
    if generated_manifest.get("generator_sha256") != FLEET_GENERATOR_SHA256:
        fail("reviewed fleet generator identity changed")
    if generated_manifest.get("schema_version") != 2:
        fail("reviewed fleet manifest must use schema version 2")
    if generated_manifest.get("repository_count") != 32:
        fail("reviewed fleet must contain exactly 32 repositories")
    if generated_manifest.get("total_tracked_files") != 888:
        fail("reviewed fleet tracked-file total changed")
    if generated_manifest.get("total_gitlinks") != 30:
        fail("reviewed fleet gitlink total changed")
    if generated_manifest.get("organizations") != {
        "hypesiege": 15,
        "streempilot": 17,
    }:
        fail("reviewed fleet organization counts changed")
    records = generated_manifest.get("repositories")
    if not isinstance(records, list) or len(records) != 32:
        fail("reviewed fleet repository ledger is malformed")
    if any(
        not isinstance(record, dict) or record.get("visibility") != "public"
        for record in records
    ):
        fail("sealed product-intent ledger is no longer uniformly public")
    return source_root, generated_manifest, publisher


def publish_exact(organization: str, evidence_out: Path) -> None:
    work = Path(tempfile.mkdtemp(prefix="exact-private-repository-gaps-"))
    try:
        source_root, generated_manifest, publisher = reconstruct_fleet(work)
        execution_manifest = project_private_execution_manifest(generated_manifest)
        records = execution_manifest.get("repositories")
        if not isinstance(records, list) or len(records) != 32:
            fail("private execution manifest repository ledger is malformed")
        selected = selected_records(records, organization)
        if any(record.get("visibility") != "private" for record in selected):
            fail("exact execution selection contains a non-private repository")

        # The sealed publisher validates the complete 32-repository ledger before
        # honoring its single --repository selector. Keep fleet totals and all
        # records intact here; the exact allowlist above controls only which
        # repository invocations and evidence rows are permitted.
        publisher_manifest_path = work / "private-fleet-execution.json"
        publisher_manifest_path.write_text(
            json.dumps(execution_manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

        try:
            missing_records, existing_snapshot = classify_remote_fleet(
                selected,
                repository_lookup=repository_lookup,
                main_ref_lookup=MODULE.main_ref,
            )
        except RemoteFleetStateError as error:
            fail(str(error))

        expected_names = set(EXPECTED_REPOSITORIES[organization])
        observed_names = {str(record["full_name"]) for record in selected}
        if observed_names != expected_names:
            fail(
                "exact execution selection changed: "
                f"{sorted(observed_names)} != {sorted(expected_names)}"
            )
        print(
            "VERIFIED_EXACT_PREFLIGHT "
            f"organization={organization} missing={len(missing_records)} "
            f"preserved={len(existing_snapshot)}"
        )

        environment = os.environ.copy()
        environment["GITHUB_REPOSITORY_ADMIN_TOKEN"] = MODULE.TOKEN
        for record in missing_records:
            full_name = str(record["full_name"])
            if full_name not in expected_names:
                fail(f"refusing to publish repository outside exact allowlist: {full_name}")
            MODULE.run(
                [
                    sys.executable,
                    str(publisher),
                    "--manifest",
                    str(publisher_manifest_path),
                    "--source-root",
                    str(source_root),
                    "--repository",
                    full_name,
                    "--execute",
                    "--confirm-repository",
                    full_name,
                ],
                env=environment,
            )

        try:
            verify_created_repositories(
                missing_records,
                repository_lookup=repository_lookup,
                main_ref_lookup=MODULE.main_ref,
            )
            verify_preserved_existing(
                existing_snapshot,
                repository_lookup=repository_lookup,
                main_ref_lookup=MODULE.main_ref,
            )
        except RemoteFleetStateError as error:
            fail(str(error))

        evidence_records: list[dict[str, object]] = []
        created_names = {str(record["full_name"]) for record in missing_records}
        for record in selected:
            full_name = str(record["full_name"])
            status, repository = repository_lookup(full_name)
            if status != 200 or not isinstance(repository, dict):
                fail(f"postflight repository lookup failed for {full_name}: HTTP {status}")
            actual_sha = MODULE.main_ref(full_name)
            if repository.get("private") is not True:
                fail(f"postflight repository is not private: {full_name}")
            if repository.get("visibility") != "private":
                fail(f"postflight repository visibility is not private: {full_name}")
            if repository.get("default_branch") != "main":
                fail(f"postflight default branch is not main: {full_name}")
            if full_name in created_names and actual_sha != record.get("commit"):
                fail(
                    f"new repository head mismatch for {full_name}: "
                    f"{actual_sha} != {record.get('commit')}"
                )
            preserved = existing_snapshot.get(full_name)
            if preserved is not None and actual_sha != preserved.get("head"):
                fail(f"preserved repository head changed for {full_name}")
            evidence_records.append(
                {
                    "full_name": full_name,
                    "repository_id": repository.get("id"),
                    "visibility": repository.get("visibility"),
                    "default_branch": repository.get("default_branch"),
                    "main_sha": actual_sha,
                    "expected_sealed_sha": record.get("commit"),
                    "disposition": "created" if full_name in created_names else "preserved",
                    "html_url": repository.get("html_url"),
                }
            )

        evidence = {
            "schema_version": 1,
            "organization": organization,
            "sealed_source_repository": FLEET_SOURCE_REPOSITORY,
            "sealed_source_sha": FLEET_SOURCE_SHA,
            "generator_sha256": FLEET_GENERATOR_SHA256,
            "expected_repositories": list(EXPECTED_REPOSITORIES[organization]),
            "created_count": len(missing_records),
            "preserved_count": len(existing_snapshot),
            "repositories": evidence_records,
        }
        evidence_out.parent.mkdir(parents=True, exist_ok=True)
        evidence_out.write_text(
            json.dumps(evidence, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        print(
            "VERIFIED_EXACT_PRIVATE_GAPS "
            f"organization={organization} created={len(missing_records)} "
            f"preserved={len(existing_snapshot)} total={len(selected)}"
        )
    finally:
        shutil.rmtree(work, ignore_errors=True)


def main() -> int:
    args = parse_args()
    publish_exact(args.organization, args.evidence_out.resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
