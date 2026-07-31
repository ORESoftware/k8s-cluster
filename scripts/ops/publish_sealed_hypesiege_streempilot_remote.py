#!/usr/bin/env python3
"""Publish and verify the sealed HypeSiege/StreemPilot fleet on the SSM host.

The protected GitHub credential is read as one base64 line from stdin. It is
never accepted as a command-line argument, written into a Git URL, or included
in the emitted report. Successful execution prints exactly one bounded
`PUBLICATION_REPORT_BASE64=` record for the trusted GitHub Actions caller.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from typing import Any
from urllib.request import Request, urlopen

EXPECTED_REPOSITORIES = 32
EXPECTED_FILES = 888
EXPECTED_GITLINKS = 30
EXPECTED_ORGANIZATIONS = {"hypesiege": 15, "streempilot": 17}


class PublicationError(RuntimeError):
    """A bounded publication stage failed."""


def run(
    args: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    stage: str,
) -> str:
    completed = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()[-4096:]
        raise PublicationError(f"{stage} failed with exit {completed.returncode}: {detail}")
    return completed.stdout


def read_credential() -> str:
    encoded = sys.stdin.readline().strip()
    if not encoded:
        raise PublicationError("protected credential input is empty")
    try:
        raw = base64.b64decode(encoded, validate=True)
        token = raw.decode("utf-8")
    except (ValueError, UnicodeDecodeError) as error:
        raise PublicationError("protected credential input is not valid base64 UTF-8") from error
    if not token or "\n" in token or "\r" in token:
        raise PublicationError("protected credential must be one non-empty line")
    return token


def github_get(token: str, path: str) -> dict[str, Any]:
    request = Request(
        "https://api.github.com" + path,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "sealed-hypesiege-streempilot-ssm-publisher",
        },
    )
    with urlopen(request, timeout=30) as response:
        value = json.load(response)
    if not isinstance(value, dict):
        raise PublicationError(f"GitHub returned a non-object for {path}")
    return value


def verify_identity(token: str) -> None:
    identity = github_get(token, "/user")
    if identity.get("login") != "ORESoftware":
        raise PublicationError(
            f"unexpected publisher identity: {identity.get('login')!r}"
        )
    for organization in ("hypesiege", "StreemPilot"):
        membership = github_get(token, f"/user/memberships/orgs/{organization}")
        observed = (membership.get("role"), membership.get("state"))
        if observed != ("admin", "active"):
            raise PublicationError(
                f"{organization} owner membership is {observed!r}"
            )


def load_manifest(path: Path, generator_sha256: str) -> dict[str, Any]:
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 2:
        raise PublicationError("fleet manifest schema version is not 2")
    if manifest.get("generator_sha256") != generator_sha256:
        raise PublicationError("fleet generator identity drifted")
    if manifest.get("repository_count") != EXPECTED_REPOSITORIES:
        raise PublicationError("fleet repository count drifted")
    if manifest.get("total_tracked_files") != EXPECTED_FILES:
        raise PublicationError("fleet tracked-file count drifted")
    if manifest.get("total_gitlinks") != EXPECTED_GITLINKS:
        raise PublicationError("fleet gitlink count drifted")
    if manifest.get("organizations") != EXPECTED_ORGANIZATIONS:
        raise PublicationError("fleet organization counts drifted")
    records = manifest.get("repositories")
    if not isinstance(records, list) or len(records) != EXPECTED_REPOSITORIES:
        raise PublicationError("fleet repository ledger is malformed")
    if any(record.get("visibility") != "public" for record in records):
        raise PublicationError("sealed fleet visibility contract is not public")
    return manifest


def configure_git_auth(work: Path, token: str, environment: dict[str, str]) -> None:
    askpass = work / "git-askpass.sh"
    askpass.write_text(
        "#!/usr/bin/env bash\n"
        "case \"${1:-}\" in\n"
        "  *Username*) printf '%s\\n' x-access-token ;;\n"
        "  *Password*) printf '%s\\n' \"${GH_TOKEN:?}\" ;;\n"
        "  *) exit 1 ;;\n"
        "esac\n",
        encoding="utf-8",
    )
    askpass.chmod(0o700)
    environment.update(
        {
            "GH_TOKEN": token,
            "GITHUB_REPOSITORY_ADMIN_TOKEN": token,
            "GIT_ASKPASS": str(askpass),
            "GIT_ASKPASS_REQUIRE": "force",
            "GIT_TERMINAL_PROMPT": "0",
        }
    )


def publish(args: argparse.Namespace, token: str) -> dict[str, Any]:
    verify_identity(token)
    work = Path(tempfile.mkdtemp(prefix="sealed-fleet-publisher."))
    environment = os.environ.copy()
    environment["CODEX_HOME"] = str(work / "codex-home")
    Path(environment["CODEX_HOME"]).mkdir(mode=0o700)
    configure_git_auth(work, token, environment)

    try:
        coordinator = work / "ai-agent-coordinator"
        run(["git", "init", str(coordinator)], stage="initialize coordinator checkout")
        run(
            [
                "git",
                "-C",
                str(coordinator),
                "remote",
                "add",
                "origin",
                "https://github.com/ORESoftware/ai-agent-coordinator.rs.git",
            ],
            stage="configure coordinator origin",
        )
        run(
            [
                "git",
                "-C",
                str(coordinator),
                "fetch",
                "--depth=1",
                "origin",
                args.coordinator_sha,
            ],
            env=environment,
            stage="fetch reviewed coordinator commit",
        )
        run(
            ["git", "-C", str(coordinator), "checkout", "--detach", "FETCH_HEAD"],
            stage="check out reviewed coordinator commit",
        )
        observed_sha = run(
            ["git", "-C", str(coordinator), "rev-parse", "HEAD"],
            stage="read coordinator commit",
        ).strip()
        if observed_sha != args.coordinator_sha:
            raise PublicationError("coordinator checkout identity drifted")

        scripts = [
            "scripts/reconstruct_hypesiege_streempilot_fleet.py",
            "scripts/publish_hypesiege_streempilot_fleet.py",
            "scripts/test_publish_hypesiege_streempilot_fleet.py",
        ]
        run(
            ["python3", "-m", "py_compile", *scripts],
            cwd=coordinator,
            stage="compile reviewed coordinator scripts",
        )
        run(
            [
                "python3",
                "-m",
                "unittest",
                "-v",
                "scripts/test_publish_hypesiege_streempilot_fleet.py",
            ],
            cwd=coordinator,
            env=environment,
            stage="run reviewed fleet tests",
        )

        source_root = work / "hypesiege-streempilot-fleet"
        generated_manifest = work / "hypesiege-streempilot-manifest.json"
        run(
            [
                "python3",
                "scripts/reconstruct_hypesiege_streempilot_fleet.py",
                "--payload-dir",
                "repository-fleets/hypesiege-streempilot",
                "--output-root",
                str(source_root),
                "--manifest-out",
                str(generated_manifest),
            ],
            cwd=coordinator,
            env=environment,
            stage="reconstruct deterministic fleet",
        )
        reviewed_manifest = load_manifest(
            coordinator / "repository-fleets/hypesiege-streempilot.json",
            args.generator_sha256,
        )
        generated = load_manifest(generated_manifest, args.generator_sha256)
        if generated != reviewed_manifest:
            raise PublicationError("reconstructed fleet differs from reviewed ledger")

        publisher = coordinator / "scripts/publish_hypesiege_streempilot_fleet.py"
        for record in generated["repositories"]:
            repository = record["full_name"]
            run(
                [
                    "python3",
                    str(publisher),
                    "--manifest",
                    str(generated_manifest),
                    "--source-root",
                    str(source_root),
                    "--repository",
                    repository,
                    "--execute",
                    "--confirm-repository",
                    repository,
                ],
                env=environment,
                stage=f"publish {repository}",
            )

        repositories: list[dict[str, Any]] = []
        counts = {"hypesiege": 0, "streempilot": 0}
        for record in generated["repositories"]:
            metadata = github_get(token, f"/repos/{record['full_name']}")
            reference = github_get(
                token, f"/repos/{record['full_name']}/git/ref/heads/main"
            )
            observed = reference.get("object", {}).get("sha")
            if metadata.get("full_name", "").casefold() != record["full_name"].casefold():
                raise PublicationError(f"{record['full_name']} repository identity drifted")
            if metadata.get("default_branch") != "main":
                raise PublicationError(f"{record['full_name']} default branch drifted")
            if metadata.get("visibility") != record["visibility"]:
                raise PublicationError(f"{record['full_name']} visibility drifted")
            if observed != record["commit"]:
                raise PublicationError(f"{record['full_name']} remote main drifted")
            counts[record["org"]] += 1
            repositories.append(
                {
                    "repository": metadata["full_name"],
                    "repository_id": metadata["id"],
                    "node_id": metadata["node_id"],
                    "visibility": metadata["visibility"],
                    "default_branch": metadata["default_branch"],
                    "main_sha": observed,
                    "expected_sha": record["commit"],
                    "html_url": metadata["html_url"],
                }
            )
        if counts != EXPECTED_ORGANIZATIONS:
            raise PublicationError("verified remote organization counts drifted")

        return {
            "success": True,
            "verified_at": datetime.now(timezone.utc).isoformat(),
            "coordinator_sha": args.coordinator_sha,
            "generator_sha256": args.generator_sha256,
            "repository_count": len(repositories),
            "organization_counts": counts,
            "total_tracked_files": generated["total_tracked_files"],
            "total_gitlinks": generated["total_gitlinks"],
            "repositories": repositories,
        }
    finally:
        environment.pop("GH_TOKEN", None)
        environment.pop("GITHUB_REPOSITORY_ADMIN_TOKEN", None)
        environment.pop("GIT_ASKPASS", None)
        shutil.rmtree(work, ignore_errors=True)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--coordinator-sha", required=True)
    parser.add_argument("--generator-sha256", required=True)
    args = parser.parse_args()
    for name, value, length in (
        ("coordinator SHA", args.coordinator_sha, 40),
        ("generator SHA-256", args.generator_sha256, 64),
    ):
        if len(value) != length or any(ch not in "0123456789abcdef" for ch in value):
            parser.error(f"{name} must be {length} lowercase hexadecimal characters")
    return args


def main() -> int:
    args = parse_args()
    token = read_credential()
    try:
        report = publish(args, token)
    finally:
        token = ""
        os.environ.pop("GH_TOKEN", None)
        os.environ.pop("GITHUB_REPOSITORY_ADMIN_TOKEN", None)
    compact = json.dumps(report, separators=(",", ":"), sort_keys=True).encode("utf-8")
    print("PUBLICATION_REPORT_BASE64=" + base64.b64encode(compact).decode("ascii"))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PublicationError as error:
        raise SystemExit(f"sealed fleet publication refused: {error}") from error
