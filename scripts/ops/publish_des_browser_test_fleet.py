#!/usr/bin/env python3
"""Publish the discrete-event-systems-test browser automation fleet.

Run only from the trusted main-branch workflow after AWS OIDC loads and masks the
protected ORESoftware GitHub publisher token. Repository payloads are retained in
`des-browser-test-fleet-template.tar.gz.b64` beside this script.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import pathlib
import shutil
import subprocess
import tarfile
import tempfile
import textwrap
from dataclasses import dataclass
from typing import Iterable

ORG = "discrete-event-systems-test"
VERSION = "des-browser-fleet.v1"
BRANCH = "agent/des-browser-automation-v1"
PROJECT_TITLE = "DES Browser Automation"
TEMPLATE_SHA256 = "6622398aef1fcd6de496459cd45decfdf87daadbb51d049eb5fa27368ed49d47"


@dataclass(frozen=True)
class Repo:
    name: str
    template_dir: str
    description: str
    issue_title: str
    node: bool


REPOS = (
    Repo(
        ".github",
        "_dotgithub",
        "Organization profile and policy for DES cross-repository tests",
        "Track DES browser automation fleet governance",
        False,
    ),
    Repo(
        "des-route-smoke",
        "des-route-smoke",
        "Playwright route, health, API catalog, and ownership canaries for des-web.rs",
        "Track DES route and API canary coverage",
        True,
    ),
    Repo(
        "des-browser-flows",
        "des-browser-flows",
        "Playwright navigation and mounted-path browser contracts for des-web.rs",
        "Track DES browser-flow coverage",
        True,
    ),
    Repo(
        "des-gateway-compat",
        "des-gateway-compat",
        "Playwright public gateway, authentication, and DES compatibility-path canaries",
        "Track DES gateway compatibility retirement",
        True,
    ),
)


def dedent(text: str) -> str:
    return textwrap.dedent(text).strip() + "\n"


def run(
    args: Iterable[str],
    *,
    cwd: pathlib.Path | None = None,
    env: dict[str, str] | None = None,
    check: bool = True,
    capture: bool = False,
) -> subprocess.CompletedProcess[str]:
    command = [str(value) for value in args]
    print("+", " ".join(command))
    return subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=check,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )


def output(args: Iterable[str], *, cwd: pathlib.Path | None = None) -> str:
    return run(args, cwd=cwd, capture=True).stdout.strip()


def git_env(askpass: pathlib.Path) -> dict[str, str]:
    env = os.environ.copy()
    env["GIT_ASKPASS"] = str(askpass)
    env["GIT_TERMINAL_PROMPT"] = "0"
    return env


def verify_identity() -> None:
    if not os.environ.get("GH_TOKEN", "").strip():
        raise SystemExit("GH_TOKEN is required")
    login = output(["gh", "api", "user", "--jq", ".login"])
    if login != "ORESoftware":
        raise SystemExit(f"refusing to publish as {login!r}; expected ORESoftware")
    membership = output(
        ["gh", "api", f"/user/memberships/orgs/{ORG}", "--jq", '.role + ":" + .state']
    )
    if membership != "admin:active":
        raise SystemExit(f"{ORG} membership must be admin:active, got {membership!r}")


def extract_templates(destination: pathlib.Path) -> pathlib.Path:
    import hashlib

    source = pathlib.Path(__file__).with_name("des-browser-test-fleet-template.tar.gz.b64")
    archive = destination / "templates.tar.gz"
    archive.write_bytes(base64.b64decode(source.read_text(encoding="utf-8")))
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if digest != TEMPLATE_SHA256:
        raise SystemExit(f"template checksum mismatch: {digest} != {TEMPLATE_SHA256}")
    root = destination / "templates"
    root.mkdir()
    with tarfile.open(archive, mode="r:gz") as bundle:
        for member in bundle.getmembers():
            resolved = (root / member.name).resolve()
            if root.resolve() not in resolved.parents and resolved != root.resolve():
                raise SystemExit(f"unsafe template member: {member.name}")
        bundle.extractall(root, filter="data")
    return root


def exists(full_name: str) -> bool:
    result = run(["gh", "api", f"repos/{full_name}"], check=False, capture=True)
    if result.returncode == 0:
        return True
    if "404" in (result.stderr or "") or "Not Found" in (result.stderr or ""):
        return False
    raise RuntimeError(result.stderr or f"repository lookup failed: {full_name}")


def ensure_repo(repo: Repo) -> None:
    full_name = f"{ORG}/{repo.name}"
    if exists(full_name):
        print(f"{full_name} already exists")
        return
    run(
        [
            "gh",
            "api",
            "--method",
            "POST",
            f"orgs/{ORG}/repos",
            "-f",
            f"name={repo.name}",
            "-f",
            f"description={repo.description}",
            "-F",
            "private=false",
            "-F",
            "has_issues=true",
            "-F",
            "has_projects=false",
            "-F",
            "has_wiki=false",
            "-F",
            "auto_init=false",
        ]
    )


def has_main(full_name: str) -> bool:
    return (
        run(
            ["gh", "api", f"repos/{full_name}/git/ref/heads/main"],
            check=False,
            capture=True,
        ).returncode
        == 0
    )


def configure_git(path: pathlib.Path) -> None:
    run(["git", "config", "user.name", "ORESoftware DES test publisher"], cwd=path)
    run(["git", "config", "user.email", "bot@oresoftware.dev"], cwd=path)


def bootstrap(repo: Repo, workspace: pathlib.Path, askpass: pathlib.Path) -> None:
    full_name = f"{ORG}/{repo.name}"
    if has_main(full_name):
        return
    path = workspace / f"bootstrap-{repo.template_dir}"
    path.mkdir()
    run(["git", "init", "-b", "main"], cwd=path)
    configure_git(path)
    (path / "README.md").write_text(
        f"# {repo.name}\n\nBootstrap for reviewed DES browser automation.\n",
        encoding="utf-8",
    )
    run(["git", "add", "README.md"], cwd=path)
    run(["git", "commit", "-m", "chore: bootstrap repository"], cwd=path)
    run(["git", "remote", "add", "origin", f"https://github.com/{full_name}.git"], cwd=path)
    run(["git", "push", "origin", "HEAD:refs/heads/main"], cwd=path, env=git_env(askpass))


def marker_present(full_name: str) -> bool:
    result = run(
        ["gh", "api", f"repos/{full_name}/contents/README.md", "--jq", ".content"],
        check=False,
        capture=True,
    )
    if result.returncode != 0:
        return False
    try:
        content = base64.b64decode(result.stdout.replace("\n", "")).decode("utf-8")
    except Exception:
        return False
    return f"automation-version: {VERSION}" in content


def publish(repo: Repo, templates: pathlib.Path, workspace: pathlib.Path, askpass: pathlib.Path) -> None:
    full_name = f"{ORG}/{repo.name}"
    ensure_repo(repo)
    bootstrap(repo, workspace, askpass)
    if marker_present(full_name):
        print(f"{full_name} already contains {VERSION}; skipping")
        return

    path = workspace / f"repo-{repo.template_dir}"
    run(
        ["git", "clone", "--depth", "1", f"https://github.com/{full_name}.git", str(path)],
        env=git_env(askpass),
    )
    configure_git(path)
    for child in path.iterdir():
        if child.name == ".git":
            continue
        shutil.rmtree(child) if child.is_dir() else child.unlink()
    run(["git", "checkout", "-b", BRANCH], cwd=path)
    shutil.copytree(templates / repo.template_dir, path, dirs_exist_ok=True)

    if repo.node:
        run(["npm", "install", "--package-lock-only", "--ignore-scripts"], cwd=path)
        run(["npm", "ci", "--ignore-scripts"], cwd=path)
        run(["npx", "playwright", "test", "--list"], cwd=path)
        shutil.rmtree(path / "node_modules", ignore_errors=True)

    run(["git", "add", "-A"], cwd=path)
    run(["git", "commit", "-m", "test: add DES browser automation contracts"], cwd=path)
    run(["git", "push", "-u", "origin", BRANCH], cwd=path, env=git_env(askpass))
    pr = output(
        [
            "gh",
            "pr",
            "create",
            "--repo",
            full_name,
            "--base",
            "main",
            "--head",
            BRANCH,
            "--title",
            "Add cross-repository DES browser automation",
            "--body",
            dedent(
                f"""
                ## Summary

                Publishes the `{repo.name}` Playwright contracts for the
                canonical DES web server.

                - same bounded workflow on GitHub Actions and `gha-indie-worker`
                - in-cluster DES Service detection with public `/des` fallback
                - GitHub-hosted Playwright evidence artifacts
                - immutable worker-submission and ownership documentation

                Generated by the reviewed `ORESoftware/k8s-cluster` publisher.
                """
            ),
        ]
    )
    number = pr.rstrip("/").split("/")[-1]
    run(
        [
            "gh",
            "pr",
            "merge",
            number,
            "--repo",
            full_name,
            "--squash",
            "--admin",
            "--delete-branch",
            "--subject",
            "test: add DES browser automation contracts",
            "--body",
            "Publishes the reviewed cross-repository DES Playwright suite.",
        ]
    )
    if not marker_present(full_name):
        raise RuntimeError(f"{full_name}: merged main is missing {VERSION}")


def ensure_issue(repo: Repo) -> str:
    full_name = f"{ORG}/{repo.name}"
    matches = output(
        [
            "gh",
            "issue",
            "list",
            "--repo",
            full_name,
            "--state",
            "open",
            "--search",
            f'"{repo.issue_title}" in:title',
            "--json",
            "url,title",
        ]
    )
    for issue in json.loads(matches or "[]"):
        if issue["title"] == repo.issue_title:
            return issue["url"]
    return output(
        [
            "gh",
            "issue",
            "create",
            "--repo",
            full_name,
            "--title",
            repo.issue_title,
            "--body",
            dedent(
                f"""
                Track ongoing ownership for `{full_name}`.

                - keep GitHub-hosted browser contracts green;
                - plan and execute `.github/workflows/ci.yml` on `gha-indie-worker` at an immutable SHA;
                - retain failure evidence artifacts;
                - update this repository alongside route changes in `discrete-event-systems/des-web.rs`.

                Linear project: `github.com/{ORG}`.
                """
            ),
        ]
    )


def sync_project(issue_urls: list[str]) -> None:
    try:
        projects = json.loads(
            output(["gh", "project", "list", "--owner", ORG, "--format", "json"])
            or '{"projects":[]}'
        )
        project = next((p for p in projects.get("projects", []) if p["title"] == PROJECT_TITLE), None)
        if project is None:
            project = json.loads(
                output(
                    [
                        "gh",
                        "project",
                        "create",
                        "--owner",
                        ORG,
                        "--title",
                        PROJECT_TITLE,
                        "--format",
                        "json",
                    ]
                )
            )
        number = str(project["number"])
        for url in issue_urls:
            result = run(
                ["gh", "project", "item-add", number, "--owner", ORG, "--url", url],
                check=False,
                capture=True,
            )
            if result.returncode and "already exists" not in (result.stderr or "").lower():
                print(f"warning: project item add failed for {url}: {result.stderr}")
        print(f"GitHub project: https://github.com/orgs/{ORG}/projects/{number}")
    except Exception as error:
        print(f"warning: GitHub Projects synchronization unavailable: {error}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    print(json.dumps({"organization": ORG, "repositories": [repo.name for repo in REPOS]}, indent=2))
    if not args.execute:
        return
    verify_identity()
    with tempfile.TemporaryDirectory(prefix="des-browser-publisher-") as tmp:
        workspace = pathlib.Path(tmp)
        templates = extract_templates(workspace)
        askpass = workspace / "git-askpass.sh"
        askpass.write_text(
            '#!/bin/sh\ncase "$1" in *Username*) echo x-access-token;; *) echo "$GH_TOKEN";; esac\n',
            encoding="utf-8",
        )
        askpass.chmod(0o700)
        for repo in REPOS:
            publish(repo, templates, workspace, askpass)
        issues = [ensure_issue(repo) for repo in REPOS]
        sync_project(issues)
        print(json.dumps({"published": [f"{ORG}/{repo.name}" for repo in REPOS], "issues": issues}, indent=2))


if __name__ == "__main__":
    main()
