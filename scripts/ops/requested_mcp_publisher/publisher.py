"""Deterministic Git materialization and no-force publication."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from typing import Any

from .github import (
    EXPECTED_LOGIN,
    GitHubClient,
    bootstrap_is_ancestor,
    configure_repository,
    create_repository,
    main_ref,
    preflight,
    repository,
    validate_repository_metadata,
)
from .model import (
    COMMIT_AUTHOR_EMAIL,
    COMMIT_AUTHOR_NAME,
    COMMIT_DATE,
    PublisherError,
    RepositorySpec,
    bootstrap_files,
)


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
    if completed.returncode != 0:
        output = (completed.stdout or "")[-4000:]
        raise PublisherError(f"command failed ({completed.returncode}): {args[0]} {args[1:]}\n{output}")
    return completed.stdout or ""


def materialize_bootstrap(spec: RepositorySpec, root: Path) -> str:
    root.mkdir(parents=True, exist_ok=False)
    for relative_path, content in bootstrap_files(spec).items():
        destination = root / relative_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content, encoding="utf-8", newline="\n")

    run(["git", "init", "-b", "main"], cwd=root)
    run(["git", "config", "user.name", COMMIT_AUTHOR_NAME], cwd=root)
    run(["git", "config", "user.email", COMMIT_AUTHOR_EMAIL], cwd=root)
    run(["git", "config", "commit.gpgsign", "false"], cwd=root)
    run(["git", "config", "core.autocrlf", "false"], cwd=root)
    run(["git", "add", "--", "."], cwd=root)
    environment = os.environ.copy()
    environment.update({"GIT_AUTHOR_DATE": COMMIT_DATE, "GIT_COMMITTER_DATE": COMMIT_DATE})
    run(["git", "commit", "-m", f"chore: bootstrap {spec.name}"], cwd=root, env=environment)
    return run(["git", "rev-parse", "HEAD"], cwd=root).strip()


def push_bootstrap(local: Path, spec: RepositorySpec, expected_sha: str) -> None:
    askpass = local.parent / f".{spec.owner}-{spec.name}.askpass.sh"
    askpass.write_text(
        '#!/usr/bin/env sh\ncase "${1:-}" in *Username*) printf "%s\\n" x-access-token;; *Password*) printf "%s\\n" "${GH_TOKEN:?GH_TOKEN required}";; *) exit 1;; esac\n',
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
    remote = f"https://github.com/{spec.full_name}.git"
    if "@" in remote or not remote.startswith("https://github.com/"):
        raise PublisherError("unsafe Git remote construction")
    try:
        run(["git", "remote", "add", "canonical", remote], cwd=local)
        run(
            ["git", "push", "--set-upstream", "canonical", "HEAD:refs/heads/main"],
            cwd=local,
            env=environment,
        )
    finally:
        askpass.unlink(missing_ok=True)
    actual = run(["git", "rev-parse", "HEAD"], cwd=local).strip()
    if actual != expected_sha:
        raise PublisherError(f"local commit changed before push for {spec.full_name}")


def publish(specs: tuple[RepositorySpec, ...], report_path: Path) -> dict[str, Any]:
    client = GitHubClient(os.environ.get("GH_TOKEN", ""))
    preflight(client, specs)
    report: dict[str, Any] = {
        "schema_version": 1,
        "publisher": EXPECTED_LOGIN,
        "repositories": [],
    }
    work = Path(tempfile.mkdtemp(prefix="requested-mcp-repositories."))
    try:
        for spec in specs:
            local = work / spec.owner / spec.name
            expected_sha = materialize_bootstrap(spec, local)
            status, payload = repository(client, spec)
            created = False
            if status == 404:
                payload = create_repository(client, spec)
                created = True
            if not isinstance(payload, dict):
                raise PublisherError(f"missing repository metadata for {spec.full_name}")
            validate_repository_metadata(spec, payload)

            current = main_ref(client, spec)
            if current is None:
                push_bootstrap(local, spec, expected_sha)
                current = main_ref(client, spec)
                if current != expected_sha:
                    raise PublisherError(
                        f"remote bootstrap verification failed for {spec.full_name}: {current!r}"
                    )
            elif not bootstrap_is_ancestor(client, spec, expected_sha, current):
                raise PublisherError(
                    f"refusing unrelated existing history for {spec.full_name}: main={current}"
                )

            configure_repository(client, spec)
            status, payload = repository(client, spec)
            if status != 200 or not isinstance(payload, dict):
                raise PublisherError(f"unable to re-read {spec.full_name}")
            validate_repository_metadata(spec, payload)
            if payload.get("default_branch") != "main":
                raise PublisherError(f"default branch is not main for {spec.full_name}")

            report["repositories"].append(
                {
                    "full_name": spec.full_name,
                    "visibility": spec.visibility,
                    "created": created,
                    "bootstrap_sha": expected_sha,
                    "current_main_sha": current,
                    "bootstrap_is_ancestor": True,
                }
            )
            print(f"VERIFIED {spec.full_name} visibility={spec.visibility} main={current}")
    finally:
        shutil.rmtree(work, ignore_errors=True)

    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return report


def check(specs: tuple[RepositorySpec, ...]) -> dict[str, Any]:
    work = Path(tempfile.mkdtemp(prefix="requested-mcp-check."))
    rows = []
    try:
        for spec in specs:
            sha = materialize_bootstrap(spec, work / spec.owner / spec.name)
            rows.append(
                {
                    "full_name": spec.full_name,
                    "visibility": spec.visibility,
                    "bootstrap_sha": sha,
                    "files": sorted(bootstrap_files(spec)),
                }
            )
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return {"schema_version": 1, "repositories": rows}
