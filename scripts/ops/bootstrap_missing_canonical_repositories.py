#!/usr/bin/env python3
"""Materialize canonical repository gaps without overwriting live Git history.

This bounded bootstrap phase has four rules:

* HypeSiege and StreemPilot repositories follow the sealed public schema-v2 ledger;
* missing or empty ``main`` refs receive only their exact sealed commit;
* existing nonempty/divergent histories are preserved for repository-specific merges;
* Meta Agents is managed by its exact public publisher, while File Tunnel remains private.

The strict publisher/finalizer remains the completion gate. No force push, deletion,
rename, or blanket history replacement is permitted here.
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
from typing import Any, Callable

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
EXPECTED_ORGANIZATIONS = {"hypesiege": 15, "streempilot": 17}
META_AGENT = "meta-agents-demo/meta-agent-control-plane.rs"
FILE_TUNNEL = "file-tunnel/ftnl-mcp-server.rs"


def fail(message: str) -> None:
    raise RuntimeError(message)


def run(
    args: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
) -> str:
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


def expected_private(visibility: str) -> bool:
    if visibility not in {"public", "private"}:
        fail(f"unsupported repository visibility: {visibility}")
    return visibility == "private"


def reconcile_visibility(
    full_name: str,
    current: dict[str, Any],
    visibility: str,
    *,
    api: Callable[..., tuple[int, object | None]] | None = None,
) -> dict[str, Any]:
    """Change approved metadata only; never touch Git refs."""
    api_call = api or CORE.api
    private = expected_private(visibility)
    if current.get("visibility") == visibility and current.get("private") is private:
        return current
    status, updated = api_call(
        "PATCH",
        f"/repos/{full_name}",
        {"private": private, "visibility": visibility},
    )
    if status != 200 or not isinstance(updated, dict):
        fail(f"failed to reconcile {full_name} visibility: HTTP {status}")
    if updated.get("visibility") != visibility or updated.get("private") is not private:
        fail(
            f"{full_name}: visibility reconciliation returned "
            f"private={updated.get('private')!r}, visibility={updated.get('visibility')!r}"
        )
    print(f"VISIBILITY {full_name} {visibility}")
    return updated


def ensure_repository(
    owner: str,
    name: str,
    description: str,
    visibility: str,
    *,
    api: Callable[..., tuple[int, object | None]] | None = None,
) -> dict[str, Any]:
    """Create a missing repository with approved visibility or reconcile metadata."""
    api_call = api or CORE.api
    full_name = f"{owner}/{name}"
    status, current = api_call("GET", f"/repos/{full_name}")
    if status == 404:
        private = expected_private(visibility)
        status, current = api_call(
            "POST",
            f"/orgs/{owner}/repos",
            {
                "name": name,
                "description": description,
                "private": private,
                "visibility": visibility,
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
        print(f"CREATED_{visibility.upper()} {full_name}")
    if not isinstance(current, dict):
        fail(f"invalid repository metadata for {full_name}")
    return reconcile_visibility(full_name, current, visibility, api=api_call)


def load_reconstructed_fleet(work: Path) -> tuple[Path, dict[str, Any]]:
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
    source_root = work / "hypesiege-streempilot-fleet"
    generated_manifest_path = work / "hypesiege-streempilot-manifest.json"

    run([sys.executable, "-m", "py_compile", str(reconstructor)])
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
    expected = {
        "schema_version": 2,
        "generator_sha256": FLEET_GENERATOR_SHA256,
        "repository_count": EXPECTED_REPOSITORIES,
        "total_tracked_files": EXPECTED_FILES,
        "total_gitlinks": EXPECTED_GITLINKS,
        "organizations": EXPECTED_ORGANIZATIONS,
    }
    for key, value in expected.items():
        if generated.get(key) != value:
            fail(f"fleet {key} changed: {generated.get(key)!r} != {value!r}")
    records = generated.get("repositories")
    if not isinstance(records, list) or len(records) != EXPECTED_REPOSITORIES:
        fail("fleet repository ledger is malformed")
    if any(record.get("visibility") != "public" for record in records):
        fail("sealed product fleet must remain public")
    return source_root, generated


def git_environment(work: Path) -> dict[str, str]:
    askpass = work / "git-askpass.sh"
    askpass.write_text(
        "#!/usr/bin/env sh\n"
        "case \"${1:-}\" in\n"
        "  *Username*) printf '%s\\n' 'x-access-token' ;;\n"
        "  *Password*) printf '%s\\n' \"${GITHUB_REPOSITORY_ADMIN_TOKEN:?token required}\" ;;\n"
        "  *) exit 1 ;;\n"
        "esac\n",
        encoding="utf-8",
    )
    askpass.chmod(0o700)
    environment = os.environ.copy()
    environment["GITHUB_REPOSITORY_ADMIN_TOKEN"] = CORE.TOKEN
    environment["GIT_ASKPASS"] = str(askpass)
    environment["GIT_ASKPASS_REQUIRE"] = "force"
    environment["GIT_TERMINAL_PROMPT"] = "0"
    environment["GIT_CONFIG_COUNT"] = "1"
    environment["GIT_CONFIG_KEY_0"] = "credential.helper"
    environment["GIT_CONFIG_VALUE_0"] = ""
    return environment


def local_repository(source_root: Path, full_name: str) -> Path:
    direct = source_root / full_name
    if direct.is_dir():
        return direct
    owner, name = full_name.split("/", 1)
    for candidate in (source_root / owner.lower() / name, source_root / owner / name):
        if candidate.is_dir():
            return candidate
    fail(f"reconstructed repository directory is missing for {full_name}")
    raise AssertionError("unreachable")


def push_exact_main(
    source_root: Path,
    record: dict[str, Any],
    environment: dict[str, str],
) -> None:
    full_name = str(record["full_name"])
    repository = local_repository(source_root, full_name)
    expected = str(record["commit"])
    local_head = run(["git", "-C", str(repository), "rev-parse", "HEAD"]).strip()
    if local_head != expected:
        fail(f"{full_name}: reconstructed HEAD {local_head} != sealed {expected}")

    run(
        [
            "git",
            "-C",
            str(repository),
            "push",
            "--porcelain",
            f"https://github.com/{full_name}.git",
            f"{expected}:refs/heads/main",
        ],
        env=environment,
    )
    observed = CORE.main_ref(full_name)
    if observed != expected:
        fail(f"{full_name}: remote main {observed!r} != sealed {expected}")
    status, _ = CORE.api("PATCH", f"/repos/{full_name}", {"default_branch": "main"})
    if status != 200:
        fail(f"{full_name}: failed to set default branch main: HTTP {status}")
    print(f"PUSHED_EXACT_PUBLIC_MAIN {full_name} {expected}")


def bootstrap_fleet(work: Path) -> dict[str, Any]:
    source_root, manifest = load_reconstructed_fleet(work)
    records = manifest["repositories"]
    missing_or_empty: list[dict[str, Any]] = []
    exact: list[str] = []
    divergent: list[dict[str, str]] = []
    visibility_reconciled: list[str] = []

    for record in records:
        if not isinstance(record, dict):
            fail("fleet contains a non-object record")
        full_name = str(record["full_name"])
        owner, name = full_name.split("/", 1)
        status, before = CORE.api("GET", f"/repos/{full_name}")
        before_visibility = (
            str(before.get("visibility")) if isinstance(before, dict) else "missing"
        )
        current = ensure_repository(
            owner,
            name,
            f"Canonical {full_name} repository bootstrapped from sealed schema-v2 source.",
            "public",
        )
        if before_visibility != "public" and current.get("visibility") == "public":
            visibility_reconciled.append(full_name)

        actual = CORE.main_ref(full_name)
        if actual is None:
            missing_or_empty.append(record)
        elif actual == str(record["commit"]):
            exact.append(full_name)
        else:
            divergent.append(
                {
                    "repository": full_name,
                    "remote_main": actual,
                    "sealed_main": str(record["commit"]),
                }
            )
            print(f"PRESERVED_DIVERGENT {full_name} remote={actual} sealed={record['commit']}")

    # Child repositories must exist before either monorepo is initialized.
    missing_or_empty.sort(
        key=lambda item: (item.get("kind") == "monorepo", str(item["full_name"]))
    )
    environment = git_environment(work)
    created: list[str] = []
    for record in missing_or_empty:
        push_exact_main(source_root, record, environment)
        created.append(str(record["full_name"]))

    missing_after = [
        str(record["full_name"])
        for record in records
        if CORE.main_ref(str(record["full_name"])) is None
    ]
    if missing_after:
        fail(f"canonical fleet still has missing/empty repositories: {missing_after}")

    summary = {
        "created_or_initialized": created,
        "already_exact": exact,
        "preserved_divergent": divergent,
        "visibility_reconciled_to_public": visibility_reconciled,
        "repository_objects_with_main": EXPECTED_REPOSITORIES,
    }
    print(json.dumps({"fleet_bootstrap": summary}, sort_keys=True))
    return summary


def bootstrap_extracted(work: Path) -> dict[str, str]:
    results: dict[str, str] = {}

    status, meta = CORE.api("GET", f"/repos/{META_AGENT}")
    if status == 404:
        results[META_AGENT] = "managed_by_exact_public_publisher"
    elif isinstance(meta, dict):
        reconcile_visibility(META_AGENT, meta, "public")
        results[META_AGENT] = "preserved_public"
    else:
        fail(f"invalid repository metadata for {META_AGENT}")

    owner, name = FILE_TUNNEL.split("/", 1)
    ensure_repository(
        owner,
        name,
        "Private File Tunnel MCP server extracted from the canonical fleet.",
        "private",
    )
    if CORE.main_ref(FILE_TUNNEL) is None:
        original = CORE.ensure_repository
        try:
            CORE.ensure_repository = lambda target_owner, target_name, description: ensure_repository(
                target_owner,
                target_name,
                description,
                "private",
            )
            CORE.publish_file_tunnel_mcp(work)
        finally:
            CORE.ensure_repository = original
        results[FILE_TUNNEL] = "created_or_initialized_private"
    else:
        results[FILE_TUNNEL] = "preserved_private"

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
