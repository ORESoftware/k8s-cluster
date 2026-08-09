#!/usr/bin/env python3
"""Publish non-destructive recovery branches for previously unpushed StreemPilot work.

The recovery branch is based on the current remote main branch. Recovered files are
stored under an isolated `.recovery/` namespace; current product paths are never
modified. This preserves the exact recovered blobs and patch while making semantic
conflicts explicit instead of choosing an older or newer side wholesale.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ARCHIVE_DIR = ".recovery/streamyard-parity-foundation-20260809"
DEFAULT_BRANCH = "recovery/streamyard-parity-foundation-20260809"
FULL_SHA = re.compile(r"^[0-9a-f]{40}$")


class RecoveryError(RuntimeError):
    pass


def run(
    args: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    check: bool = True,
    capture: bool = True,
) -> subprocess.CompletedProcess[str]:
    merged = os.environ.copy()
    if env:
        merged.update(env)
    return subprocess.run(
        args,
        cwd=cwd,
        env=merged,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
        check=check,
    )


def output(args: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> str:
    result = run(args, cwd=cwd, env=env)
    return result.stdout.strip()


def git(repo: Path, *args: str, env: dict[str, str] | None = None, check: bool = True) -> str:
    result = run(["git", *args], cwd=repo, env=env, check=check)
    return result.stdout.strip()


def gh(*args: str, check: bool = True) -> str:
    result = run(["gh", *args], check=check)
    return result.stdout.strip()


def safe_repo_name(full: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", full):
        raise RecoveryError(f"unsafe repository name: {full!r}")
    return full.split("/", 1)[1]


def git_object_sha(path: Path) -> str:
    sha = output(["git", "hash-object", str(path)])
    if not FULL_SHA.fullmatch(sha):
        raise RecoveryError(f"invalid Git blob SHA for {path}: {sha!r}")
    return sha


def path_blob(repo: Path, commit: str, path: str) -> str | None:
    result = run(["git", "rev-parse", f"{commit}:{path}"], cwd=repo, check=False)
    if result.returncode == 0:
        sha = result.stdout.strip()
        if not FULL_SHA.fullmatch(sha):
            raise RecoveryError(f"invalid remote blob SHA for {path}: {sha!r}")
        return sha
    return None


def markdown_escape(text: str) -> str:
    return text.replace("|", "\\|").replace("`", "\\`")


@dataclass
class Classification:
    path: str
    status: str
    recovered_blob: str
    current_blob: str | None


def notes_for(entry: dict[str, Any], current_main: str, classes: list[Classification]) -> str:
    exact = sum(c.status == "already-integrated-exactly" for c in classes)
    absent = sum(c.status == "absent-from-current-main" for c in classes)
    diverged = sum(c.status == "diverged-or-superseded" for c in classes)
    lines = [
        "# Recovered StreamYard-parity foundation snapshot",
        "",
        "This directory preserves code that existed in an earlier prepared Git history but whose exact commit objects were not present on the remote repository.",
        "",
        "## Provenance",
        "",
        f"- Repository: `{entry['repository']}`",
        f"- Original prepared main: `{entry['original_main_sha']}`",
        f"- Original prepared feature: `{entry['original_feature_sha']}`",
        f"- Original prepared branch: `{entry['original_feature_branch']}`",
        f"- Current remote main at recovery: `{current_main}`",
        "",
        "## Semantic resolution",
        "",
        "Current product paths remain authoritative. No recovered file was copied over a current source path, no branch was force-pushed, and no default branch was changed.",
        "",
        "- `already-integrated-exactly`: the recovered blob is already present at the same path on current `main`; no product change is required.",
        "- `absent-from-current-main`: the recovered file is preserved here for review, but is not silently introduced into the product tree.",
        "- `diverged-or-superseded`: current `main` contains a different blob; the current implementation wins unless a later, path-specific review deliberately ports an invariant from the recovered version.",
        "",
        f"Summary: **{exact} exact**, **{absent} absent**, **{diverged} diverged/superseded**.",
        "",
        "## File classification",
        "",
        "| Original path | Classification | Recovered blob | Current-main blob |",
        "|---|---|---|---|",
    ]
    for item in classes:
        lines.append(
            f"| `{markdown_escape(item.path)}` | `{item.status}` | `{item.recovered_blob}` | "
            + (f"`{item.current_blob}`" if item.current_blob else "—")
            + " |"
        )
    lines += [
        "",
        "## Preserved artifacts",
        "",
        "- `snapshot/` contains the exact recovered file blobs at their original relative paths.",
        "- `recovered.patch` is the original binary-safe full-index patch from the prepared main to the prepared feature commit.",
        "- `metadata.json` records commit identity, authorship, dates, original parents, modes, and blob hashes.",
        "",
        "Do not merge this directory by mechanically copying `snapshot/` into the repository root. Port any useful invariant through a separate semantic change based on current `main`.",
        "",
    ]
    return "\n".join(lines)


def add_file_to_index(repo: Path, index_env: dict[str, str], source: Path, destination: str, mode: str = "100644") -> str:
    if not re.fullmatch(r"100[67]55|100644", mode):
        raise RecoveryError(f"unsupported mode {mode!r} for {destination}")
    blob = git(repo, "hash-object", "-w", str(source), env=index_env)
    if not FULL_SHA.fullmatch(blob):
        raise RecoveryError(f"failed to write blob for {source}")
    git(repo, "update-index", "--add", "--cacheinfo", f"{mode},{blob},{destination}", env=index_env)
    return blob


def create_pr(full: str, branch: str, report: dict[str, Any]) -> tuple[int, str]:
    existing_raw = gh(
        "pr", "list", "--repo", full, "--head", branch, "--state", "all",
        "--json", "number,url,state", "--limit", "10",
    )
    existing = json.loads(existing_raw or "[]")
    if existing:
        return int(existing[0]["number"]), str(existing[0]["url"])

    counts = report["classification_counts"]
    body = (
        "## Recovery purpose\n\n"
        "Preserve an exact, previously unpushed StreamYard-parity foundation snapshot without replacing any current product file.\n\n"
        "## Semantic merge result\n\n"
        f"- already integrated exactly: **{counts['already-integrated-exactly']}**\n"
        f"- absent from current main: **{counts['absent-from-current-main']}**\n"
        f"- diverged or superseded: **{counts['diverged-or-superseded']}**\n\n"
        "All recovered content lives under `.recovery/streamyard-parity-foundation-20260809/`. "
        "Current source paths, default branches, and existing histories are unchanged. This is a draft archival PR, not permission to overwrite current implementations.\n\n"
        f"Original prepared feature: `{report['original_feature_sha']}`\n\n"
        "See `RECOVERY_NOTES.md` in the recovery directory for the path-by-path classification.\n"
    )
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", delete=False) as handle:
        handle.write(body)
        body_path = handle.name
    try:
        url = gh(
            "pr", "create", "--repo", full, "--base", "main", "--head", branch,
            "--draft", "--title", "chore: preserve recovered StreamYard parity snapshot",
            "--body-file", body_path,
        )
    finally:
        Path(body_path).unlink(missing_ok=True)
    number = int(url.rstrip("/").split("/")[-1])
    return number, url


def recover_repository(payload_root: Path, entry: dict[str, Any], work_root: Path, branch: str) -> dict[str, Any]:
    full = str(entry["repository"])
    name = safe_repo_name(full)
    if full != f"StreemPilot/{name}":
        raise RecoveryError(f"recovery scope is restricted to StreemPilot: {full}")
    repo_payload = payload_root / "repositories" / name
    if not repo_payload.is_dir():
        raise RecoveryError(f"payload missing for {full}")

    default_branch = gh("api", f"repos/{full}", "--jq", ".default_branch")
    if default_branch != "main":
        raise RecoveryError(f"unexpected default branch for {full}: {default_branch}")

    repo = work_root / name
    repo.mkdir(parents=True)
    git(repo, "init", "-q")
    git(repo, "remote", "add", "origin", f"https://github.com/{full}.git")
    git(repo, "config", "gc.auto", "0")
    git(repo, "fetch", "-q", "--filter=blob:none", "--depth=1", "origin", "main")
    current_main = git(repo, "rev-parse", "FETCH_HEAD")
    if not FULL_SHA.fullmatch(current_main):
        raise RecoveryError(f"invalid current main SHA for {full}")

    classes: list[Classification] = []
    for file_info in entry["files"]:
        path = str(file_info["path"])
        snapshot = repo_payload / "snapshot" / path
        if not snapshot.is_file():
            raise RecoveryError(f"snapshot missing: {full}:{path}")
        recovered_blob = git_object_sha(snapshot)
        if recovered_blob != file_info["feature_blob"]:
            raise RecoveryError(
                f"payload blob mismatch for {full}:{path}: {recovered_blob} != {file_info['feature_blob']}"
            )
        current_blob = path_blob(repo, current_main, path)
        if current_blob == recovered_blob:
            status = "already-integrated-exactly"
        elif current_blob is None:
            status = "absent-from-current-main"
        else:
            status = "diverged-or-superseded"
        classes.append(Classification(path, status, recovered_blob, current_blob))

    notes = notes_for(entry, current_main, classes)
    notes_file = repo_payload / "RECOVERY_NOTES.generated.md"
    notes_file.write_text(notes, encoding="utf-8")

    index_path = repo / "recovery.index"
    index_env = {"GIT_INDEX_FILE": str(index_path)}
    git(repo, "read-tree", current_main, env=index_env)

    existing_namespace = run(
        ["git", "cat-file", "-e", f"{current_main}:{ARCHIVE_DIR}"], cwd=repo, check=False
    )
    if existing_namespace.returncode == 0:
        raise RecoveryError(f"{full}: recovery namespace already exists on current main")

    prefix = ARCHIVE_DIR
    add_file_to_index(repo, index_env, notes_file, f"{prefix}/RECOVERY_NOTES.md")
    add_file_to_index(repo, index_env, repo_payload / "metadata.json", f"{prefix}/metadata.json")
    add_file_to_index(repo, index_env, repo_payload / "recovered.patch", f"{prefix}/recovered.patch")
    for file_info in entry["files"]:
        path = str(file_info["path"])
        add_file_to_index(
            repo,
            index_env,
            repo_payload / "snapshot" / path,
            f"{prefix}/snapshot/{path}",
            str(file_info.get("mode") or "100644"),
        )

    tree = git(repo, "write-tree", env=index_env)
    if not FULL_SHA.fullmatch(tree):
        raise RecoveryError(f"invalid recovery tree for {full}")

    fixed_env = {
        **index_env,
        "GIT_AUTHOR_NAME": "Alexander Mills",
        "GIT_AUTHOR_EMAIL": "alex@oresoftware.com",
        "GIT_AUTHOR_DATE": "2026-08-09T06:00:00Z",
        "GIT_COMMITTER_NAME": "Alexander Mills",
        "GIT_COMMITTER_EMAIL": "alex@oresoftware.com",
        "GIT_COMMITTER_DATE": "2026-08-09T06:00:00Z",
    }
    commit = output(
        ["git", "commit-tree", tree, "-p", current_main, "-m", "chore: preserve recovered StreamYard parity snapshot"],
        cwd=repo,
        env=fixed_env,
    )
    if not FULL_SHA.fullmatch(commit):
        raise RecoveryError(f"invalid recovery commit for {full}")

    changed = output(["git", "diff-tree", "--no-commit-id", "--name-only", "-r", commit], cwd=repo).splitlines()
    if not changed or any(not path.startswith(f"{ARCHIVE_DIR}/") for path in changed):
        raise RecoveryError(f"{full}: recovery commit touches non-recovery paths: {changed}")

    remote_check = run(["git", "ls-remote", "--heads", "origin", branch], cwd=repo)
    remote_line = remote_check.stdout.strip()
    pushed = False
    if remote_line:
        remote_sha = remote_line.split()[0]
        git(repo, "fetch", "-q", "--filter=blob:none", "--depth=1", "origin", f"refs/heads/{branch}")
        remote_tree = git(repo, "rev-parse", "FETCH_HEAD^{tree}")
        if remote_tree != tree:
            raise RecoveryError(
                f"{full}: remote recovery branch exists with a different tree; refusing non-fast-forward overwrite"
            )
        branch_sha = remote_sha
    else:
        git(repo, "push", "origin", f"{commit}:refs/heads/{branch}")
        branch_sha = commit
        pushed = True

    counts = {
        key: sum(c.status == key for c in classes)
        for key in (
            "already-integrated-exactly",
            "absent-from-current-main",
            "diverged-or-superseded",
        )
    }
    result: dict[str, Any] = {
        "repository": full,
        "branch": branch,
        "branch_sha": branch_sha,
        "current_main_sha": current_main,
        "original_main_sha": entry["original_main_sha"],
        "original_feature_sha": entry["original_feature_sha"],
        "pushed": pushed,
        "classification_counts": counts,
        "files": [c.__dict__ for c in classes],
    }
    pr_number, pr_url = create_pr(full, branch, result)
    result["pull_request_number"] = pr_number
    result["pull_request_url"] = pr_url
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--payload-root", type=Path, required=True)
    parser.add_argument("--report-json", type=Path, required=True)
    parser.add_argument("--branch", default=DEFAULT_BRANCH)
    args = parser.parse_args()

    if args.branch != DEFAULT_BRANCH:
        raise RecoveryError("unexpected recovery branch")
    payload_root = args.payload_root.resolve()
    manifest_path = payload_root / "manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 2:
        raise RecoveryError("unsupported recovery payload schema")
    entries = manifest.get("repositories")
    if not isinstance(entries, list) or len(entries) != 8:
        raise RecoveryError("expected exactly eight StreemPilot repositories")

    with tempfile.TemporaryDirectory(prefix="streempilot-recovery-") as tmp:
        work_root = Path(tmp)
        results = [recover_repository(payload_root, entry, work_root, args.branch) for entry in entries]

    report = {
        "schema_version": 1,
        "state": "completed",
        "branch": args.branch,
        "repository_count": len(results),
        "pull_request_count": len(results),
        "pushed_count": sum(bool(r["pushed"]) for r in results),
        "repositories": results,
    }
    if report["repository_count"] != 8 or report["pull_request_count"] != 8:
        raise RecoveryError("recovery count invariant failed")
    args.report_json.parent.mkdir(parents=True, exist_ok=True)
    args.report_json.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "state": "completed",
        "repositories": report["repository_count"],
        "pull_requests": report["pull_request_count"],
        "pushed": report["pushed_count"],
    }, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RecoveryError, subprocess.CalledProcessError, OSError, json.JSONDecodeError) as exc:
        print(f"RECOVERY_FAILED: {exc}", file=sys.stderr)
        if isinstance(exc, subprocess.CalledProcessError):
            if exc.stdout:
                print(exc.stdout, file=sys.stderr)
            if exc.stderr:
                print(exc.stderr, file=sys.stderr)
        raise SystemExit(1)
