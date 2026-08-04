#!/usr/bin/env python3
"""Create canonical-cloud/canonical-docs and publish its exact reviewed refs."""

from __future__ import annotations

import argparse
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Sequence

API_ROOT = "https://api.github.com"
PUBLISHER_LOGIN = "ORESoftware"
TARGET_OWNER = "canonical-cloud"
TARGET_NAME = "canonical-docs"
TARGET_REPOSITORY = f"{TARGET_OWNER}/{TARGET_NAME}"
DESCRIPTION = "Canonical Cloud strategy, compliance, trust, and operating documentation"
MAIN_REF = "main"
MAIN_SHA = "1848835599049ca41f68a079b5ac04f7d360fe87"
FEATURE_REF = "agent/den-1049-repository-baseline"
FEATURE_SHA = "54aa2efcbcfd21020614cbecccea5a907ead813f"
PR_TITLE = "Establish Canonical documentation governance"
PR_BODY = """## Summary

- publish the substantive Canonical Cloud business plan from the preserved initial commit;
- add an intentional licensing posture pending DEN-621;
- add evidence/claim governance, security, and contribution boundaries;
- standardize lowercase `agents.md` with minimal alternate pointers;
- add hermetic Markdown, link, structure, and claim-boundary CI;
- call the immutable Canonical Cloud hierarchy validator.

## Evidence boundary

The repository and business plan remain public planning material. They do not claim that Canonical Cloud is certified, authorized, production-deployed, independently audited, or guaranteed to achieve the stated targets. Financial values, pricing, milestones, and growth outcomes remain management assumptions to validate.

## Validation

- `python3 scripts/check_docs.py`
- `Documentation contract` GitHub Actions workflow
- `Agent instruction hierarchy` reusable workflow

Refs DEN-1049
Related: DEN-319, DEN-621, DEN-127
"""


class PublishError(RuntimeError):
    """A fail-closed repository publication invariant was violated."""


def fail(message: str) -> None:
    raise PublishError(message)


def token_from_environment() -> str:
    token = os.environ.get("GITHUB_REPOSITORY_ADMIN_TOKEN") or os.environ.get("GH_TOKEN")
    if not token or any(character.isspace() for character in token):
        fail("a non-whitespace repository-administration token is required")
    if not (token.startswith("ghp_") or token.startswith("github_pat_")):
        fail("repository-administration token has an unsupported shape")
    return token


def api(
    token: str,
    method: str,
    path: str,
    body: dict[str, object] | None = None,
    *,
    allow_not_found: bool = False,
) -> tuple[int, object | None]:
    payload = None if body is None else json.dumps(body).encode("utf-8")
    request = urllib.request.Request(API_ROOT + path, data=payload, method=method)
    request.add_header("Accept", "application/vnd.github+json")
    request.add_header("Authorization", f"Bearer {token}")
    request.add_header("X-GitHub-Api-Version", "2022-11-28")
    request.add_header("User-Agent", "canonical-docs-protected-publisher")
    if payload is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            raw = response.read()
            return response.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        raw = error.read(4096).decode(errors="replace")
        if allow_not_found and error.code == 404:
            return 404, None
        fail(f"GitHub API {error.code} for {method} {path}: {raw}")


def require_object(value: object | None, label: str) -> dict[str, object]:
    if not isinstance(value, dict):
        fail(f"{label} response is not an object")
    return value


def verify_identity(token: str) -> None:
    status, identity_value = api(token, "GET", "/user")
    if status != 200:
        fail(f"identity preflight returned HTTP {status}")
    identity = require_object(identity_value, "identity")
    if identity.get("login") != PUBLISHER_LOGIN:
        fail(f"unexpected publisher identity: {identity.get('login')!r}")

    status, membership_value = api(
        token, "GET", f"/user/memberships/orgs/{TARGET_OWNER}"
    )
    if status != 200:
        fail(f"organization membership preflight returned HTTP {status}")
    membership = require_object(membership_value, "membership")
    observed = (membership.get("role"), membership.get("state"))
    if observed != ("admin", "active"):
        fail(f"{TARGET_OWNER} owner membership is {observed!r}")


def repository_payload() -> dict[str, object]:
    return {
        "name": TARGET_NAME,
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
    }


def validate_repository(repository: dict[str, object]) -> None:
    if repository.get("full_name") != TARGET_REPOSITORY:
        fail(f"repository identity mismatch: {repository.get('full_name')!r}")
    owner = repository.get("owner")
    if not isinstance(owner, dict) or owner.get("login") != TARGET_OWNER:
        fail("repository owner mismatch")
    if repository.get("visibility") != "public" or repository.get("private") is not False:
        fail("canonical-docs must be public")
    if repository.get("archived") is not False or repository.get("disabled") is not False:
        fail("canonical-docs must be active")


def ensure_repository(token: str) -> tuple[dict[str, object], bool]:
    status, current = api(
        token, "GET", f"/repos/{TARGET_REPOSITORY}", allow_not_found=True
    )
    created = False
    if status == 404:
        status, current = api(
            token, "POST", f"/orgs/{TARGET_OWNER}/repos", repository_payload()
        )
        if status != 201:
            fail(f"repository creation returned HTTP {status}")
        created = True
    elif status != 200:
        fail(f"repository lookup returned HTTP {status}")
    repository = require_object(current, "repository")
    validate_repository(repository)
    return repository, created


def ref_path(ref: str) -> str:
    return f"/repos/{TARGET_REPOSITORY}/git/ref/heads/{urllib.parse.quote(ref, safe='/')}"


def read_ref(token: str, ref: str) -> str | None:
    status, value = api(token, "GET", ref_path(ref), allow_not_found=True)
    if status == 404:
        return None
    if status != 200:
        fail(f"ref lookup returned HTTP {status}: {ref}")
    payload = require_object(value, f"ref {ref}")
    target = payload.get("object")
    if not isinstance(target, dict):
        fail(f"ref {ref} has no target object")
    sha = target.get("sha")
    if not isinstance(sha, str) or re.fullmatch(r"[0-9a-f]{40}", sha) is None:
        fail(f"ref {ref} has an invalid SHA")
    return sha


def run(args: Sequence[str], *, env: dict[str, str] | None = None) -> str:
    completed = subprocess.run(
        list(args),
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if completed.returncode != 0:
        fail(
            f"command failed ({completed.returncode}): {' '.join(args)}\n"
            f"{completed.stdout[:4000]}"
        )
    return completed.stdout


def require_regular_bundle(bundle: Path) -> Path:
    try:
        metadata = bundle.lstat()
    except OSError as error:
        fail(f"cannot inspect bundle: {error}")
    if not stat.S_ISREG(metadata.st_mode):
        fail("bundle must be a regular file")
    try:
        return bundle.resolve(strict=True)
    except OSError as error:
        fail(f"cannot resolve bundle: {error}")


def prepare_source(bundle: Path, work: Path) -> Path:
    source = work / "source.git"
    run(["git", "clone", "--bare", str(bundle), str(source)])
    for ref, expected in ((MAIN_REF, MAIN_SHA), (FEATURE_REF, FEATURE_SHA)):
        actual = run(
            ["git", "--git-dir", str(source), "rev-parse", f"refs/heads/{ref}"]
        ).strip()
        if actual != expected:
            fail(f"local bundle ref mismatch for {ref}: {actual} != {expected}")
    return source


def git_environment(token: str, work: Path) -> dict[str, str]:
    askpass = work / "git-askpass.py"
    askpass.write_text(
        "#!/usr/bin/env python3\n"
        "import os, sys\n"
        "prompt = sys.argv[1] if len(sys.argv) > 1 else ''\n"
        "print('x-access-token' if 'Username' in prompt else os.environ['GH_TOKEN'])\n",
        encoding="utf-8",
    )
    askpass.chmod(0o700)
    environment = os.environ.copy()
    environment.update(
        {
            "GH_TOKEN": token,
            "GIT_ASKPASS": str(askpass),
            "GIT_ASKPASS_REQUIRE": "force",
            "GIT_TERMINAL_PROMPT": "0",
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "credential.helper",
            "GIT_CONFIG_VALUE_0": "",
        }
    )
    return environment


def ensure_ref(
    token: str,
    source: Path,
    environment: dict[str, str],
    ref: str,
    expected: str,
) -> str:
    current = read_ref(token, ref)
    if current is not None:
        if current != expected:
            fail(f"refusing to overwrite {ref}: {current} != {expected}")
        print(f"VERIFIED_REF {ref} {current}")
        return current

    remote = f"https://github.com/{TARGET_REPOSITORY}.git"
    run(
        [
            "git",
            "--git-dir",
            str(source),
            "push",
            remote,
            f"{expected}:refs/heads/{ref}",
        ],
        env=environment,
    )
    current = read_ref(token, ref)
    if current != expected:
        fail(f"remote verification failed for {ref}: {current} != {expected}")
    print(f"PUSHED_REF {ref} {current}")
    return current


def enforce_repository_settings(token: str) -> dict[str, object]:
    payload = repository_payload().copy()
    payload.pop("name")
    payload.pop("private")
    payload.pop("auto_init")
    payload["default_branch"] = MAIN_REF
    status, value = api(token, "PATCH", f"/repos/{TARGET_REPOSITORY}", payload)
    if status != 200:
        fail(f"repository settings update returned HTTP {status}")
    repository = require_object(value, "updated repository")
    validate_repository(repository)
    if repository.get("default_branch") != MAIN_REF:
        fail("canonical-docs default branch is not main")
    if repository.get("allow_rebase_merge") is not False:
        fail("rebase merging must remain disabled")
    if repository.get("delete_branch_on_merge") is not True:
        fail("merged feature branches must be deleted")
    return repository


def ensure_pull_request(token: str) -> dict[str, object]:
    query = urllib.parse.urlencode(
        {
            "state": "all",
            "head": f"{TARGET_OWNER}:{FEATURE_REF}",
            "base": MAIN_REF,
            "per_page": "100",
        }
    )
    status, value = api(token, "GET", f"/repos/{TARGET_REPOSITORY}/pulls?{query}")
    if status != 200 or not isinstance(value, list):
        fail("pull request lookup returned an invalid response")
    if len(value) > 1:
        fail("multiple pull requests exist for the exact feature branch")
    if value:
        pull = require_object(value[0], "pull request")
        if pull.get("merged_at") is not None:
            return pull
        if pull.get("state") == "closed":
            number = pull.get("number")
            if not isinstance(number, int):
                fail("closed pull request has no numeric identifier")
            status, reopened = api(
                token,
                "PATCH",
                f"/repos/{TARGET_REPOSITORY}/pulls/{number}",
                {"state": "open"},
            )
            if status != 200:
                fail(f"pull request reopen returned HTTP {status}")
            pull = require_object(reopened, "reopened pull request")
        return pull

    status, created = api(
        token,
        "POST",
        f"/repos/{TARGET_REPOSITORY}/pulls",
        {
            "title": PR_TITLE,
            "head": FEATURE_REF,
            "base": MAIN_REF,
            "body": PR_BODY,
            "draft": False,
            "maintainer_can_modify": True,
        },
    )
    if status != 201:
        fail(f"pull request creation returned HTTP {status}")
    return require_object(created, "created pull request")


def publish(bundle: Path) -> dict[str, object]:
    token = token_from_environment()
    verify_identity(token)
    repository, created = ensure_repository(token)

    with tempfile.TemporaryDirectory(prefix="canonical-docs-publisher-") as temporary:
        work = Path(temporary)
        source = prepare_source(require_regular_bundle(bundle), work)
        environment = git_environment(token, work)
        main = ensure_ref(token, source, environment, MAIN_REF, MAIN_SHA)
        feature = ensure_ref(token, source, environment, FEATURE_REF, FEATURE_SHA)

    repository = enforce_repository_settings(token)
    pull = ensure_pull_request(token)
    number = pull.get("number")
    html_url = pull.get("html_url")
    if not isinstance(number, int) or not isinstance(html_url, str):
        fail("pull request response lacks a stable identifier or URL")
    head = pull.get("head")
    base = pull.get("base")
    if (
        not isinstance(head, dict)
        or head.get("ref") != FEATURE_REF
        or not isinstance(base, dict)
        or base.get("ref") != MAIN_REF
    ):
        fail("pull request ref contract mismatch")

    return {
        "repository": TARGET_REPOSITORY,
        "repository_url": repository.get("html_url"),
        "created": created,
        "visibility": repository.get("visibility"),
        "default_branch": repository.get("default_branch"),
        "main_sha": main,
        "feature_ref": FEATURE_REF,
        "feature_sha": feature,
        "pull_request_number": number,
        "pull_request_url": html_url,
        "pull_request_state": pull.get("state"),
        "pull_request_merged": pull.get("merged_at") is not None,
    }


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--json-report", type=Path)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        report = publish(args.bundle)
        if args.json_report is not None:
            args.json_report.write_text(
                json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
    except (PublishError, OSError, UnicodeError) as error:
        print(f"canonical-docs publication failed: {error}", file=sys.stderr)
        return 1
    print(
        "canonical-docs publication: PASS "
        f"repository={report['repository']} main={report['main_sha']} "
        f"feature={report['feature_sha']} pr={report['pull_request_number']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
