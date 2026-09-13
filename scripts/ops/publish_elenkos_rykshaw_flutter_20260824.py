#!/usr/bin/env python3
"""Publish the validated Rykshaw Flutter source to a review branch and PR.

The caller supplies a mode-0600 GitHub token file obtained through an ephemeral
RSA handoff. The token is never serialized into evidence or embedded in a remote
URL. Application source is never written directly to the target main branch.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path, PurePosixPath
from typing import Any, Iterable

API_ROOT = "https://api.github.com"
EXPECTED_LOGIN = "ORESoftware"
EXPECTED_ORG = "elenkos-systems"
EXPECTED_REPOSITORY = "elenkos-systems/elenkos-rykshaw-flutter"
EXPECTED_BRANCH = "den-3837/rykshaw-volunteer-qa-beta"
EXPECTED_SOURCE_ROOT = "elenkos-rykshaw-flutter"


class PublicationError(RuntimeError):
    pass


class GitHubApi:
    def __init__(self, token: str) -> None:
        self._token = token

    def request(
        self,
        method: str,
        path: str,
        *,
        payload: dict[str, Any] | None = None,
        expected: Iterable[int] = (200,),
    ) -> tuple[int, Any]:
        if not path.startswith("/"):
            raise PublicationError(f"invalid GitHub API path: {path!r}")
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        request = urllib.request.Request(
            f"{API_ROOT}{path}",
            data=data,
            method=method,
        )
        request.add_header("Accept", "application/vnd.github+json")
        request.add_header("Authorization", f"Bearer {self._token}")
        request.add_header("X-GitHub-Api-Version", "2022-11-28")
        request.add_header("User-Agent", "elenkos-rykshaw-flutter-publisher/1")
        if data is not None:
            request.add_header("Content-Type", "application/json")
        expected_set = set(expected)
        try:
            with urllib.request.urlopen(request, timeout=45) as response:
                raw = response.read()
                body = json.loads(raw) if raw else None
                status = response.status
        except urllib.error.HTTPError as error:
            raw = error.read()
            try:
                body = json.loads(raw) if raw else None
            except json.JSONDecodeError:
                body = {"message": raw.decode("utf-8", errors="replace")[:1000]}
            status = error.code
        if status not in expected_set:
            message = body.get("message") if isinstance(body, dict) else repr(body)
            raise PublicationError(
                f"GitHub API {method} {path} returned {status}: {message}"
            )
        return status, body


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_and_validate_source(source_archive: Path, source_meta: Path) -> dict[str, Any]:
    meta = json.loads(source_meta.read_text(encoding="utf-8"))
    required = {
        "schema": "oresoftware.normalized-source/v1",
        "target_repository": EXPECTED_REPOSITORY,
        "flutter_version": "3.47.0",
        "telemetry_ref": "cc9c9e0f3fe57f0b692d76a099a23970939559ab",
    }
    for key, expected in required.items():
        if meta.get(key) != expected:
            raise PublicationError(
                f"unexpected normalized-source metadata {key}: {meta.get(key)!r}"
            )
    expected_sha = meta.get("source_sha256")
    if not isinstance(expected_sha, str) or len(expected_sha) != 64:
        raise PublicationError("normalized source metadata lacks a SHA-256")
    actual_sha = sha256_file(source_archive)
    if actual_sha != expected_sha:
        raise PublicationError(
            f"normalized source SHA-256 mismatch: {actual_sha} != {expected_sha}"
        )
    return meta


def validate_member(member: tarfile.TarInfo) -> None:
    path = PurePosixPath(member.name)
    if path.is_absolute() or ".." in path.parts or not path.parts:
        raise PublicationError(f"unsafe archive path: {member.name!r}")
    if path.parts[0] != EXPECTED_SOURCE_ROOT:
        raise PublicationError(f"unexpected archive root: {member.name!r}")
    if member.issym() or member.islnk() or member.isdev() or member.isfifo():
        raise PublicationError(f"unsupported archive member type: {member.name!r}")
    if ".git" in path.parts:
        raise PublicationError(f"Git metadata is forbidden in source: {member.name!r}")
    if "env" in path.parts and "dec" in path.parts:
        raise PublicationError(f"decrypted environment content is forbidden: {member.name!r}")
    if path.name.endswith(".env") or path.name == ".env":
        raise PublicationError(f"plaintext environment file is forbidden: {member.name!r}")


def extract_source(source_archive: Path, destination: Path) -> Path:
    with tarfile.open(source_archive, "r:gz") as archive:
        members = archive.getmembers()
        if not members:
            raise PublicationError("normalized source archive is empty")
        for member in members:
            validate_member(member)
        archive.extractall(destination, members=members, filter="data")
    root = destination / EXPECTED_SOURCE_ROOT
    if not root.is_dir():
        raise PublicationError("normalized source root was not extracted")
    required = [
        root / "pubspec.yaml",
        root / "pubspec.lock",
        root / "lib" / "main.dart",
        root / ".zpkg.toml",
        root / ".zed" / "hooks" / "pre-build.sh",
    ]
    missing = [str(path.relative_to(root)) for path in required if not path.is_file()]
    if missing:
        raise PublicationError(f"normalized source is missing required files: {missing}")
    return root


def run_git(repo: Path, args: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    command = ["git", "-C", str(repo), *args]
    result = subprocess.run(
        command,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        stderr = result.stderr[-4000:]
        raise PublicationError(f"git {' '.join(args[:3])} failed: {stderr}")
    return result


def try_git(repo: Path, args: list[str], env: dict[str, str]) -> bool:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        env=env,
        text=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return result.returncode == 0


def remove_worktree_contents(repo: Path) -> None:
    for child in repo.iterdir():
        if child.name == ".git":
            continue
        if child.is_dir() and not child.is_symlink():
            shutil.rmtree(child)
        else:
            child.unlink()


def copy_source(source_root: Path, repo: Path) -> None:
    for child in source_root.iterdir():
        target = repo / child.name
        if child.is_dir():
            shutil.copytree(child, target, copy_function=shutil.copy2)
        else:
            shutil.copy2(child, target)


def build_git_env(token_file: Path, temp_root: Path) -> dict[str, str]:
    mode = stat.S_IMODE(token_file.stat().st_mode)
    if mode != 0o600:
        raise PublicationError(f"token file mode must be 0600, got {mode:04o}")
    askpass = temp_root / "git-askpass.sh"
    askpass.write_text(
        "#!/usr/bin/env bash\n"
        "set -euo pipefail\n"
        "case \"${1:-}\" in\n"
        "  *Username*) printf '%s\\n' 'x-access-token' ;;\n"
        "  *Password*) cat \"${RYKSHAW_TOKEN_FILE:?}\" ;;\n"
        "  *) exit 1 ;;\n"
        "esac\n",
        encoding="utf-8",
    )
    askpass.chmod(0o700)
    env = os.environ.copy()
    env.update(
        {
            "GIT_ASKPASS": str(askpass),
            "GIT_TERMINAL_PROMPT": "0",
            "RYKSHAW_TOKEN_FILE": str(token_file),
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "credential.useHttpPath",
            "GIT_CONFIG_VALUE_0": "true",
        }
    )
    for key in ("GIT_TRACE", "GIT_TRACE_CURL", "GIT_CURL_VERBOSE"):
        env.pop(key, None)
    return env


def ensure_repository(api: GitHubApi, owner: str, name: str) -> dict[str, Any]:
    status, repo = api.request("GET", f"/repos/{owner}/{name}", expected=(200, 404))
    if status == 404:
        _, repo = api.request(
            "POST",
            f"/orgs/{owner}/repos",
            payload={
                "name": name,
                "description": (
                    "Invite-only Flutter portal for optional community QA and "
                    "sandbox appreciation workflows"
                ),
                "private": True,
                "auto_init": True,
                "has_issues": True,
                "has_projects": True,
                "has_wiki": False,
            },
            expected=(201,),
        )
    if not isinstance(repo, dict):
        raise PublicationError("repository API returned an unexpected payload")
    if repo.get("full_name", "").lower() != f"{owner}/{name}".lower():
        raise PublicationError(f"unexpected repository identity: {repo.get('full_name')!r}")
    if repo.get("private") is not True:
        raise PublicationError("target repository must be private")
    api.request(
        "PATCH",
        f"/repos/{owner}/{name}",
        payload={
            "default_branch": "main",
            "has_wiki": False,
            "delete_branch_on_merge": True,
        },
        expected=(200,),
    )
    return repo


def wait_for_main(api: GitHubApi, owner: str, name: str) -> bool:
    for _ in range(20):
        status, _ = api.request(
            "GET",
            f"/repos/{owner}/{name}/git/ref/heads/main",
            expected=(200, 404, 409),
        )
        if status == 200:
            return True
        time.sleep(1)
    return False


def publish(
    *,
    source_root: Path,
    source_sha: str,
    meta: dict[str, Any],
    token_file: Path,
    repository: str,
    branch: str,
    evidence_out: Path,
) -> dict[str, Any]:
    owner, name = repository.split("/", 1)
    if owner != EXPECTED_ORG or repository != EXPECTED_REPOSITORY:
        raise PublicationError("publisher target is outside the approved Elenkos repository")
    if branch != EXPECTED_BRANCH:
        raise PublicationError("publisher branch is outside the approved review branch")

    token = token_file.read_text(encoding="utf-8").strip()
    if len(token) < 20 or any(character.isspace() for character in token):
        raise PublicationError("token file does not contain one valid credential")
    api = GitHubApi(token)
    _, user = api.request("GET", "/user", expected=(200,))
    if not isinstance(user, dict) or user.get("login") != EXPECTED_LOGIN:
        raise PublicationError(f"unexpected GitHub identity: {user.get('login')!r}")
    _, membership = api.request(
        "GET",
        f"/user/memberships/orgs/{owner}",
        expected=(200,),
    )
    if (
        not isinstance(membership, dict)
        or membership.get("state") != "active"
        or membership.get("role") != "admin"
    ):
        raise PublicationError("credential is not an active organization administrator")

    repo_payload = ensure_repository(api, owner, name)

    with tempfile.TemporaryDirectory(prefix="rykshaw-publish-") as temporary:
        temp_root = Path(temporary)
        repo = temp_root / "repo"
        repo.mkdir()
        env = build_git_env(token_file, temp_root)
        run_git(repo, ["init", "--initial-branch=main"], env)
        run_git(repo, ["config", "user.name", "ORESoftware Automation"], env)
        run_git(repo, ["config", "user.email", "automation@oresoftware.com"], env)
        run_git(repo, ["remote", "add", "origin", f"https://github.com/{repository}.git"], env)

        main_exists = try_git(
            repo,
            ["fetch", "--no-tags", "origin", "refs/heads/main:refs/remotes/origin/main"],
            env,
        )
        if not main_exists and wait_for_main(api, owner, name):
            main_exists = try_git(
                repo,
                ["fetch", "--no-tags", "origin", "refs/heads/main:refs/remotes/origin/main"],
                env,
            )
        if not main_exists:
            run_git(repo, ["checkout", "--orphan", "main"], env)
            (repo / "README.md").write_text(
                "# Elenkos Rykshaw Flutter\n\n"
                "Application changes are reviewed through pull requests.\n",
                encoding="utf-8",
            )
            run_git(repo, ["add", "README.md"], env)
            run_git(repo, ["commit", "-m", "chore: initialize private repository"], env)
            run_git(repo, ["push", "origin", "HEAD:refs/heads/main"], env)
            run_git(
                repo,
                ["fetch", "--no-tags", "origin", "refs/heads/main:refs/remotes/origin/main"],
                env,
            )

        branch_ref = f"refs/heads/{branch}:refs/remotes/origin/{branch}"
        branch_exists = try_git(repo, ["fetch", "--no-tags", "origin", branch_ref], env)
        if branch_exists:
            run_git(repo, ["checkout", "-B", branch, f"origin/{branch}"], env)
        else:
            run_git(repo, ["checkout", "-B", branch, "origin/main"], env)

        remove_worktree_contents(repo)
        copy_source(source_root, repo)
        run_git(repo, ["add", "-A"], env)
        changed = not try_git(repo, ["diff", "--cached", "--quiet"], env)
        if changed:
            run_git(
                repo,
                ["commit", "-m", "feat: add Rykshaw volunteer QA Flutter beta"],
                env,
            )
        head_sha = run_git(repo, ["rev-parse", "HEAD"], env).stdout.strip()
        if len(head_sha) != 40:
            raise PublicationError("git did not produce a valid branch head")
        run_git(repo, ["push", "origin", f"HEAD:refs/heads/{branch}"], env)

    quoted_branch = urllib.parse.quote(branch, safe="")
    _, ref_payload = api.request(
        "GET",
        f"/repos/{owner}/{name}/git/ref/heads/{quoted_branch}",
        expected=(200,),
    )
    remote_sha = ref_payload.get("object", {}).get("sha") if isinstance(ref_payload, dict) else None
    if remote_sha != head_sha:
        raise PublicationError(f"remote branch mismatch: {remote_sha!r} != {head_sha}")

    query = urllib.parse.urlencode(
        {"state": "open", "base": "main", "head": f"{owner}:{branch}", "per_page": 100}
    )
    _, pulls = api.request("GET", f"/repos/{owner}/{name}/pulls?{query}", expected=(200,))
    title = "feat: add Rykshaw volunteer QA Flutter beta"
    body = (
        "## Summary\n\n"
        "Publishes the invite-only Elenkos Rykshaw Flutter beta as a review branch. "
        "The app provides QA assignments, finding/activity capture, versioned volunteer-program "
        "acknowledgements, optional foreground-only coarse location sharing, bounded remote "
        "experiences, sandbox appreciation intents, Rust read/write API contracts, ores-otel "
        "telemetry integration, and Zed package lifecycle gates.\n\n"
        "## Validated gates\n\n"
        "- Flutter 3.47.0 / Dart 3.13.0\n"
        "- strict analyzer with fatal informational diagnostics and warnings\n"
        "- 12 Flutter tests\n"
        "- Android debug APK build\n"
        f"- normalized source SHA-256: `{source_sha}`\n"
        f"- trusted validator commit: `{meta.get('validation_sha')}`\n\n"
        "## Release boundaries\n\n"
        "No app-store publication, real-money transfer, payroll execution, stored-value balance, "
        "background location tracking, or production tester enrollment is enabled by this PR. "
        "Legal, worker-classification, privacy, sanctions, tax, and payment-provider review remain "
        "release gates.\n\n"
        "Tracking: DEN-3837."
    )
    if not isinstance(pulls, list):
        raise PublicationError("pull request search returned an unexpected payload")
    if len(pulls) > 1:
        raise PublicationError("multiple open target pull requests found for the review branch")
    if pulls:
        number = pulls[0].get("number")
        _, pull = api.request(
            "PATCH",
            f"/repos/{owner}/{name}/pulls/{number}",
            payload={"title": title, "body": body, "base": "main"},
            expected=(200,),
        )
        created = False
    else:
        _, pull = api.request(
            "POST",
            f"/repos/{owner}/{name}/pulls",
            payload={
                "title": title,
                "body": body,
                "head": branch,
                "base": "main",
                "draft": False,
                "maintainer_can_modify": True,
            },
            expected=(201,),
        )
        created = True
    if not isinstance(pull, dict) or not pull.get("html_url"):
        raise PublicationError("pull request API returned an unexpected payload")

    evidence = {
        "schema_version": 1,
        "tracking_issue": "DEN-3837",
        "repository": repository,
        "repository_url": repo_payload.get("html_url"),
        "private": repo_payload.get("private"),
        "branch": branch,
        "head_sha": head_sha,
        "source_sha256": source_sha,
        "source_validation_sha": meta.get("validation_sha"),
        "pull_request_number": pull.get("number"),
        "pull_request_url": pull.get("html_url"),
        "pull_request_created": created,
        "commit_created": changed,
        "credential_identity": EXPECTED_LOGIN,
        "credential_exposed": False,
        "direct_main_application_commit": False,
    }
    evidence_out.parent.mkdir(parents=True, exist_ok=True)
    evidence_out.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    return evidence


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-archive", type=Path, required=True)
    parser.add_argument("--source-meta", type=Path, required=True)
    parser.add_argument("--token-file", type=Path)
    parser.add_argument("--repository", default=EXPECTED_REPOSITORY)
    parser.add_argument("--branch", default=EXPECTED_BRANCH)
    parser.add_argument("--evidence-out", type=Path, required=True)
    parser.add_argument("--validate-only", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    meta = load_and_validate_source(args.source_archive, args.source_meta)
    with tempfile.TemporaryDirectory(prefix="rykshaw-source-") as temporary:
        source_root = extract_source(args.source_archive, Path(temporary))
        if args.validate_only:
            result = {
                "schema_version": 1,
                "validated": True,
                "repository": args.repository,
                "branch": args.branch,
                "source_sha256": meta["source_sha256"],
                "file_count": sum(1 for path in source_root.rglob("*") if path.is_file()),
            }
            args.evidence_out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
            print(json.dumps(result, sort_keys=True))
            return 0
        if args.token_file is None:
            raise PublicationError("--token-file is required unless --validate-only is used")
        evidence = publish(
            source_root=source_root,
            source_sha=meta["source_sha256"],
            meta=meta,
            token_file=args.token_file,
            repository=args.repository,
            branch=args.branch,
            evidence_out=args.evidence_out,
        )
        print(json.dumps(evidence, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (PublicationError, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"publisher error: {error}", file=sys.stderr)
        raise SystemExit(1)
