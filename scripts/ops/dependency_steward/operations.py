"""Manifest mutation, testing, remediation, and managed-PR helpers."""

from __future__ import annotations

import argparse
import base64
import configparser
import csv
import dataclasses
import datetime as dt
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict

from .models import *
from .runtime import *
from .scanners import *

def update_zpkg_dependency(path: Path, dependency: str, target: SemVer) -> None:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    in_dependencies = False
    changed = False
    key_pattern = re.compile(
        r"^(?P<prefix>\s*(?:" + re.escape(json.dumps(dependency)) + "|"
        + re.escape(dependency)
        + r")\s*=\s*)(?P<value>.*)$"
    )
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_dependencies = stripped == "[dependencies]"
            continue
        if not in_dependencies:
            continue
        newline = "\n" if line.endswith("\n") else ""
        body = line[:-1] if newline else line
        match = key_pattern.match(body)
        if not match:
            continue
        value = match.group("value")
        replacement = SEMVER_RE.sub(
            lambda semver: (
                semver.group(0)[:1] if semver.group(0).startswith("v") else ""
            )
            + str(target),
            value,
            count=1,
        )
        if replacement == value:
            raise StewardError(
                f"cannot locate SemVer for Zed dependency {dependency} in {path}"
            )
        lines[index] = match.group("prefix") + replacement + newline
        changed = True
        break
    if not changed:
        raise StewardError(f"cannot locate Zed dependency {dependency} in {path}")
    path.write_text("".join(lines), encoding="utf-8")


def load_repo_policy(root: Path) -> RepoPolicy:
    config_path = root / ".dependency-steward.toml"
    config: dict[str, Any] = {}
    if config_path.is_file():
        raw = tomllib.loads(config_path.read_text(encoding="utf-8"))
        section = raw.get("dependency_steward") or {}
        if isinstance(section, dict):
            config = section

    def string_list(key: str) -> list[str]:
        value = config.get(key)
        if value is None:
            return []
        if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
            raise StewardError(f"{config_path}: {key} must be an array of strings")
        return list(value)

    configured_tests = string_list("test_commands")
    if configured_tests:
        tests = configured_tests
    else:
        tests = default_test_commands(root)
    timeout = int(config.get("timeout_seconds", 3600))
    if timeout < 60 or timeout > 21600:
        raise StewardError("dependency_steward.timeout_seconds must be 60..21600")
    remediate = config.get("remediate_command")
    if remediate is not None and not isinstance(remediate, str):
        raise StewardError("dependency_steward.remediate_command must be a string")
    excluded = set(string_list("excluded_dependencies"))
    return RepoPolicy(
        test_commands=tests,
        prepare_commands=string_list("prepare_commands"),
        lock_commands=string_list("lock_commands"),
        remediate_command=remediate,
        timeout_seconds=timeout,
        excluded_dependencies=excluded,
    )


def default_test_commands(root: Path) -> list[str]:
    commands: list[str] = []
    zpkg = root / ".zpkg.toml"
    if zpkg.is_file():
        try:
            data = tomllib.loads(zpkg.read_text(encoding="utf-8"))
            script = (data.get("scripts") or {}).get("test")
            if isinstance(script, str) and script.strip():
                commands.append(script.strip())
        except tomllib.TOMLDecodeError:
            pass
    if (root / "flake.nix").is_file():
        commands.append("nix flake check")
    if (root / "Cargo.toml").is_file():
        commands.append("cargo test --locked --all-targets")
    if (root / "pnpm-lock.yaml").is_file() and (root / "package.json").is_file():
        commands.append("corepack pnpm install --frozen-lockfile && corepack pnpm test")
    elif (root / "yarn.lock").is_file() and (root / "package.json").is_file():
        commands.append("corepack yarn install --immutable && corepack yarn test")
    elif (root / "package-lock.json").is_file() and (root / "package.json").is_file():
        commands.append("npm ci && npm test")
    elif (root / "package.json").is_file():
        commands.append("npm install --ignore-scripts=false && npm test")
    if (root / "go.mod").is_file():
        commands.append("go test ./...")
    if (root / "pyproject.toml").is_file() or (root / "pytest.ini").is_file():
        commands.append("python -m pytest")
    if (root / "pubspec.yaml").is_file():
        commands.append(
            "if command -v flutter >/dev/null 2>&1; then flutter pub get && flutter test; "
            "else dart pub get && dart test; fi"
        )
    if (root / "gradlew").is_file():
        commands.append("./gradlew test")
    elif (root / "mvnw").is_file():
        commands.append("./mvnw test")
    elif (root / "pom.xml").is_file():
        commands.append("mvn test")
    # Preserve order while removing exact duplicates.
    return list(dict.fromkeys(commands))


def apply_dependency(
    root: Path,
    dep: DependencyRef,
    target: RemoteVersion,
    *,
    token: str,
    policy: RepoPolicy,
) -> None:
    if dep.kind == "zpkg":
        update_zpkg_dependency(
            root / dep.manifest_path, dep.locator["dependency"], target.version
        )
        commands = policy.lock_commands or ["zed install"]
        result = run_shell_commands(
            commands,
            cwd=(root / dep.manifest_path).parent,
            timeout_seconds=min(policy.timeout_seconds, 1800),
            env={"CI": "1", "ZED_PKG_NO_PROMPT": "1"},
        )
        if not result.passed:
            raise StewardError(f"Zed lock refresh failed:\n{result.log_tail}")
        return
    if dep.kind == "git-submodule":
        module_path = dep.locator["path"]
        auth = git_auth_config(token)
        run_process(
            ["git", *auth, "submodule", "update", "--init", "--recursive", "--", module_path],
            cwd=root,
            timeout=1800,
        )
        module = root / module_path
        run_process(
            ["git", *auth, "fetch", "--depth=1", "origin", target.sha],
            cwd=module,
            timeout=1200,
        )
        run_process(["git", "checkout", "--detach", target.sha], cwd=module)
        run_process(["git", "add", "--", module_path], cwd=root)
        return
    if dep.kind == "nix-flake":
        coordinate = canonical_github_repo(dep.source_url)
        if not coordinate:
            raise StewardError(f"unsupported Nix source: {dep.source_url}")
        cwd = (root / dep.manifest_path).parent
        run_process(
            [
                "nix",
                "flake",
                "lock",
                "--override-input",
                dep.locator["input"],
                f"github:{coordinate}/{target.tag}",
            ],
            cwd=cwd,
            timeout=1800,
        )
        return
    if policy.lock_commands and dep.kind == "nix-expression":
        env = {
            "DEPENDENCY_STEWARD_DEPENDENCY": dep.name,
            "DEPENDENCY_STEWARD_TARGET": str(target.version),
            "DEPENDENCY_STEWARD_TARGET_TAG": target.tag,
            "DEPENDENCY_STEWARD_TARGET_SHA": target.sha,
            "DEPENDENCY_STEWARD_MANIFEST": dep.manifest_path,
        }
        result = run_shell_commands(
            policy.lock_commands,
            cwd=root,
            timeout_seconds=min(policy.timeout_seconds, 1800),
            env=env,
        )
        if not result.passed:
            raise StewardError(f"repository update command failed:\n{result.log_tail}")
        return
    raise StewardError(f"{dep.kind} edge is not safely mutable")


def validate_patch(patch: str) -> None:
    if len(patch.encode()) > 2_000_000:
        raise StewardError("remediation patch exceeds 2 MB")
    touched: set[str] = set()
    for line in patch.splitlines():
        if line.startswith(("+++ ", "--- ")):
            value = line[4:].strip().split("\t", 1)[0]
            if value == "/dev/null":
                continue
            value = value.removeprefix("a/").removeprefix("b/")
            candidate = Path(value)
            if candidate.is_absolute() or ".." in candidate.parts:
                raise StewardError(f"unsafe patch path: {value}")
            touched.add(candidate.as_posix())
    denied = (
        ".github/workflows/",
        ".env",
        "env/enc/",
        ".git/",
    )
    for path in touched:
        if any(path == prefix.rstrip("/") or path.startswith(prefix) for prefix in denied):
            raise StewardError(f"remediation may not modify protected path: {path}")


def http_remediation_patch(
    endpoint: str,
    token: str | None,
    payload: Mapping[str, Any],
) -> str | None:
    headers = {"Content-Type": "application/json", "User-Agent": JOB_MARKER}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(
        endpoint,
        data=json.dumps(payload).encode(),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=900) as response:
            result = json.loads(response.read())
    except (urllib.error.URLError, json.JSONDecodeError) as exc:
        raise StewardError(f"remediation endpoint failed: {exc}") from exc
    patch = result.get("patch") if isinstance(result, dict) else None
    if patch is None:
        return None
    if not isinstance(patch, str):
        raise StewardError("remediation endpoint patch must be a string")
    validate_patch(patch)
    return patch


def try_remediation(
    *,
    root: Path,
    repository: str,
    base_sha: str,
    dep: DependencyRef,
    target: RemoteVersion,
    policy: RepoPolicy,
    failed: CommandResult,
    endpoint: str | None,
    endpoint_token: str | None,
    global_command: str | None,
) -> tuple[bool, str | None]:
    env = {
        "DEPENDENCY_STEWARD_REPOSITORY": repository,
        "DEPENDENCY_STEWARD_BASE_SHA": base_sha,
        "DEPENDENCY_STEWARD_DEPENDENCY": dep.name,
        "DEPENDENCY_STEWARD_KIND": dep.kind,
        "DEPENDENCY_STEWARD_CURRENT": (
            str(dep.current_version) if dep.current_version else str(dep.current_ref or "")
        ),
        "DEPENDENCY_STEWARD_TARGET": str(target.version),
        "DEPENDENCY_STEWARD_TARGET_TAG": target.tag,
        "DEPENDENCY_STEWARD_TARGET_SHA": target.sha,
        "DEPENDENCY_STEWARD_FAILED_COMMAND": failed.command,
    }
    command = policy.remediate_command or global_command
    if command:
        result = run_shell_commands(
            [command],
            cwd=root,
            timeout_seconds=min(policy.timeout_seconds, 1800),
            env=env,
        )
        if result.passed:
            diff = run_process(["git", "diff", "--binary"], cwd=root).stdout
            return bool(diff.strip()), diff
    if endpoint:
        patch = http_remediation_patch(
            endpoint,
            endpoint_token,
            {
                "contract": JOB_MARKER,
                "repository": repository,
                "base_sha": base_sha,
                "dependency": dep.graph_dict(),
                "target": dataclasses.asdict(target),
                "failed_command": failed.command,
                "log_tail": failed.log_tail[-10000:],
            },
        )
        if patch:
            patch_file = root / ".git" / "dependency-steward-remediation.patch"
            patch_file.write_text(patch, encoding="utf-8")
            run_process(["git", "apply", "--check", str(patch_file)], cwd=root)
            run_process(["git", "apply", str(patch_file)], cwd=root)
            return True, run_process(["git", "diff", "--binary"], cwd=root).stdout
    return False, None


def git_diff(root: Path) -> str:
    return run_process(["git", "diff", "--binary", "HEAD"], cwd=root).stdout


def changed_files(root: Path) -> list[str]:
    result = run_process(
        ["git", "status", "--porcelain=v1", "-z"], cwd=root
    ).stdout
    paths: list[str] = []
    for entry in result.split("\0"):
        if not entry:
            continue
        path = entry[3:] if len(entry) > 3 else entry
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        paths.append(path)
    return sorted(set(paths))


def push_branch(
    *,
    root: Path,
    branch: str,
    base_sha: str,
    token: str,
    message: str,
) -> None:
    run_process(["git", "checkout", "-B", branch, base_sha], cwd=root)
    run_process(["git", "add", "-A"], cwd=root)
    if not run_process(["git", "diff", "--cached", "--quiet"], cwd=root, check=False).returncode:
        raise StewardError("refusing to publish an empty dependency update")
    run_process(
        ["git", "-c", "core.hooksPath=/dev/null", "commit", "-m", message],
        cwd=root,
    )
    auth = git_auth_config(token)
    remote = run_process(
        ["git", *auth, "ls-remote", "--heads", "origin", f"refs/heads/{branch}"],
        cwd=root,
        check=False,
    ).stdout.strip()
    push = ["git", *auth, "push", "origin", f"HEAD:refs/heads/{branch}"]
    if remote:
        old_sha = remote.split()[0]
        push.append(f"--force-with-lease=refs/heads/{branch}:{old_sha}")
    else:
        push.append("--force-with-lease")
    run_process(push, cwd=root, timeout=1200)


def issue_marker(category: str, repository: str, dep: DependencyRef, target: str) -> str:
    digest = hashlib.sha256(
        f"{category}|{repository}|{dep.key}|{target}".encode()
    ).hexdigest()[:24]
    return f"{JOB_MARKER}:{category}:{digest}"


def format_attempts(attempts: Sequence[ProbeResult]) -> str:
    if not attempts:
        return "No candidate tests completed."
    rows = ["| Target | Result | Seconds | Remediated |", "|---|---:|---:|---:|"]
    for item in attempts:
        rows.append(
            f"| `{item.version}` | {'pass' if item.passed else 'fail'} | "
            f"{item.duration_seconds:.1f} | {'yes' if item.remediated else 'no'} |"
        )
    return "\n".join(rows)


def pr_body(
    *,
    repository: str,
    dep: DependencyRef,
    current: SemVer,
    target: RemoteVersion,
    base_sha: str,
    tests: Sequence[str],
    attempts: Sequence[ProbeResult],
    remediated: bool,
) -> str:
    marker = pr_marker(
        {
            "base_sha": base_sha,
            "key": dep.key,
            "kind": dep.kind,
            "repository": repository,
            "target": str(target.version),
        }
    )
    commands = "\n".join(f"- `{command}`" for command in tests)
    return f"""{marker}

## Minor-only dependency update

- Dependency: `{dep.name}`
- Source kind: `{dep.kind}`
- Manifest: `{dep.manifest_path}`
- Current: `{current}`
- Target: `{target.version}` (`{target.tag}` / `{target.sha}`)
- Exact base SHA: `{base_sha}`
- Compatibility remediation applied: `{'yes' if remediated else 'no'}`

Patch-only releases were deliberately skipped. Major releases are handled only
through the mapped Linear project and are never included in this PR.

## Validation

{commands or '- No command (this PR would not have been opened)'}

{format_attempts(attempts)}

This PR is intentionally not auto-merged. Branch protection and repository CI
remain authoritative.
"""
