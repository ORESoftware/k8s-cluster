#!/usr/bin/env python3
"""Run the bounded publisher with its current transport and visibility contract."""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

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


def fail(message: str) -> None:
    raise RuntimeError(message)


def repair_or_validate_publisher(path: Path) -> None:
    """Repair the one known legacy escaping defect, or validate current source."""

    text = path.read_text(encoding="utf-8")
    broken = """        askpass.write_text('#!/bin/sh
case \"$1\" in *Username*) echo x-access-token;; *) echo \"$GITHUB_REPOSITORY_ADMIN_TOKEN\";; esac
')
"""
    fixed = """        askpass.write_text(
            '#!/bin/sh\\ncase \"$1\" in *Username*) echo x-access-token;; *) echo \"$GITHUB_REPOSITORY_ADMIN_TOKEN\";; esac\\n'
        )
"""

    if broken in text:
        if text.count(broken) != 1:
            fail("publisher contains an unexpected number of legacy transport defects")
        path.write_text(text.replace(broken, fixed, 1), encoding="utf-8")
        text = path.read_text(encoding="utf-8")
    elif "askpass.write_text(" not in text:
        fail("publisher lacks the bounded non-interactive Git credential transport")

    required = (
        "GITHUB_REPOSITORY_ADMIN_TOKEN",
        "GIT_ASKPASS",
        "GIT_TERMINAL_PROMPT",
        "x-access-token",
    )
    missing = [snippet for snippet in required if snippet not in text]
    if missing:
        fail(f"publisher credential contract is incomplete: {missing}")

    subprocess.run([sys.executable, "-m", "py_compile", str(path)], check=True)


def ensure_private_repository(owner: str, name: str, description: str) -> dict[str, Any]:
    """Create extracted repositories privately and reject visibility drift."""

    status, current = MODULE.api("GET", f"/repos/{owner}/{name}")
    if status == 404:
        status, current = MODULE.api(
            "POST",
            f"/orgs/{owner}/repos",
            {
                "name": name,
                "description": description,
                "private": True,
                "has_issues": True,
                "has_projects": False,
                "has_wiki": False,
                "auto_init": False,
                "allow_squash_merge": True,
                "allow_merge_commit": True,
                "allow_rebase_merge": False,
                "delete_branch_on_merge": True,
            },
        )
        if status != 201 or not isinstance(current, dict):
            fail(f"failed to create {owner}/{name}: HTTP {status}")
        print(f"CREATED {owner}/{name}")

    if not isinstance(current, dict):
        fail(f"invalid repository response for {owner}/{name}")
    if current.get("private") is not True or current.get("visibility") != "private":
        fail(
            f"visibility mismatch for {owner}/{name}: "
            f"private={current.get('private')!r}, visibility={current.get('visibility')!r}"
        )
    return current


def publish_current_hypesiege_and_streempilot(work: Path) -> None:
    """Publish the exact reviewed schema-v2 fleet one repository at a time."""

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
    MODULE.run(
        ["git", "-C", str(carrier), "checkout", "--detach", FLEET_SOURCE_SHA]
    )
    actual_source_sha = MODULE.run(
        ["git", "-C", str(carrier), "rev-parse", "HEAD"]
    ).strip()
    if actual_source_sha != FLEET_SOURCE_SHA:
        fail(
            "fleet source checkout mismatch: "
            f"{actual_source_sha} != {FLEET_SOURCE_SHA}"
        )

    payload_dir = carrier / "repository-fleets/hypesiege-streempilot"
    checked_manifest_path = carrier / "repository-fleets/hypesiege-streempilot.json"
    reconstructor = carrier / "scripts/reconstruct_hypesiege_streempilot_fleet.py"
    publisher = carrier / "scripts/publish_hypesiege_streempilot_fleet.py"
    source_root = work / "hypesiege-streempilot-fleet"
    generated_manifest_path = work / "hypesiege-streempilot-manifest.json"
    execution_manifest_path = work / "hypesiege-streempilot-private-execution.json"

    MODULE.run(
        [
            sys.executable,
            "-m",
            "py_compile",
            str(reconstructor),
            str(publisher),
        ]
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

    checked_manifest = json.loads(
        checked_manifest_path.read_text(encoding="utf-8")
    )
    generated_manifest = json.loads(
        generated_manifest_path.read_text(encoding="utf-8")
    )
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

    # Preserve the sealed reviewed ledger as provenance, but execute with the
    # repository visibility currently enforced by the protected publisher.
    # The projection helper proves that visibility is the only changed field.
    execution_manifest = project_private_execution_manifest(generated_manifest)
    execution_manifest_path.write_text(
        json.dumps(execution_manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    execution_records = execution_manifest.get("repositories")
    if not isinstance(execution_records, list) or len(execution_records) != 32:
        fail("private execution manifest repository ledger is malformed")
    if any(record.get("visibility") != "private" for record in execution_records):
        fail("private execution manifest contains a non-private repository")
    print(
        "VERIFIED private execution projection for 32 reviewed repository histories"
    )

    environment = os.environ.copy()
    environment["GITHUB_REPOSITORY_ADMIN_TOKEN"] = MODULE.TOKEN

    for record in execution_records:
        if not isinstance(record, dict):
            fail("reviewed fleet contains a non-object repository record")
        full_name = record.get("full_name")
        if not isinstance(full_name, str):
            fail("reviewed fleet contains an invalid repository identity")
        MODULE.run(
            [
                sys.executable,
                str(publisher),
                "--manifest",
                str(execution_manifest_path),
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

    for record in execution_records:
        full_name = str(record["full_name"])
        expected = str(record["commit"])
        actual = MODULE.main_ref(full_name)
        if actual != expected:
            fail(
                f"fleet verification failed for {full_name}: "
                f"{actual!r} != {expected}"
            )
        print(f"VERIFIED {full_name} {actual}")
    print("VERIFIED 32/32 HypeSiege and StreemPilot repositories")


MODULE.repair_publisher = repair_or_validate_publisher
MODULE.ensure_repository = ensure_private_repository
MODULE.publish_hypesiege_and_streempilot = publish_current_hypesiege_and_streempilot
raise SystemExit(MODULE.main())
