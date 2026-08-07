#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request

ORG = "meta-agents-demo"
NAME = "meta-agent-control-plane.rs"
FULL_NAME = f"{ORG}/{NAME}"
DESCRIPTION = "Single-binary Rust control plane for observable, reflective AI agents."
EXPECTED_MAIN = "4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1"
FEATURE = "agent/den-1057-meta-agent-control-plane"
EXPECTED_FEATURE = "789d48039da232faed985d4f8de176959f117e08"


def api(method: str, path: str, token: str, body: dict | None = None):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(f"https://api.github.com{path}", data=data, method=method)
    request.add_header("Accept", "application/vnd.github+json")
    request.add_header("Authorization", f"Bearer {token}")
    request.add_header("X-GitHub-Api-Version", "2022-11-28")
    request.add_header("User-Agent", "critical-org-fleet-publisher")
    if data is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = response.read()
            return response.status, json.loads(payload) if payload else None
    except urllib.error.HTTPError as error:
        payload = error.read(4096).decode(errors="replace")
        if error.code == 404 and method == "GET":
            return 404, None
        raise RuntimeError(f"GitHub API {error.code} for {method} {path}: {payload}") from error


def run(args: list[str], *, cwd: pathlib.Path | None = None, env: dict | None = None) -> str:
    process = subprocess.run(args, cwd=cwd, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if process.returncode:
        raise RuntimeError(
            f"command failed ({process.returncode}): {' '.join(args)}\n"
            f"stdout:\n{process.stdout[-4000:]}\nstderr:\n{process.stderr[-4000:]}"
        )
    return process.stdout.strip()


def main() -> int:
    token = os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN", "").strip()
    if not token:
        raise SystemExit("GITHUB_REPOSITORY_ADMIN_TOKEN is required")
    bundle = pathlib.Path(sys.argv[1]).resolve()
    if not bundle.is_file():
        raise SystemExit(f"bundle missing: {bundle}")

    status, repo = api("GET", f"/repos/{FULL_NAME}", token)
    if status == 404:
        _, repo = api(
            "POST",
            f"/orgs/{ORG}/repos",
            token,
            {
                "name": NAME,
                "description": DESCRIPTION,
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
    if not repo or repo.get("visibility") != "public":
        raise SystemExit(f"unexpected visibility or repository response for {FULL_NAME}")

    with tempfile.TemporaryDirectory(prefix="meta-control-plane-") as tmp:
        root = pathlib.Path(tmp) / "repo"
        run(["git", "clone", str(bundle), str(root)])
        main_sha = run(["git", "rev-parse", "refs/remotes/origin/main"], cwd=root)
        feature_sha = run(["git", "rev-parse", f"refs/heads/{FEATURE}"], cwd=root)
        if main_sha != EXPECTED_MAIN or feature_sha != EXPECTED_FEATURE:
            raise SystemExit(
                f"bundle refs changed: main={main_sha} feature={feature_sha}"
            )

        askpass = pathlib.Path(tmp) / "askpass.sh"
        askpass.write_text(
            '#!/bin/sh\ncase "$1" in *Username*) echo x-access-token;; *) echo "$GITHUB_REPOSITORY_ADMIN_TOKEN";; esac\n'
        )
        askpass.chmod(0o700)
        env = os.environ.copy()
        env["GIT_ASKPASS"] = str(askpass)
        env["GIT_TERMINAL_PROMPT"] = "0"
        run(["git", "remote", "set-url", "origin", f"https://github.com/{FULL_NAME}.git"], cwd=root)
        run(["git", "push", "origin", f"refs/remotes/origin/main:refs/heads/main"], cwd=root, env=env)
        run(["git", "push", "origin", f"refs/heads/{FEATURE}:refs/heads/{FEATURE}"], cwd=root, env=env)
        api("PATCH", f"/repos/{FULL_NAME}", token, {"default_branch": "main"})
        remote = run(["git", "ls-remote", "origin", "refs/heads/main", f"refs/heads/{FEATURE}"], cwd=root, env=env)
        observed = {}
        for line in remote.splitlines():
            sha, ref = line.split("\t", 1)
            observed[ref] = sha
        expected = {
            "refs/heads/main": EXPECTED_MAIN,
            f"refs/heads/{FEATURE}": EXPECTED_FEATURE,
        }
        if observed != expected:
            raise SystemExit(f"remote ref mismatch: {observed!r} != {expected!r}")

    print(json.dumps({"repository": FULL_NAME, "main": EXPECTED_MAIN, "feature": EXPECTED_FEATURE}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
