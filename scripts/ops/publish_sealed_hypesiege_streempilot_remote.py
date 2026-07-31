#!/usr/bin/env python3
"""Publish the reviewed HypeSiege/StreemPilot fleet from the protected cluster host.

The GitHub owner credential is read only from the Kubernetes Secret already
materialized by External Secrets on the protected SSM host. Credential values
never cross the SSM boundary or enter Git URLs, repository files, or reports.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import json
import os
import pathlib
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from typing import Any, Iterable

API_BASE = "https://api.github.com"
API_VERSION = "2022-11-28"
USER_AGENT = "k8s-cluster-sealed-hypesiege-streempilot-publisher"
EXPECTED_REPOSITORIES = 32
EXPECTED_FILES = 888
EXPECTED_GITLINKS = 30
EXPECTED_COUNTS = {"hypesiege": 15, "streempilot": 17}
TARGET_ORGANIZATIONS = ("hypesiege", "StreemPilot")
EVIDENCE_REPOSITORY = "ORESoftware/k8s-cluster"
EVIDENCE_MARKDOWN = "docs/ops/hypesiege-streempilot-publication-2026-07-31.md"
EVIDENCE_JSON = "docs/ops/hypesiege-streempilot-publication-2026-07-31.json"


class PublicationError(RuntimeError):
    """Fail-closed publication error."""


def log(message: str) -> None:
    print(message, flush=True)


def run(
    args: list[str],
    *,
    cwd: pathlib.Path | None = None,
    env: dict[str, str] | None = None,
    stdin: str | None = None,
) -> str:
    completed = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        input=stdin,
        text=True,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()[:4096]
        location = f" in {cwd}" if cwd else ""
        raise PublicationError(f"{' '.join(args)} failed{location}: {detail}")
    return completed.stdout


def require_sha(value: str, label: str, length: int = 40) -> str:
    if not re.fullmatch(rf"[0-9a-f]{{{length}}}", value):
        raise PublicationError(f"{label} must be {length} lowercase hexadecimal characters")
    return value


def kubectl_command() -> list[str]:
    kubectl = shutil.which("kubectl")
    if kubectl:
        return [kubectl]
    k3s = shutil.which("k3s")
    if k3s:
        return [k3s, "kubectl"]
    fallback = pathlib.Path("/usr/local/bin/kubectl")
    if fallback.is_file() and os.access(fallback, os.X_OK):
        return [str(fallback)]
    raise PublicationError("kubectl is unavailable in the protected ec2-user login shell")


def load_repository_admin_token() -> str:
    log("stage=cluster-credential")
    command = kubectl_command()
    log(f"kubectl_command={' '.join(command)}")
    context = run([*command, "config", "current-context"]).strip()
    if not context:
        raise PublicationError("Kubernetes current context is empty")
    log("kubernetes_context=available")

    raw = run(
        [
            *command,
            "-n",
            "default",
            "get",
            "secret",
            "dd-agent-secrets",
            "-o",
            "json",
        ]
    )
    payload = json.loads(raw)
    data = payload.get("data")
    if not isinstance(data, dict):
        raise PublicationError("default/dd-agent-secrets has no data map")
    keys = sorted(str(key) for key in data)
    log(f"secret_keys={','.join(keys) if keys else 'none'}")

    encoded = data.get("GH_PAT")
    if not isinstance(encoded, str) or not encoded:
        raise PublicationError(
            "default/dd-agent-secrets does not contain a non-empty GH_PAT key"
        )
    try:
        token = base64.b64decode(encoded, validate=True).decode("utf-8")
    except (binascii.Error, UnicodeDecodeError) as error:
        raise PublicationError("default/dd-agent-secrets GH_PAT is not valid base64 text") from error
    if not token.strip():
        raise PublicationError("decoded GH_PAT is empty")
    return token.strip()


def request_json(
    method: str,
    path: str,
    token: str,
    body: dict[str, Any] | None = None,
    *,
    allow_404: bool = False,
) -> dict[str, Any] | None:
    data = None if body is None else json.dumps(body, separators=(",", ":")).encode()
    request = urllib.request.Request(
        API_BASE + path,
        data=data,
        method=method,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": USER_AGENT,
        },
    )
    if data is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            raw = response.read(512 * 1024 + 1)
            if len(raw) > 512 * 1024:
                raise PublicationError(f"GitHub response exceeded 512 KiB for {path}")
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as error:
        if allow_404 and error.code == 404:
            return None
        detail = error.read(4096).decode(errors="replace")
        raise PublicationError(
            f"GitHub API {error.code} for {method} {path}: {detail}"
        ) from error
    except urllib.error.URLError as error:
        raise PublicationError(f"GitHub API unavailable for {method} {path}: {error}") from error


def verify_publisher_identity(token: str) -> None:
    log("stage=publisher-identity")
    identity = request_json("GET", "/user", token)
    if not isinstance(identity, dict) or identity.get("login") != "ORESoftware":
        observed = identity.get("login") if isinstance(identity, dict) else None
        raise PublicationError(f"unexpected publisher identity: {observed!r}")
    log("publisher_identity=ORESoftware")

    for organization in TARGET_ORGANIZATIONS:
        membership = request_json(
            "GET", f"/user/memberships/orgs/{organization}", token
        )
        observed = (
            membership.get("role") if isinstance(membership, dict) else None,
            membership.get("state") if isinstance(membership, dict) else None,
        )
        if observed != ("admin", "active"):
            raise PublicationError(
                f"{organization} owner membership is {observed!r}, expected ('admin', 'active')"
            )
        log(f"{organization}_owner_membership=verified")


def write_askpass(directory: pathlib.Path) -> pathlib.Path:
    askpass = directory / "github-askpass.sh"
    askpass.write_text(
        "#!/bin/sh\n"
        'case "$1" in\n'
        '  *Username*) printf "%s\\n" x-access-token ;;\n'
        '  *) printf "%s\\n" "$GH_TOKEN" ;;\n'
        "esac\n",
        encoding="utf-8",
    )
    askpass.chmod(stat.S_IRWXU)
    return askpass


def clone_exact(
    url: str,
    sha: str,
    destination: pathlib.Path,
    environment: dict[str, str],
) -> None:
    destination.mkdir(parents=True, exist_ok=False)
    run(["git", "init", "-q"], cwd=destination, env=environment)
    run(["git", "remote", "add", "origin", url], cwd=destination, env=environment)
    run(
        ["git", "fetch", "-q", "--depth=1", "origin", sha],
        cwd=destination,
        env=environment,
    )
    run(["git", "checkout", "-q", "--detach", "FETCH_HEAD"], cwd=destination, env=environment)
    observed = run(["git", "rev-parse", "HEAD"], cwd=destination, env=environment).strip()
    if observed != sha:
        raise PublicationError(f"exact clone mismatch: {observed} != {sha}")


def validate_manifest(
    coordinator: pathlib.Path,
    generated_manifest: pathlib.Path,
    expected_generator_sha256: str,
) -> dict[str, Any]:
    checked = json.loads(
        (coordinator / "repository-fleets/hypesiege-streempilot.json").read_text()
    )
    generated = json.loads(generated_manifest.read_text())
    if generated != checked:
        raise PublicationError("reconstructed fleet differs from the reviewed ledger")
    if generated.get("schema_version") != 2:
        raise PublicationError("reviewed fleet schema version changed")
    if generated.get("generator_sha256") != expected_generator_sha256:
        raise PublicationError("reviewed fleet generator checksum changed")
    if generated.get("repository_count") != EXPECTED_REPOSITORIES:
        raise PublicationError("reviewed fleet repository count changed")
    if generated.get("total_tracked_files") != EXPECTED_FILES:
        raise PublicationError("reviewed fleet tracked-file count changed")
    if generated.get("total_gitlinks") != EXPECTED_GITLINKS:
        raise PublicationError("reviewed fleet gitlink count changed")
    if generated.get("organizations") != EXPECTED_COUNTS:
        raise PublicationError("reviewed fleet organization counts changed")

    seen = {"hypesiege": 0, "streempilot": 0}
    repositories = generated.get("repositories")
    if not isinstance(repositories, list) or len(repositories) != EXPECTED_REPOSITORIES:
        raise PublicationError("reviewed fleet repository ledger is malformed")
    for record in repositories:
        org = record.get("org")
        if org not in seen:
            raise PublicationError(f"unexpected organization in ledger: {org!r}")
        if record.get("kind") == "monorepo":
            expected = 14 if org == "hypesiege" else 16
            if seen[org] != expected:
                raise PublicationError(
                    f"{record.get('full_name')} precedes one or more child repositories"
                )
        else:
            seen[org] += 1
    if seen != {"hypesiege": 14, "streempilot": 16}:
        raise PublicationError(f"child repository counts changed: {seen}")
    log("reviewed_fleet=32_repositories_validated")
    return generated


def publish_repositories(
    coordinator: pathlib.Path,
    manifest_path: pathlib.Path,
    source_root: pathlib.Path,
    records: Iterable[dict[str, Any]],
    environment: dict[str, str],
) -> None:
    log("stage=publication")
    publisher = coordinator / "scripts/publish_hypesiege_streempilot_fleet.py"
    published = 0
    for record in records:
        full_name = record.get("full_name")
        if not isinstance(full_name, str) or not full_name:
            raise PublicationError("repository ledger contains an invalid full_name")
        log(f"publishing={full_name}")
        run(
            [
                sys.executable,
                str(publisher),
                "--manifest",
                str(manifest_path),
                "--source-root",
                str(source_root),
                "--repository",
                full_name,
                "--execute",
                "--confirm-repository",
                full_name,
            ],
            cwd=coordinator,
            env=environment,
        )
        published += 1
    if published != EXPECTED_REPOSITORIES:
        raise PublicationError(f"published {published}, expected {EXPECTED_REPOSITORIES}")


def verify_remotes(
    manifest: dict[str, Any], token: str, trusted_sha: str, coordinator_sha: str
) -> dict[str, Any]:
    log("stage=remote-verification")
    repositories: list[dict[str, Any]] = []
    counts = {"hypesiege": 0, "streempilot": 0}
    for record in manifest["repositories"]:
        full_name = record["full_name"]
        metadata = request_json("GET", f"/repos/{full_name}", token)
        ref = request_json("GET", f"/repos/{full_name}/git/ref/heads/main", token)
        if not isinstance(metadata, dict) or not isinstance(ref, dict):
            raise PublicationError(f"GitHub metadata missing for {full_name}")
        observed = ref.get("object", {}).get("sha")
        if metadata.get("full_name", "").casefold() != full_name.casefold():
            raise PublicationError(f"GitHub returned the wrong repository for {full_name}")
        if metadata.get("default_branch") != "main":
            raise PublicationError(f"{full_name} default branch is not main")
        if metadata.get("visibility") != record["visibility"]:
            raise PublicationError(f"{full_name} visibility differs from the ledger")
        if observed != record["commit"]:
            raise PublicationError(
                f"{full_name} remote main is {observed!r}, expected {record['commit']}"
            )
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
    if counts != EXPECTED_COUNTS:
        raise PublicationError(f"remote organization counts changed: {counts}")
    return {
        "success": True,
        "verified_at": datetime.now(timezone.utc).isoformat(),
        "k8s_cluster_sha": trusted_sha,
        "coordinator_sha": coordinator_sha,
        "generator_sha256": manifest["generator_sha256"],
        "repository_count": len(repositories),
        "organization_counts": counts,
        "total_tracked_files": manifest["total_tracked_files"],
        "total_gitlinks": manifest["total_gitlinks"],
        "repositories": repositories,
    }


def markdown_report(report: dict[str, Any]) -> str:
    counts = report["organization_counts"]
    lines = [
        "# HypeSiege and StreemPilot repository publication",
        "",
        "Overall result: **SUCCESS**",
        "",
        f"- Trusted k8s-cluster commit: `{report['k8s_cluster_sha']}`",
        f"- Coordinator product commit: `{report['coordinator_sha']}`",
        f"- Repositories verified: `{report['repository_count']}/32`",
        f"- HypeSiege: `{counts['hypesiege']}/15`",
        f"- StreemPilot: `{counts['streempilot']}/17`",
        f"- Generator SHA-256: `{report['generator_sha256']}`",
        "",
    ]
    for item in report["repositories"]:
        lines.append(
            f"- `{item['repository']}` — `{item['default_branch']}` "
            f"`{item['main_sha']}`; {item['visibility']}; ID `{item['repository_id']}`"
        )
    return "\n".join(lines) + "\n"


def persist_evidence(path: str, text: str, message: str, token: str) -> None:
    encoded_path = urllib.parse.quote(path, safe="/")
    current = request_json(
        "GET",
        f"/repos/{EVIDENCE_REPOSITORY}/contents/{encoded_path}?ref=main",
        token,
        allow_404=True,
    )
    body: dict[str, Any] = {
        "message": message,
        "content": base64.b64encode(text.encode()).decode(),
        "branch": "main",
    }
    if isinstance(current, dict):
        body["sha"] = current["sha"]
    request_json(
        "PUT",
        f"/repos/{EVIDENCE_REPOSITORY}/contents/{encoded_path}",
        token,
        body,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--trusted-sha", required=True)
    parser.add_argument("--coordinator-sha", required=True)
    parser.add_argument("--expected-generator-sha256", required=True)
    parser.add_argument("--work-root", type=pathlib.Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    trusted_sha = require_sha(args.trusted_sha, "trusted SHA")
    coordinator_sha = require_sha(args.coordinator_sha, "coordinator SHA")
    expected_generator = require_sha(
        args.expected_generator_sha256, "generator SHA-256", length=64
    )

    work_root = args.work_root or pathlib.Path(
        tempfile.mkdtemp(prefix="sealed-hypesiege-streempilot-")
    )
    work_root.mkdir(parents=True, exist_ok=True)
    work_root.chmod(stat.S_IRWXU)

    token = load_repository_admin_token()
    verify_publisher_identity(token)

    environment = os.environ.copy()
    environment.pop("GITHUB_TOKEN", None)
    environment.pop("CODEX_HOME", None)
    environment["GH_TOKEN"] = token
    environment["GITHUB_REPOSITORY_ADMIN_TOKEN"] = token
    environment["GIT_TERMINAL_PROMPT"] = "0"
    environment["GIT_ASKPASS_REQUIRE"] = "force"
    environment["GIT_ASKPASS"] = str(write_askpass(work_root))

    log("stage=reviewed-source")
    coordinator = work_root / "ai-agent-coordinator"
    clone_exact(
        "https://github.com/ORESoftware/ai-agent-coordinator.rs.git",
        coordinator_sha,
        coordinator,
        environment,
    )

    source_root = work_root / "hypesiege-streempilot-fleet"
    manifest_path = work_root / "hypesiege-streempilot-manifest.json"
    run(
        [
            sys.executable,
            "-m",
            "py_compile",
            "scripts/reconstruct_hypesiege_streempilot_fleet.py",
            "scripts/publish_hypesiege_streempilot_fleet.py",
            "scripts/test_publish_hypesiege_streempilot_fleet.py",
        ],
        cwd=coordinator,
        env=environment,
    )
    run(
        [
            sys.executable,
            "-m",
            "unittest",
            "-v",
            "scripts/test_publish_hypesiege_streempilot_fleet.py",
        ],
        cwd=coordinator,
        env=environment,
    )
    run(
        [
            sys.executable,
            "scripts/reconstruct_hypesiege_streempilot_fleet.py",
            "--payload-dir",
            "repository-fleets/hypesiege-streempilot",
            "--output-root",
            str(source_root),
            "--manifest-out",
            str(manifest_path),
        ],
        cwd=coordinator,
        env=environment,
    )
    manifest = validate_manifest(coordinator, manifest_path, expected_generator)

    publish_repositories(
        coordinator,
        manifest_path,
        source_root,
        manifest["repositories"],
        environment,
    )
    report = verify_remotes(manifest, token, trusted_sha, coordinator_sha)
    report_json = json.dumps(report, indent=2, sort_keys=True) + "\n"
    report_markdown = markdown_report(report)

    persist_evidence(
        EVIDENCE_MARKDOWN,
        report_markdown,
        "docs: record verified HypeSiege and StreemPilot publication",
        token,
    )
    persist_evidence(
        EVIDENCE_JSON,
        report_json,
        "docs: record machine-readable HypeSiege and StreemPilot inventory",
        token,
    )

    log("publication_result=success")
    log("repositories_verified=32/32")
    log("hypesiege_verified=15/15")
    log("streempilot_verified=17/17")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PublicationError as error:
        print(f"publication_error={error}", file=sys.stderr, flush=True)
        raise SystemExit(1)
