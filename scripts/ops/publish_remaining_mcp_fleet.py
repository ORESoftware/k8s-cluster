#!/usr/bin/env python3
"""Protected publication of the five remaining organization MCP servers.

The caller supplies GH_TOKEN only through the protected SSM runtime. This script
creates repositories with reviewed visibility, bootstraps product changes on
feature branches, requires push-triggered GitHub Actions to pass, merges exact
heads, and then wires exact gitlinks through separate monorepo pull requests.
"""
from __future__ import annotations

import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.parse
from pathlib import Path
from typing import Any

from remaining_mcp_fleet import (
    MONOREPO_SPECS,
    MSRV,
    RMCP_VERSION,
    RUST_VERSION,
    SERVER_SPECS,
    SHARED_REVISION,
    MonorepoSpec,
    RepositorySpec,
    template_digest,
    validate_request_manifest,
    write_server_tree,
)

CORE_PATH = Path(__file__).with_name("publish_missing_org_repositories.py")
SPEC = importlib.util.spec_from_file_location("remaining_mcp_core", CORE_PATH)
if SPEC is None or SPEC.loader is None:
    raise SystemExit(f"unable to load {CORE_PATH}")
CORE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CORE
SPEC.loader.exec_module(CORE)

MAX_COMMAND_OUTPUT = 64 * 1024
CI_TIMEOUT_SECONDS = 45 * 60
POLL_SECONDS = 10


def fail(message: str) -> None:
    raise RuntimeError(message)


def run(
    args: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    timeout: int = 1800,
) -> str:
    completed = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=timeout,
        check=False,
    )
    output = completed.stdout or ""
    if completed.returncode:
        bounded = output[-MAX_COMMAND_OUTPUT:]
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(args)}\n{bounded}"
        )
    return output


def api(method: str, path: str, body: dict[str, object] | None = None) -> tuple[int, object | None]:
    return CORE.api(method, path, body)


def api_expect(method: str, path: str, expected: set[int], body: dict[str, object] | None = None) -> object | None:
    status, payload = api(method, path, body)
    if status not in expected:
        fail(f"GitHub API {method} {path} returned HTTP {status}, expected {sorted(expected)}")
    return payload


def git_environment(work: Path) -> dict[str, str]:
    token = os.environ.get("GH_TOKEN", "")
    if not token or any(character.isspace() for character in token):
        fail("GH_TOKEN must be a non-empty whitespace-free protected runtime secret")
    askpass = work / "git-askpass.sh"
    askpass.write_text(
        "#!/usr/bin/env sh\n"
        "case \"${1:-}\" in\n"
        "  *Username*) printf '%s\\n' 'x-access-token' ;;\n"
        "  *Password*) printf '%s\\n' \"${GH_TOKEN:?GH_TOKEN required}\" ;;\n"
        "  *) exit 1 ;;\n"
        "esac\n",
        encoding="utf-8",
    )
    askpass.chmod(0o700)
    environment = os.environ.copy()
    environment.update(
        {
            "GIT_ASKPASS": str(askpass),
            "GIT_ASKPASS_REQUIRE": "force",
            "GIT_TERMINAL_PROMPT": "0",
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "credential.helper",
            "GIT_CONFIG_VALUE_0": "",
            "CARGO_NET_GIT_FETCH_WITH_CLI": "true",
        }
    )
    return environment


def verified_repository(payload: object, full_name: str, visibility: str) -> dict[str, Any]:
    if not isinstance(payload, dict):
        fail(f"invalid repository response for {full_name}")
    if str(payload.get("full_name", "")).casefold() != full_name.casefold():
        fail(f"repository identity mismatch for {full_name}: {payload.get('full_name')!r}")
    expected_private = visibility == "private"
    if payload.get("private") is not expected_private or payload.get("visibility") != visibility:
        fail(
            f"visibility mismatch for {full_name}: private={payload.get('private')!r}, "
            f"visibility={payload.get('visibility')!r}, expected={visibility}"
        )
    return payload


def ensure_repository(owner: str, name: str, visibility: str, description: str) -> dict[str, Any]:
    full_name = f"{owner}/{name}"
    status, current = api("GET", f"/repos/{full_name}")
    if status == 200:
        return verified_repository(current, full_name, visibility)
    if status != 404:
        fail(f"failed to inspect {full_name}: HTTP {status}")
    payload = {
        "name": name,
        "description": description,
        "private": visibility == "private",
        "visibility": visibility,
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "auto_init": True,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "delete_branch_on_merge": True,
    }
    try:
        status, created = api("POST", f"/orgs/{owner}/repos", payload)
    except RuntimeError as error:
        if not re.search(r"GitHub API (409|422) for POST", str(error)):
            raise
        status, created = 422, None
    if status == 201:
        print(f"CREATED_{visibility.upper()} {full_name}")
        return verified_repository(created, full_name, visibility)
    if status not in {409, 422}:
        fail(f"failed to create {full_name}: HTTP {status}")
    status, reconciled = api("GET", f"/repos/{full_name}")
    if status != 200:
        fail(f"repository create race reconciliation failed for {full_name}: HTTP {status}")
    print(f"RECONCILED_{visibility.upper()} {full_name}")
    return verified_repository(reconciled, full_name, visibility)


def main_sha(full_name: str) -> str:
    sha = CORE.main_ref(full_name)
    if not sha:
        fail(f"{full_name} has no main branch")
    return sha


def clone_repository(full_name: str, destination: Path, environment: dict[str, str]) -> None:
    run(
        ["git", "clone", "--filter=blob:none", "--branch", "main", "--single-branch", f"https://github.com/{full_name}.git", str(destination)],
        env=environment,
    )


def configure_git(repository: Path) -> None:
    run(["git", "config", "user.name", "ORESoftware MCP fleet publisher"], cwd=repository)
    run(["git", "config", "user.email", "11139560+ORESoftware@users.noreply.github.com"], cwd=repository)


def remote_branch_sha(full_name: str, branch: str) -> str | None:
    escaped = urllib.parse.quote(branch, safe="")
    status, payload = api("GET", f"/repos/{full_name}/git/ref/heads/{escaped}")
    if status == 404:
        return None
    if status != 200 or not isinstance(payload, dict):
        fail(f"invalid branch response for {full_name}:{branch}")
    object_value = payload.get("object")
    if not isinstance(object_value, dict) or not isinstance(object_value.get("sha"), str):
        fail(f"missing branch SHA for {full_name}:{branch}")
    return str(object_value["sha"])


def install_rust_toolchain() -> str:
    rustup = shutil.which("rustup")
    cargo = shutil.which("cargo")
    if rustup:
        run(
            [
                rustup,
                "toolchain",
                "install",
                RUST_VERSION,
                "--profile",
                "minimal",
                "--component",
                "rustfmt",
                "--component",
                "clippy",
            ],
            timeout=1800,
        )
        return f"+{RUST_VERSION}"
    if not cargo:
        fail("protected publisher host has neither rustup nor cargo")
    version = run([cargo, "--version"]).strip()
    if RUST_VERSION not in version:
        fail(f"protected publisher cargo is not the reviewed toolchain: {version}")
    return ""


def cargo(repository: Path, toolchain: str, args: list[str], environment: dict[str, str], timeout: int = 1800) -> None:
    command = ["cargo"] + ([toolchain] if toolchain else []) + args
    run(command, cwd=repository, env=environment, timeout=timeout)


def verify_generated_repository(repository: Path) -> None:
    tracked = set(run(["git", "ls-files"], cwd=repository).splitlines())
    allowed_before = {"README.md", "LICENSE", "SECURITY.md", ".gitignore"}
    unexpected = tracked - allowed_before
    if unexpected:
        fail(f"refusing to bootstrap nonempty repository {repository.name}: {sorted(unexpected)}")


def checkout_product_branch(repository: Path, full_name: str, branch: str, environment: dict[str, str]) -> bool:
    observed = remote_branch_sha(full_name, branch)
    if observed is None:
        run(["git", "checkout", "-b", branch], cwd=repository)
        return False
    run(["git", "fetch", "origin", f"refs/heads/{branch}:refs/remotes/origin/{branch}"], cwd=repository, env=environment)
    run(["git", "checkout", "-B", branch, f"origin/{branch}"], cwd=repository)
    return True


def write_generated_tree(repository: Path, spec: RepositorySpec) -> None:
    for child in repository.iterdir():
        if child.name == ".git":
            continue
        if child.is_dir():
            shutil.rmtree(child)
        else:
            child.unlink()
    write_server_tree(spec, repository)


def validate_lockfile(repository: Path) -> None:
    lock = tomllib.loads((repository / "Cargo.lock").read_text(encoding="utf-8"))
    versions = {
        package["name"]: package["version"]
        for package in lock["package"]
        if package["name"] in {"rmcp", "rmcp-macros"}
    }
    expected = {"rmcp": RMCP_VERSION, "rmcp-macros": RMCP_VERSION}
    if versions != expected:
        fail(f"unexpected SDK lock resolution in {repository}: {versions!r}")
    shared = {
        package["name"]: package.get("source", "")
        for package in lock["package"]
        if package["name"] in {"ore-mcp-safety", "ore-mcp-testkit"}
    }
    if set(shared) != {"ore-mcp-safety", "ore-mcp-testkit"} or any(SHARED_REVISION not in source for source in shared.values()):
        fail(f"unexpected shared-library lock resolution in {repository}: {shared!r}")


def bootstrap_server(
    spec: RepositorySpec,
    work: Path,
    environment: dict[str, str],
    toolchain: str,
) -> tuple[int, str]:
    ensure_repository(spec.owner, spec.name, spec.visibility, spec.description)
    repository = work / f"server-{spec.owner}-{spec.name}"
    clone_repository(spec.full_name, repository, environment)
    configure_git(repository)

    marker = repository / ".mcp-bootstrap.json"
    if marker.is_file():
        value = json.loads(marker.read_text(encoding="utf-8"))
        if value.get("template_digest") == template_digest() and value.get("full_name") == spec.full_name:
            print(f"VERIFIED_SERVER_MAIN {spec.full_name} {main_sha(spec.full_name)}")
            return 0, main_sha(spec.full_name)

    branch_exists = checkout_product_branch(repository, spec.full_name, spec.branch, environment)
    if branch_exists:
        marker = repository / ".mcp-bootstrap.json"
        if not marker.is_file() or json.loads(marker.read_text()).get("template_digest") != template_digest():
            fail(f"existing bootstrap branch for {spec.full_name} has an unexpected template")
        head = run(["git", "rev-parse", "HEAD"], cwd=repository).strip()
    else:
        verify_generated_repository(repository)
        write_generated_tree(repository, spec)
        cargo(repository, toolchain, ["generate-lockfile"], environment)
        validate_lockfile(repository)
        cargo(repository, toolchain, ["fmt", "--all"], environment)
        cargo(repository, toolchain, ["clippy", "--locked", "--all-targets", "--", "-D", "warnings"], environment)
        cargo(repository, toolchain, ["test", "--locked", "--all-targets"], environment)
        doc_env = environment.copy()
        doc_env["RUSTDOCFLAGS"] = "-D warnings"
        cargo(repository, toolchain, ["doc", "--locked", "--no-deps"], doc_env)
        cargo(repository, toolchain, ["build", "--locked", "--release"], environment)
        run(["git", "add", "-A"], cwd=repository)
        run(["git", "diff", "--cached", "--check"], cwd=repository)
        run(["git", "commit", "-m", f"feat({spec.issue}): bootstrap read-only organization MCP server"], cwd=repository)
        head = run(["git", "rev-parse", "HEAD"], cwd=repository).strip()
        run(["git", "push", "--set-upstream", "origin", f"HEAD:refs/heads/{spec.branch}"], cwd=repository, env=environment)

    pr_number = ensure_pull_request(
        spec.full_name,
        spec.branch,
        f"{spec.issue}: bootstrap hardened read-only Rust MCP server",
        server_pr_body(spec, head),
    )
    wait_for_workflow(spec.full_name, head, "ci")
    merge_pull_request(spec.full_name, pr_number, head, f"{spec.issue}: bootstrap hardened read-only Rust MCP server")
    merged = main_sha(spec.full_name)
    print(f"MERGED_SERVER {spec.full_name} PR#{pr_number} {merged}")
    return pr_number, merged


def server_pr_body(spec: RepositorySpec, head: str) -> str:
    return f"""## Summary

Create the canonical `{spec.full_name}` implementation for {spec.issue}.

- official MCP Rust SDK pinned exactly to `rmcp ={RMCP_VERSION}`;
- stable `{spec.issue}` read-only domain diagnostics over stdio;
- immutable shared safety/testkit revision `{SHARED_REVISION}`;
- no network, subprocess, credential, filesystem-write, publication, tag, or mutation surface;
- closed schemas and domain-specific negative tests;
- exact request-ID echo, notification silence, post-error recovery, EOF shutdown, stdout-purity, annotation, output-bound, MSRV, Rustdoc, release, and RustSec gates;
- deterministic tracked `Cargo.lock`.

Exact generated head: `{head}`. The protected publisher merges only after the push-triggered CI workflow succeeds for this SHA.
"""


def ensure_pull_request(full_name: str, branch: str, title: str, body: str) -> int:
    owner = full_name.split("/", 1)[0]
    query = urllib.parse.urlencode({"state": "open", "head": f"{owner}:{branch}", "base": "main", "per_page": 20})
    payload = api_expect("GET", f"/repos/{full_name}/pulls?{query}", {200})
    if isinstance(payload, list) and payload:
        number = payload[0].get("number")
        if isinstance(number, int):
            return number
    payload = api_expect(
        "POST",
        f"/repos/{full_name}/pulls",
        {201},
        {"title": title, "head": branch, "base": "main", "body": body, "maintainer_can_modify": True},
    )
    if not isinstance(payload, dict) or not isinstance(payload.get("number"), int):
        fail(f"invalid PR response for {full_name}")
    print(f"OPENED_PR {full_name} #{payload['number']}")
    return int(payload["number"])


def wait_for_workflow(full_name: str, head_sha: str, workflow_name: str) -> None:
    deadline = time.monotonic() + CI_TIMEOUT_SECONDS
    encoded_sha = urllib.parse.quote(head_sha, safe="")
    last_state = "not observed"
    while time.monotonic() < deadline:
        payload = api_expect(
            "GET",
            f"/repos/{full_name}/actions/runs?head_sha={encoded_sha}&event=push&per_page=100",
            {200},
        )
        runs = payload.get("workflow_runs", []) if isinstance(payload, dict) else []
        matching = [run for run in runs if isinstance(run, dict) and run.get("head_sha") == head_sha and run.get("name") == workflow_name]
        if matching:
            run_record = sorted(matching, key=lambda value: value.get("run_number", 0))[-1]
            status = run_record.get("status")
            conclusion = run_record.get("conclusion")
            last_state = f"status={status} conclusion={conclusion} run={run_record.get('id')}"
            if status == "completed":
                if conclusion != "success":
                    fail(f"{full_name} {workflow_name} failed for {head_sha}: {last_state}")
                print(f"GREEN_CI {full_name} {head_sha} run={run_record.get('id')}")
                return
        time.sleep(POLL_SECONDS)
    fail(f"timed out waiting for {full_name} {workflow_name} at {head_sha}: {last_state}")


def merge_pull_request(full_name: str, number: int, head_sha: str, title: str) -> None:
    payload = api_expect(
        "PUT",
        f"/repos/{full_name}/pulls/{number}/merge",
        {200},
        {
            "sha": head_sha,
            "merge_method": "squash",
            "commit_title": title,
            "commit_message": "Validated and merged by the protected remaining-MCP fleet publisher.",
        },
    )
    if not isinstance(payload, dict) or payload.get("merged") is not True:
        fail(f"GitHub did not merge {full_name} PR #{number}: {payload!r}")


MONOREPO_VALIDATOR = '''#!/usr/bin/env python3
from __future__ import annotations
import configparser, json, subprocess
from pathlib import Path

root = Path(__file__).resolve().parents[1]
marker = json.loads((root / '.mcp-fleet-bootstrap.json').read_text())
expected = marker['submodules']
config = configparser.ConfigParser()
config.read(root / '.gitmodules')
observed = {}
for section in config.sections():
    path = config[section].get('path')
    url = config[section].get('url')
    if not path or not url or path in observed:
        raise SystemExit(f'invalid or duplicate submodule section: {section}')
    observed[path] = url
for item in expected:
    path, url, sha = item['path'], item['url'], item['sha']
    if observed.get(path) != url:
        raise SystemExit(f'{path}: URL mismatch {observed.get(path)!r} != {url!r}')
    line = subprocess.check_output(['git','-C',str(root),'ls-files','--stage','--',path], text=True).strip()
    parts = line.split()
    if len(parts) < 4 or parts[0] != '160000' or parts[1] != sha:
        raise SystemExit(f'{path}: expected gitlink {sha}, observed {line!r}')
print(f"validated {len(expected)} exact gitlinks")
'''

MONOREPO_WORKFLOW = '''name: MCP submodule contract

on:
  push:
  pull_request:
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  validate:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
        with:
          persist-credentials: false
      - uses: actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1 # v6
        with:
          python-version: '3.12'
      - run: python3 -m py_compile scripts/validate_submodules.py
      - run: python3 scripts/validate_submodules.py
      - run: git diff --check
'''


def add_or_pin_submodule(repository: Path, name: str, url: str, sha: str, environment: dict[str, str]) -> None:
    path = f"apps/{name}"
    gitmodules = repository / ".gitmodules"
    existing = run(["git", "config", "-f", str(gitmodules), "--get-regexp", r"^submodule\..*\.path$"], cwd=repository) if gitmodules.is_file() else ""
    paths = {line.split(maxsplit=1)[1] for line in existing.splitlines() if len(line.split(maxsplit=1)) == 2}
    if path not in paths:
        run(["git", "submodule", "add", "-b", "main", url, path], cwd=repository, env=environment, timeout=600)
    run(["git", "-C", path, "fetch", "--depth=1", "origin", sha], cwd=repository, env=environment, timeout=600)
    run(["git", "-C", path, "checkout", "--detach", sha], cwd=repository)
    run(["git", "add", ".gitmodules", path], cwd=repository)


def bootstrap_monorepo(
    spec: MonorepoSpec,
    server_shas: dict[str, str],
    work: Path,
    environment: dict[str, str],
) -> tuple[int, str]:
    ensure_repository(spec.owner, spec.name, spec.visibility, f"Canonical {spec.owner} organization monorepo with exact application gitlinks")
    repository = work / f"monorepo-{spec.owner}-{spec.name}"
    clone_repository(spec.full_name, repository, environment)
    configure_git(repository)

    branch_exists = checkout_product_branch(repository, spec.full_name, spec.branch, environment)
    if not branch_exists:
        for name, url in spec.repositories:
            full_name = f"{spec.owner}/{name}"
            sha = server_shas.get(full_name) or main_sha(full_name)
            add_or_pin_submodule(repository, name, url, sha, environment)
        marker = {
            "schema_version": 1,
            "tracking_issue": spec.issue,
            "template_digest": template_digest(),
            "submodules": [
                {
                    "path": f"apps/{name}",
                    "url": url,
                    "sha": server_shas.get(f"{spec.owner}/{name}") or main_sha(f"{spec.owner}/{name}"),
                }
                for name, url in spec.repositories
            ],
        }
        (repository / "scripts").mkdir(exist_ok=True)
        (repository / "scripts/validate_submodules.py").write_text(MONOREPO_VALIDATOR, encoding="utf-8")
        workflow = repository / ".github/workflows/mcp-submodule-contract.yml"
        workflow.parent.mkdir(parents=True, exist_ok=True)
        workflow.write_text(MONOREPO_WORKFLOW, encoding="utf-8")
        (repository / ".mcp-fleet-bootstrap.json").write_text(json.dumps(marker, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        readme = repository / "README.md"
        current = readme.read_text(encoding="utf-8") if readme.is_file() else f"# {spec.name}\n"
        section = f"\n## MCP server\n\n`apps/{next(name for name, _ in spec.repositories if name.endswith('mcp-server.rs'))}` is a real mode-160000 gitlink pinned by {spec.issue}.\n"
        if "## MCP server" not in current:
            readme.write_text(current.rstrip() + "\n" + section, encoding="utf-8")
        run(["git", "add", "-A"], cwd=repository)
        run(["git", "diff", "--cached", "--check"], cwd=repository)
        run(["python3", "scripts/validate_submodules.py"], cwd=repository)
        run(["git", "commit", "-m", f"chore({spec.issue}): wire exact MCP server gitlink"], cwd=repository)
        head = run(["git", "rev-parse", "HEAD"], cwd=repository).strip()
        run(["git", "push", "--set-upstream", "origin", f"HEAD:refs/heads/{spec.branch}"], cwd=repository, env=environment)
    else:
        marker = repository / ".mcp-fleet-bootstrap.json"
        if not marker.is_file() or json.loads(marker.read_text()).get("template_digest") != template_digest():
            fail(f"existing monorepo branch for {spec.full_name} has an unexpected template")
        head = run(["git", "rev-parse", "HEAD"], cwd=repository).strip()

    pr_number = ensure_pull_request(
        spec.full_name,
        spec.branch,
        f"{spec.issue}: wire canonical MCP server gitlink",
        f"Add exact mode-160000 application gitlinks and a read-only validation workflow after the MCP server passed CI and merged.\n\nExact head: `{head}`.",
    )
    wait_for_workflow(spec.full_name, head, "MCP submodule contract")
    merge_pull_request(spec.full_name, pr_number, head, f"{spec.issue}: wire canonical MCP server gitlink")
    merged = main_sha(spec.full_name)
    print(f"MERGED_MONOREPO {spec.full_name} PR#{pr_number} {merged}")
    return pr_number, merged


def main(argv: list[str] | None = None) -> int:
    arguments = argv or sys.argv[1:]
    if len(arguments) != 1:
        raise SystemExit("usage: publish_remaining_mcp_fleet.py REQUEST.json")
    manifest = json.loads(Path(arguments[0]).read_text(encoding="utf-8"))
    validate_request_manifest(manifest)
    work = Path(tempfile.mkdtemp(prefix="remaining-mcp-fleet-"))
    report: dict[str, Any] = {
        "schema_version": 1,
        "request_id": manifest["request_id"],
        "template_digest": template_digest(),
        "servers": {},
        "monorepos": {},
    }
    try:
        environment = git_environment(work)
        toolchain = install_rust_toolchain()
        server_shas: dict[str, str] = {}
        for spec in SERVER_SPECS:
            pr, sha = bootstrap_server(spec, work, environment, toolchain)
            server_shas[spec.full_name] = sha
            report["servers"][spec.full_name] = {"pull_request": pr, "main_sha": sha}
        for spec in MONOREPO_SPECS:
            pr, sha = bootstrap_monorepo(spec, server_shas, work, environment)
            report["monorepos"][spec.full_name] = {"pull_request": pr, "main_sha": sha}
    finally:
        shutil.rmtree(work, ignore_errors=True)
    report["status"] = "success"
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
