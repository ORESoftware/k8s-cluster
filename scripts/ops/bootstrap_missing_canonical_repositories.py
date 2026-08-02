#!/usr/bin/env python3
"""Create canonical repository gaps before strict history reconciliation.

This bootstrap is intentionally non-destructive:

* missing/empty canonical repositories receive their exact sealed ``main``;
* existing repository histories are never force-pushed or replaced;
* approved visibility metadata is reconciled independently of Git history;
* divergent existing ``main`` branches are reported for later semantic merges;
* the two extracted repositories are created privately when absent.

The strict publisher/finalizer still runs afterwards and remains responsible for
exact-SHA completion. This phase exists so a divergent existing repository cannot
prevent later missing repositories from being materialized on GitHub.
"""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

CORE_PATH = Path(__file__).with_name("publish_missing_org_repositories.py")
SPEC = importlib.util.spec_from_file_location("critical_org_core", CORE_PATH)
if SPEC is None or SPEC.loader is None:
    raise SystemExit(f"unable to load {CORE_PATH}")
CORE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CORE
SPEC.loader.exec_module(CORE)

FLEET_SOURCE_REPOSITORY = "ORESoftware/ai-agent-coordinator.rs"
FLEET_SOURCE_SHA = "5d9a0c2cb44dff607bc3953954ce4b9af08e5789"
FLEET_GENERATOR_SHA256 = (
    "a57b00961ee57ae09bf3bb2e2d09afbdd1ddbbbde832b027802f82a1fc5dfa84"
)
EXPECTED_REPOSITORIES = 32
EXPECTED_FILES = 888
EXPECTED_GITLINKS = 30


def fail(message: str) -> None:
    raise RuntimeError(message)


def run(args: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> str:
    completed = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if completed.returncode:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(args)}\n{completed.stdout}"
        )
    if completed.stdout:
        print(completed.stdout, end="")
    return completed.stdout


def reconcile_visibility(full_name: str, visibility: str, current: dict[str, Any]) -> dict[str, Any]:
    expected_private = visibility == "private"
    if current.get("visibility") == visibility and current.get("private") is expected_private:
        return current

    status, updated = CORE.api(
        "PATCH",
        f"/repos/{full_name}",
        {"private": expected_private, "visibility": visibility},
    )
    if status != 200 or not isinstance(updated, dict):
        fail(f"failed to reconcile {full_name} visibility: HTTP {status}")
    if updated.get("visibility") != visibility or updated.get("private") is not expected_private:
        fail(
            f"{full_name}: visibility reconciliation returned "
            f"private={updated.get('private')!r}, visibility={updated.get('visibility')!r}"
        )
    print(f"VISIBILITY {full_name} {visibility}")
    return updated


def ensure_private_repository(owner: str, name: str, description: str) -> dict[str, Any]:
    full_name = f"{owner}/{name}"
    status, current = CORE.api("GET", f"/repos/{full_name}")
    if status == 404:
        status, current = CORE.api(
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
            fail(f"failed to create {full_name}: HTTP {status}")
        print(f"CREATED {full_name}")
    if not isinstance(current, dict):
        fail(f"invalid repository metadata for {full_name}")
    return reconcile_visibility(full_name, "private", current)


def load_reconstructed_fleet(work: Path) -> tuple[Path, dict[str, Any], Path]:
    carrier = work / "fleet-carrier"
    run(
        [
            "git",
            "clone",
            "--filter=blob:none",
            "--no-checkout",
            f"https://github.com/{FLEET_SOURCE_REPOSITORY}.git",
            str(carrier),
        ]
    )
    run(["git", "-C", str(carrier), "fetch", "--depth=1", "origin", FLEET_SOURCE_SHA])
    run(["git", "-C", str(carrier), "checkout", "--detach", FLEET_SOURCE_SHA])
    if run(["git", "-C", str(carrier), "rev-parse", "HEAD"]).strip() != FLEET_SOURCE_SHA:
        fail("fleet source checkout drifted")

    payload_dir = carrier / "repository-fleets/hypesiege-streempilot"
    checked_manifest_path = carrier / "repository-fleets/hypesiege-streempilot.json"
    reconstructor = carrier / "scripts/reconstruct_hypesiege_streempilot_fleet.py"
    publisher = carrier / "scripts/publish_hypesiege_streempilot_fleet.py"
    source_root = work / "hypesiege-streempilot-fleet"
    generated_manifest_path = work / "hypesiege-streempilot-manifest.json"

    run([sys.executable, "-m", "py_compile", str(reconstructor), str(publisher)])
    run(
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

    checked = json.loads(checked_manifest_path.read_text(encoding="utf-8"))
    generated = json.loads(generated_manifest_path.read_text(encoding="utf-8"))
    if generated != checked:
        fail("reconstructed fleet differs from the checked-in schema-v2 ledger")
    if generated.get("schema_version") != 2:
        fail("fleet schema version changed")
    if generated.get("generator_sha256") != FLEET_GENERATOR_SHA256:
        fail("fleet generator identity changed")
    if generated.get("repository_count") != EXPECTED_REPOSITORIES:
        fail("fleet repository count changed")
    if generated.get("total_tracked_files") != EXPECTED_FILES:
        fail("fleet tracked-file count changed")
    if generated.get("total_gitlinks") != EXPECTED_GITLINKS:
        fail("fleet gitlink count changed")
    if generated.get("organizations") != {"hypesiege": 15, "streempilot": 17}:
        fail("fleet organization counts changed")
    records = generated.get("repositories")
    if not isinstance(records, list) or len(records) != EXPECTED_REPOSITORIES:
        fail("fleet repository ledger is malformed")
    return source_root, generated, publisher


def bootstrap_fleet(work: Path) -> dict[str, Any]:
    source_root, manifest, publisher = load_reconstructed_fleet(work)
    records = manifest["repositories"]
    publish_records: list[dict[str, Any]] = []
    exact: list[str] = []
    divergent: list[dict[str, str]] = []
    visibility_reconciled: list[str] = []

    for record in records:
        if not isinstance(record, dict):
            fail("fleet contains a non-object record")
        full_name = str(record["full_name"])
        expected_visibility = str(record["visibility"])
        status, current = CORE.api("GET", f"/repos/{full_name}")
        if status == 404:
            publish_records.append(record)
            continue
        if not isinstance(current, dict):
            fail(f"invalid repository metadata for {full_name}")
        before_visibility = current.get("visibility")
        current = reconcile_visibility(full_name, expected_visibility, current)
        if before_visibility != expected_visibility:
            visibility_reconciled.append(full_name)
        actual = CORE.main_ref(full_name)
        if actual is None:
            publish_records.append(record)
        elif actual == str(record["commit"]):
            exact.append(full_name)
        else:
            divergent.append(
                {"repository": full_name, "remote_main": actual, "sealed_main": str(record["commit"])}
            )
            print(f"PRESERVED_DIVERGENT {full_name} remote={actual} sealed={record['commit']}")

    # Child repositories must exist before either monorepo can be published.
    publish_records.sort(key=lambda item: (item.get("kind") == "monorepo", str(item["full_name"])))
    environment = os.environ.copy()
    environment["GITHUB_REPOSITORY_ADMIN_TOKEN"] = CORE.TOKEN
    created: list[str] = []
    for record in publish_records:
        full_name = str(record["full_name"])
        run(
            [
                sys.executable,
                str(publisher),
                "--manifest",
                str(work / "hypesiege-streempilot-manifest.json"),
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
        observed = CORE.main_ref(full_name)
        expected = str(record["commit"])
        if observed != expected:
            fail(f"{full_name}: bootstrap main {observed!r} != {expected}")
        created.append(full_name)

    missing_after: list[str] = []
    for record in records:
        full_name = str(record["full_name"])
        if CORE.main_ref(full_name) is None:
            missing_after.append(full_name)
    if missing_after:
        fail(f"canonical fleet still has missing/empty repositories: {missing_after}")

    summary = {
        "created_or_initialized": created,
        "already_exact": exact,
        "preserved_divergent": divergent,
        "visibility_reconciled": visibility_reconciled,
        "repository_objects_with_main": EXPECTED_REPOSITORIES,
    }
    print(json.dumps({"fleet_bootstrap": summary}, sort_keys=True))
    return summary


def bootstrap_extracted(work: Path) -> dict[str, str]:
    CORE.ensure_repository = ensure_private_repository
    results: dict[str, str] = {}

    meta = "meta-agents-demo/meta-agent-control-plane.rs"
    status, current = CORE.api("GET", f"/repos/{meta}")
    if status == 404 or CORE.main_ref(meta) is None:
        CORE.publish_meta_agents(work)
        results[meta] = "created"
    else:
        if not isinstance(current, dict):
            fail(f"invalid repository metadata for {meta}")
        reconcile_visibility(meta, "private", current)
        results[meta] = "preserved"

    file_tunnel = "file-tunnel/ftnl-mcp-server.rs"
    status, current = CORE.api("GET", f"/repos/{file_tunnel}")
    if status == 404 or CORE.main_ref(file_tunnel) is None:
        CORE.publish_file_tunnel_mcp(work)
        results[file_tunnel] = "created"
    else:
        if not isinstance(current, dict):
            fail(f"invalid repository metadata for {file_tunnel}")
        reconcile_visibility(file_tunnel, "private", current)
        results[file_tunnel] = "preserved"

    print(json.dumps({"extracted_bootstrap": results}, sort_keys=True))
    return results


def main() -> int:
    work = Path(tempfile.mkdtemp(prefix="canonical-repository-gap-bootstrap-"))
    try:
        fleet = bootstrap_fleet(work)
        extracted = bootstrap_extracted(work)
    finally:
        shutil.rmtree(work, ignore_errors=True)

    print(
        json.dumps(
            {
                "status": "repository-gaps-materialized",
                "fleet": fleet,
                "extracted": extracted,
                "strict_reconciliation_required": bool(fleet["preserved_divergent"]),
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
