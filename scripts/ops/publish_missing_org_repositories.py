#!/usr/bin/env python3
"""Publish the fixed July 31, 2026 missing-repository allowlist.

The caller supplies GH_TOKEN through a protected runtime secret. The token is
never written to a remote URL, source file, manifest, or log. Existing nonempty
main branches are accepted only when they exactly match the prepared commit.
"""

from __future__ import annotations

import base64
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.error
import urllib.request

API = "https://api.github.com"
TOKEN = os.environ.get("GH_TOKEN", "")
if not TOKEN:
    raise SystemExit("GH_TOKEN is required")


def run(args: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> str:
    completed = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if completed.returncode:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(args)}\n{completed.stdout}"
        )
    if completed.stdout:
        print(completed.stdout, end="")
    return completed.stdout


def api(method: str, path: str, body: dict[str, object] | None = None) -> tuple[int, object | None]:
    payload = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(API + path, data=payload, method=method)
    request.add_header("Accept", "application/vnd.github+json")
    request.add_header("Authorization", f"Bearer {TOKEN}")
    request.add_header("X-GitHub-Api-Version", "2022-11-28")
    request.add_header("User-Agent", "bounded-missing-repository-publisher")
    if payload is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            raw = response.read()
            return response.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        raw = error.read(4096)
        if error.code == 404:
            return 404, None
        raise RuntimeError(
            f"GitHub API {error.code} for {method} {path}: {raw.decode(errors='replace')}"
        ) from error


def ensure_repository(owner: str, name: str, description: str) -> dict[str, object]:
    status, current = api("GET", f"/repos/{owner}/{name}")
    if status == 404:
        status, current = api(
            "POST",
            f"/orgs/{owner}/repos",
            {
                "name": name,
                "description": description,
                "private": False,
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
            raise RuntimeError(f"failed to create {owner}/{name}: HTTP {status}")
        print(f"CREATED {owner}/{name}")
    if not isinstance(current, dict):
        raise RuntimeError(f"invalid repository response for {owner}/{name}")
    if current.get("visibility") != "public":
        raise RuntimeError(
            f"visibility mismatch for {owner}/{name}: {current.get('visibility')} != public"
        )
    return current


def main_ref(full_name: str) -> str | None:
    status, payload = api("GET", f"/repos/{full_name}/git/ref/heads/main")
    if status == 404:
        return None
    if not isinstance(payload, dict):
        raise RuntimeError(f"invalid main-ref response for {full_name}")
    object_value = payload.get("object")
    if not isinstance(object_value, dict) or not isinstance(object_value.get("sha"), str):
        raise RuntimeError(f"missing main SHA for {full_name}")
    return str(object_value["sha"])


def push_exact_main(local: Path, full_name: str, expected: str) -> None:
    actual = main_ref(full_name)
    if actual is not None:
        if actual != expected:
            raise RuntimeError(f"refusing to overwrite {full_name}: {actual} != {expected}")
        print(f"VERIFIED {full_name} {actual}")
        return

    askpass = local.parent / f".{local.name}.askpass.sh"
    askpass.write_text(
        '#!/bin/sh\ncase "$1" in *Username*) echo x-access-token;; *) echo "$GH_TOKEN";; esac\n',
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
    try:
        run(
            [
                "git",
                "-C",
                str(local),
                "remote",
                "add",
                "canonical",
                f"https://github.com/{full_name}.git",
            ]
        )
        run(
            [
                "git",
                "-C",
                str(local),
                "push",
                "--set-upstream",
                "canonical",
                "HEAD:refs/heads/main",
            ],
            env=environment,
        )
    finally:
        askpass.unlink(missing_ok=True)

    actual = main_ref(full_name)
    if actual != expected:
        raise RuntimeError(f"remote verification failed for {full_name}: {actual} != {expected}")
    print(f"PUSHED {full_name} {actual}")


def publish_hypesiege_and_streempilot(work: Path) -> None:
    carrier = work / "fleet-carrier"
    run(
        [
            "git",
            "clone",
            "--depth",
            "1",
            "--branch",
            "agent/hypesiege-streempilot-live-publish-20260731",
            "https://github.com/ORESoftware/ai-agent-coordinator.rs.git",
            str(carrier),
        ]
    )
    chunks = sorted(
        (carrier / "repository-fleets/hypesiege-streempilot").glob(
            "generator.py.gz.b64.part-*"
        )
    )
    if not chunks:
        raise RuntimeError("sealed HypeSiege/StreemPilot generator chunks are missing")
    encoded = b"".join(chunk.read_bytes() for chunk in chunks)
    generator = gzip.decompress(base64.b64decode(encoded))
    digest = hashlib.sha256(generator).hexdigest()
    expected_digest = "50629a57beca1ac85928cfae8fbebbca4f62a6455a7013016f92b1203dcbbd1f"
    if digest != expected_digest:
        raise RuntimeError(f"generator digest mismatch: {digest} != {expected_digest}")
    generator_path = work / "generate_hypesiege_streempilot.py"
    generator_path.write_bytes(generator)
    run(["python3", "-m", "py_compile", str(generator_path)])
    run(["python3", str(generator_path)])

    root = Path("/mnt/data/hypesiege-streempilot-fleet")
    manifest = json.loads((root / "MANIFEST.json").read_text(encoding="utf-8"))
    if manifest.get("repository_count") != 32:
        raise RuntimeError("generated fleet does not contain exactly 32 repositories")

    publisher = root / "publish.py"
    lines = publisher.read_text(encoding="utf-8").splitlines()
    broken = [
        "        askpass.write_text('#!/bin/sh",
        'case "$1" in *Username*) echo x-access-token;; *) echo "$GITHUB_REPOSITORY_ADMIN_TOKEN";; esac',
        "')",
    ]
    if lines[70:73] == broken:
        lines[70:73] = [
            "        askpass.write_text(",
            "            '#!/bin/sh\\ncase \"$1\" in *Username*) echo x-access-token;; *) echo \"$GITHUB_REPOSITORY_ADMIN_TOKEN\";; esac\\n'",
            "        )",
        ]
        publisher.write_text("\n".join(lines) + "\n", encoding="utf-8")
    run(["python3", "-m", "py_compile", str(publisher)])

    environment = os.environ.copy()
    environment["GITHUB_REPOSITORY_ADMIN_TOKEN"] = TOKEN
    run(["python3", str(publisher), "--execute", "--org", "hypesiege"], env=environment)
    run(["python3", str(publisher), "--execute", "--org", "streempilot"], env=environment)

    for record in manifest["repositories"]:
        full_name = str(record["full_name"])
        expected = str(record["commit"])
        actual = main_ref(full_name)
        if actual != expected:
            raise RuntimeError(f"fleet verification failed for {full_name}: {actual} != {expected}")
        print(f"VERIFIED {full_name} {actual}")
    print("VERIFIED 32/32 HypeSiege and StreemPilot repositories")


def initialize_clean_history(root: Path, message: str, timestamp: str) -> str:
    run(["git", "init", "-b", "main"], cwd=root)
    run(["git", "config", "user.name", "ORESoftware publication automation"], cwd=root)
    run(["git", "config", "user.email", "bot@oresoftware.dev"], cwd=root)
    run(["git", "add", "-A"], cwd=root)
    environment = os.environ.copy()
    environment["GIT_AUTHOR_DATE"] = timestamp
    environment["GIT_COMMITTER_DATE"] = timestamp
    run(["git", "commit", "-m", message], cwd=root, env=environment)
    return run(["git", "rev-parse", "HEAD"], cwd=root).strip()


def publish_meta_agents(work: Path) -> None:
    carrier = work / "meta-carrier"
    target = work / "meta-agent-control-plane.rs"
    run(
        [
            "git",
            "clone",
            "--depth",
            "1",
            "--branch",
            "incubator/meta-agent-control-plane",
            "https://github.com/ORESoftware/testing.git",
            str(carrier),
        ]
    )
    shutil.copytree(carrier, target, ignore=shutil.ignore_patterns(".git"))
    (target / ".github/workflows/materialize-meta-agent.yml").unlink(missing_ok=True)
    ci = target / ".github/workflows/ci.yml"
    if not ci.is_file():
        raise RuntimeError("materialized Meta Agents CI workflow is missing")
    ci.rename(target / ".meta-agent-ci.yml.pending")
    if (target / ".bootstrap").exists():
        raise RuntimeError("Meta Agents bootstrap archive leaked into canonical source")
    run(["python3", "scripts/verify_contract.py"], cwd=target)
    sha = initialize_clean_history(
        target,
        "feat: publish meta-agent control plane",
        "2026-07-31T06:15:00Z",
    )
    ensure_repository(
        "meta-agents-demo",
        "meta-agent-control-plane.rs",
        "Single-binary Rust meta-agent daemon and Leptos observability UI",
    )
    push_exact_main(target, "meta-agents-demo/meta-agent-control-plane.rs", sha)


def file_tunnel_ci() -> str:
    return """name: File Tunnel MCP server

on:
  pull_request:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  rust:
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - name: Check out repository
        uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0
        with:
          persist-credentials: false
      - name: Install pinned Rust toolchain
        run: rustup toolchain install 1.85.0 --profile minimal --component rustfmt,clippy
      - name: Format
        run: cargo +1.85.0 fmt --all -- --check
      - name: Test
        run: cargo +1.85.0 test --all-targets
      - name: Clippy
        run: cargo +1.85.0 clippy --all-targets -- -D warnings
      - name: Release build
        run: cargo +1.85.0 build --release
"""


def publish_file_tunnel_mcp(work: Path) -> None:
    source = work / "ftnl-monorepo"
    target = work / "ftnl-mcp-server.rs"
    run(
        [
            "git",
            "clone",
            "--depth",
            "1",
            "--branch",
            "main",
            "https://github.com/file-tunnel/ftnl-monorepo.git",
            str(source),
        ]
    )
    shutil.copytree(source / "apps/mcp-server", target)
    license_path = source / "LICENSE"
    if license_path.is_file():
        shutil.copy2(license_path, target / "LICENSE")

    cargo = target / "Cargo.toml"
    cargo_text = cargo.read_text(encoding="utf-8")
    if "repository = " not in cargo_text:
        cargo_text = cargo_text.replace(
            'license = "MIT"\n',
            'license = "MIT"\nrepository = "https://github.com/file-tunnel/ftnl-mcp-server.rs"\n',
            1,
        )
    cargo.write_text(cargo_text, encoding="utf-8")

    readme = target / "README.md"
    readme_text = readme.read_text(encoding="utf-8")
    readme_text = readme_text.replace(
        "This is the canonical implementation incubator for `file-tunnel/ftnl-mcp-server.rs` while the standalone repository is not yet available through the connected repository-creation surface. The code is kept in the File Tunnel organization—not in a ZIP—and is ready to split into the dedicated repository without changing its package boundary.",
        "This is the canonical standalone implementation for `file-tunnel/ftnl-mcp-server.rs`. The File Tunnel monorepo consumes reviewed releases or a pinned Git submodule; domain tools and safety policy remain local to this repository.",
    ).replace("cargo run --manifest-path apps/mcp-server/Cargo.toml", "cargo run")
    if "## Next extraction step" in readme_text:
        readme_text = readme_text.split("## Next extraction step", 1)[0].rstrip()
        readme_text += (
            "\n\n## Monorepo integration\n\n"
            "Pin the reviewed standalone commit from `file-tunnel/ftnl-monorepo`; "
            "do not copy the crate back into the monorepo.\n"
        )
    readme.write_text(readme_text, encoding="utf-8")
    (target / ".ftnl-mcp-ci.yml.pending").write_text(file_tunnel_ci(), encoding="utf-8")

    sha = initialize_clean_history(
        target,
        "feat: publish File Tunnel MCP server",
        "2026-07-31T06:16:00Z",
    )
    ensure_repository(
        "file-tunnel",
        "ftnl-mcp-server.rs",
        "Fail-closed Rust MCP server for File Tunnel planning and validation",
    )
    push_exact_main(target, "file-tunnel/ftnl-mcp-server.rs", sha)


def main() -> None:
    work = Path(tempfile.mkdtemp(prefix="missing-repository-publication-"))
    try:
        publish_hypesiege_and_streempilot(work)
        publish_meta_agents(work)
        publish_file_tunnel_mcp(work)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    print("PASS published and verified all 34 previously missing repositories")


if __name__ == "__main__":
    main()
