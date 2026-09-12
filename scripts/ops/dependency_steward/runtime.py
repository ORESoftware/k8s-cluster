"""Credential-isolated process, clone, and worktree helpers."""

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

def git_auth_config(token: str) -> list[str]:
    encoded = base64.b64encode(f"x-access-token:{token}".encode()).decode()
    return ["-c", f"http.https://github.com/.extraheader=AUTHORIZATION: basic {encoded}"]


def run_process(
    args: Sequence[str],
    *,
    cwd: Path | None = None,
    env: Mapping[str, str] | None = None,
    timeout: int = 600,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    merged_env = sanitized_environment(env)
    completed = subprocess.run(
        list(args),
        cwd=cwd,
        env=merged_env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=timeout,
        check=False,
    )
    if check and completed.returncode:
        command = display_command(args)
        tail = redact((completed.stdout or "")[-4000:])
        raise StewardError(f"command failed ({completed.returncode}): {command}\n{tail}")
    return completed


def run_shell_commands(
    commands: Sequence[str],
    *,
    cwd: Path,
    timeout_seconds: int,
    env: Mapping[str, str] | None = None,
) -> CommandResult:
    started = time.monotonic()
    logs: list[str] = []
    for command in commands:
        remaining = timeout_seconds - int(time.monotonic() - started)
        if remaining <= 0:
            return CommandResult(
                False,
                command,
                "test command budget exhausted",
                time.monotonic() - started,
            )
        try:
            result = run_process(
                ["bash", "-lc", command],
                cwd=cwd,
                env=env,
                timeout=remaining,
                check=False,
            )
        except subprocess.TimeoutExpired:
            logs.append(f"$ {command}\nTIMEOUT")
            return CommandResult(
                False,
                command,
                redact("\n".join(logs)[-12000:]),
                time.monotonic() - started,
            )
        logs.append(f"$ {command}\n{result.stdout or ''}")
        if result.returncode:
            return CommandResult(
                False,
                command,
                redact("\n".join(logs)[-12000:]),
                time.monotonic() - started,
            )
    return CommandResult(
        True,
        commands[-1] if commands else "",
        redact("\n".join(logs)[-12000:]),
        time.monotonic() - started,
    )


def clone_exact_repository(
    *,
    full_name: str,
    clone_url: str,
    branch: str,
    sha: str,
    token: str,
    destination: Path,
) -> None:
    auth = git_auth_config(token)
    run_process(
        [
            "git",
            *auth,
            "clone",
            "--filter=blob:none",
            "--no-tags",
            "--no-checkout",
            clone_url,
            str(destination),
        ],
        timeout=1800,
    )
    run_process(
        ["git", *auth, "fetch", "--depth=1", "origin", sha],
        cwd=destination,
        timeout=1200,
    )
    run_process(["git", "checkout", "--detach", sha], cwd=destination)
    actual = run_process(["git", "rev-parse", "HEAD"], cwd=destination).stdout.strip()
    if actual != sha:
        raise StewardError(f"exact checkout mismatch for {full_name}: {actual} != {sha}")
    run_process(["git", "config", "user.name", "dependency-steward[bot]"], cwd=destination)
    run_process(
        ["git", "config", "user.email", "dependency-steward@users.noreply.github.com"],
        cwd=destination,
    )
    run_process(["git", "config", "advice.detachedHead", "false"], cwd=destination)


def reset_worktree(repo: Path, base_sha: str) -> None:
    run_process(["git", "reset", "--hard", base_sha], cwd=repo)
    run_process(["git", "clean", "-ffdx"], cwd=repo)
    run_process(
        ["git", "submodule", "foreach", "--recursive", "git reset --hard || true"],
        cwd=repo,
        check=False,
    )
    run_process(
        ["git", "submodule", "foreach", "--recursive", "git clean -ffdx || true"],
        cwd=repo,
        check=False,
    )


def iter_manifest_paths(root: Path) -> Iterable[Path]:
    for current, directories, files in os.walk(root):
        directories[:] = [name for name in directories if name not in SKIP_DIRS]
        base = Path(current)
        for name in files:
            if (
                name in {".gitmodules", ".zpkg.toml", ".zpkg.lock", "flake.lock"}
                or name.endswith(".nix")
            ):
                yield base / name


def relpath(path: Path, root: Path) -> str:
    return path.relative_to(root).as_posix()
