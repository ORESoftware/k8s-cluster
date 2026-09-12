"""Credential-safe repository checkout and bot-branch publication."""

from __future__ import annotations

import base64
import os
import shutil
import subprocess
from pathlib import Path

from .model import BotError, Repository, run_command, safe_slug

class GitWorkspace:
    def __init__(self, repository: Repository, token: str, root: Path) -> None:
        self.repository = repository
        self.root = root
        self.path = root / safe_slug(repository.full_name.replace("/", "--"), 100)
        auth = base64.b64encode(f"x-access-token:{token}".encode()).decode()
        self.git_env = dict(os.environ)
        self.git_env.update(
            {
                "GIT_TERMINAL_PROMPT": "0",
                "GIT_CONFIG_COUNT": "1",
                "GIT_CONFIG_KEY_0": "http.https://github.com/.extraheader",
                "GIT_CONFIG_VALUE_0": f"AUTHORIZATION: basic {auth}",
            }
        )

    def clone(self) -> None:
        self.root.mkdir(parents=True, exist_ok=True)
        if self.path.exists():
            shutil.rmtree(self.path)
        run_command(
            (
                "git",
                "clone",
                "--filter=blob:none",
                "--no-tags",
                "--single-branch",
                "--branch",
                self.repository.default_branch,
                self.repository.clone_url,
                str(self.path),
            ),
            env=self.git_env,
            timeout=600,
        )
        self.git("config", "user.name", "portfolio-dependency-bot")
        self.git("config", "user.email", "dependency-bot@users.noreply.github.com")

    def git(self, *args: str, check: bool = True, timeout: int = 300) -> subprocess.CompletedProcess[str]:
        return run_command(("git", *args), cwd=self.path, env=self.git_env, timeout=timeout, check=check)

    def reset_to_base(self, branch: str) -> None:
        self.git("fetch", "origin", self.repository.default_branch, timeout=600)
        self.git("checkout", "-B", branch, f"origin/{self.repository.default_branch}")
        self.git("reset", "--hard", f"origin/{self.repository.default_branch}")
        self.git("clean", "-ffd")

    def commit_and_push(self, branch: str, message: str) -> str:
        self.git("add", "-A")
        diff = self.git("diff", "--cached", "--quiet", check=False)
        if diff.returncode == 0:
            raise BotError("candidate produced no tracked diff")
        if diff.returncode not in {0, 1}:
            raise BotError("git diff --cached failed")
        self.git("commit", "-m", message)
        sha = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("push", "--force", "origin", f"HEAD:refs/heads/{branch}", timeout=900)
        return sha

    def remove_remote_branch(self, branch: str) -> None:
        self.git("push", "origin", f":refs/heads/{branch}", check=False, timeout=300)
