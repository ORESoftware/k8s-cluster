#!/usr/bin/env python3
"""Run the exact DEN-2328 private-repository publisher with bounded retries.

The credential is read from a mode-0600 file created by the run-bound RSA
handoff. It is never accepted as a command-line argument and never written to
result evidence. Whole-organization retries are safe because the underlying
publisher is create-only and verifies preserved repository IDs and main heads.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from typing import Any, Mapping

API = "https://api.github.com"
EXPECTED_LOGIN = "ORESoftware"
EXPECTED_REPOSITORIES = {
    "StreemPilot/streempilot-media-router.rs",
    "hypesiege/hypesiege-analytics.rs",
    "hypesiege/hypesiege-publishing-worker.rs",
    "hypesiege/hypesiege-scheduler.rs",
}
ORGANIZATIONS = ("hypesiege", "StreemPilot")
MAX_ATTEMPTS = 8
TRANSIENT_STATUSES = frozenset({403, 429, 500, 502, 503, 504})
TRANSIENT_MARKERS = (
    "secondary rate limit",
    "rate limit",
    "abuse detection",
    "temporarily unavailable",
    "service unavailable",
    "internal server error",
    "bad gateway",
    "gateway timeout",
    "connection reset",
    "remote end hung up",
    "timed out",
    "timeout",
    "http 429",
    "http 500",
    "http 502",
    "http 503",
    "http 504",
)
SHA_RE = re.compile(r"^[0-9a-f]{40}$")


class GitHubRequestError(RuntimeError):
    def __init__(self, status: int, path: str, message: str):
        super().__init__(f"GitHub API {status} for GET {path}: {message[:500]}")
        self.status = status
        self.path = path
        self.message = message[:500]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--token-file", required=True, type=Path)
    parser.add_argument("--evidence-dir", required=True, type=Path)
    parser.add_argument("--result", required=True, type=Path)
    return parser.parse_args()


def is_transient_failure(text: str, status: int | None = None) -> bool:
    normalized = text.casefold()
    if status is not None and status not in TRANSIENT_STATUSES:
        return False
    if status in {429, 500, 502, 503, 504}:
        return True
    return any(marker in normalized for marker in TRANSIENT_MARKERS)


def retry_delay(attempt: int, headers: Mapping[str, str] | None = None) -> int:
    if headers:
        retry_after = headers.get("Retry-After") or headers.get("retry-after")
        if retry_after and retry_after.isdigit():
            return max(5, min(int(retry_after), 180))
        reset = headers.get("X-RateLimit-Reset") or headers.get("x-ratelimit-reset")
        if reset and reset.isdigit():
            return max(5, min(int(reset) - int(time.time()) + 5, 180))
    return min(15 * (2 ** max(attempt - 1, 0)), 180)


def api_get_json(token: str, path: str) -> dict[str, Any]:
    for attempt in range(1, MAX_ATTEMPTS + 1):
        request = urllib.request.Request(API + path, method="GET")
        request.add_header("Accept", "application/vnd.github+json")
        request.add_header("Authorization", f"Bearer {token}")
        request.add_header("X-GitHub-Api-Version", "2022-11-28")
        request.add_header("User-Agent", "den-2328-exact-gap-runner")
        try:
            with urllib.request.urlopen(request, timeout=45) as response:
                payload = json.loads(response.read())
                if not isinstance(payload, dict):
                    raise RuntimeError(f"invalid GitHub response for {path}")
                return payload
        except urllib.error.HTTPError as error:
            raw = error.read(4096).decode(errors="replace")
            try:
                message = str(json.loads(raw).get("message", raw))
            except json.JSONDecodeError:
                message = raw
            if (
                attempt < MAX_ATTEMPTS
                and error.code in TRANSIENT_STATUSES
                and is_transient_failure(message, error.code)
            ):
                delay = retry_delay(attempt, error.headers)
                print(
                    f"RETRY_GITHUB_API path={path} status={error.code} "
                    f"attempt={attempt}/{MAX_ATTEMPTS} sleep={delay}s",
                    flush=True,
                )
                time.sleep(delay)
                continue
            raise GitHubRequestError(error.code, path, message) from error
        except urllib.error.URLError as error:
            if attempt < MAX_ATTEMPTS and is_transient_failure(str(error)):
                delay = retry_delay(attempt)
                print(
                    f"RETRY_GITHUB_TRANSPORT path={path} "
                    f"attempt={attempt}/{MAX_ATTEMPTS} sleep={delay}s",
                    flush=True,
                )
                time.sleep(delay)
                continue
            raise RuntimeError(f"GitHub transport failure for {path}: {error}") from error
    raise AssertionError("unreachable")


def run_organization(
    organization: str,
    evidence_path: Path,
    token: str,
) -> None:
    command = [
        sys.executable,
        "scripts/ops/publish_exact_private_repository_gaps.py",
        "--organization",
        organization,
        "--evidence-out",
        str(evidence_path),
    ]
    environment = os.environ.copy()
    environment["GH_TOKEN"] = token
    environment["GITHUB_REPOSITORY_ADMIN_TOKEN"] = token

    for attempt in range(1, MAX_ATTEMPTS + 1):
        completed = subprocess.run(
            command,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=False,
        )
        safe_output = completed.stdout.replace(token, "***")
        if safe_output:
            print(safe_output, end="" if safe_output.endswith("\n") else "\n")
        if completed.returncode == 0:
            return
        if attempt < MAX_ATTEMPTS and is_transient_failure(safe_output):
            delay = retry_delay(attempt)
            print(
                f"RETRY_EXACT_PUBLISHER organization={organization} "
                f"attempt={attempt}/{MAX_ATTEMPTS} sleep={delay}s",
                flush=True,
            )
            time.sleep(delay)
            continue
        raise RuntimeError(
            f"exact publisher failed for {organization} with exit code "
            f"{completed.returncode}"
        )


def load_evidence(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise RuntimeError(f"invalid evidence document: {path}")
    return payload


def combine_evidence(
    documents: list[dict[str, Any]],
    *,
    authenticated_login: str,
) -> dict[str, Any]:
    if {document.get("organization") for document in documents} != set(ORGANIZATIONS):
        raise RuntimeError("organization evidence set is not exact")

    repositories: list[dict[str, Any]] = []
    for document in documents:
        if document.get("schema_version") != 1:
            raise RuntimeError("unexpected organization evidence schema")
        if document.get("sealed_source_repository") != "ORESoftware/ai-agent-coordinator.rs":
            raise RuntimeError("sealed source repository changed")
        if document.get("sealed_source_sha") != "5d9a0c2cb44dff607bc3953954ce4b9af08e5789":
            raise RuntimeError("sealed source SHA changed")
        records = document.get("repositories")
        if not isinstance(records, list):
            raise RuntimeError("organization evidence repositories are malformed")
        repositories.extend(records)

    names = {record.get("full_name") for record in repositories}
    if names != EXPECTED_REPOSITORIES or len(repositories) != 4:
        raise RuntimeError(
            f"exact repository evidence mismatch: {sorted(str(name) for name in names)}"
        )

    created = 0
    preserved = 0
    for record in repositories:
        full_name = record.get("full_name")
        if record.get("visibility") != "private":
            raise RuntimeError(f"repository is not private: {full_name}")
        if record.get("default_branch") != "main":
            raise RuntimeError(f"repository default branch is not main: {full_name}")
        repository_id = record.get("repository_id")
        if not isinstance(repository_id, int) or repository_id <= 0:
            raise RuntimeError(f"invalid repository ID: {full_name}")
        main_sha = record.get("main_sha")
        expected_sha = record.get("expected_sealed_sha")
        if not isinstance(main_sha, str) or not SHA_RE.fullmatch(main_sha):
            raise RuntimeError(f"invalid main SHA: {full_name}")
        if not isinstance(expected_sha, str) or not SHA_RE.fullmatch(expected_sha):
            raise RuntimeError(f"invalid sealed SHA: {full_name}")
        if main_sha != expected_sha:
            raise RuntimeError(
                f"repository does not match sealed history: {full_name} "
                f"{main_sha} != {expected_sha}"
            )
        disposition = record.get("disposition")
        if disposition == "created":
            created += 1
        elif disposition == "preserved":
            preserved += 1
        else:
            raise RuntimeError(f"invalid repository disposition: {full_name}")

    repositories.sort(key=lambda record: str(record["full_name"]).casefold())
    return {
        "schema_version": 1,
        "authenticated_login": authenticated_login,
        "sealed_source_repository": "ORESoftware/ai-agent-coordinator.rs",
        "sealed_source_sha": "5d9a0c2cb44dff607bc3953954ce4b9af08e5789",
        "expected_repository_count": 4,
        "summary": {
            "created": created,
            "preserved_exact": preserved,
            "verified": len(repositories),
            "failures": 0,
        },
        "repositories": repositories,
    }


def read_token(path: Path) -> str:
    token = path.read_text(encoding="utf-8").strip()
    if len(token) < 20 or any(character.isspace() for character in token):
        raise RuntimeError("credential file has an invalid token shape")
    return token


def main() -> int:
    args = parse_args()
    token = read_token(args.token_file)
    args.evidence_dir.mkdir(parents=True, exist_ok=True)

    profile = api_get_json(token, "/user")
    login = profile.get("login")
    if not isinstance(login, str) or login.casefold() != EXPECTED_LOGIN.casefold():
        raise RuntimeError(f"unexpected authenticated GitHub login: {login!r}")
    print(f"VERIFIED_GITHUB_IDENTITY login={login}")

    evidence_paths: list[Path] = []
    for organization in ORGANIZATIONS:
        path = args.evidence_dir / f"{organization.casefold()}-exact-private-gaps.json"
        run_organization(organization, path, token)
        evidence_paths.append(path)

    combined = combine_evidence(
        [load_evidence(path) for path in evidence_paths],
        authenticated_login=login,
    )
    args.result.parent.mkdir(parents=True, exist_ok=True)
    args.result.write_text(
        json.dumps(combined, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    token = ""
    print(
        "VERIFIED_ENCRYPTED_EXACT_PRIVATE_GAPS "
        f"created={combined['summary']['created']} "
        f"preserved_exact={combined['summary']['preserved_exact']} total=4"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
