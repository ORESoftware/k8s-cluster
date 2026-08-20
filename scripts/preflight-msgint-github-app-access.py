#!/usr/bin/env python3
"""Verify one short-lived GitHub App token can read one immutable private revision."""

from __future__ import annotations

import json
import os
import pathlib
import re
import stat
import sys
import urllib.error
import urllib.request
from collections.abc import Callable
from typing import Any

API_BASE = "https://api.github.com"
API_VERSION = "2026-03-10"
MAX_RESPONSE_BYTES = 1024 * 1024
MIN_TOKEN_BYTES = 20
MAX_TOKEN_BYTES = 4096
REVISION_RE = re.compile(r"[a-f0-9]{40}")
REPOSITORY = "messaging-intel/msgint-connectors"


class PreflightError(RuntimeError):
    """A redacted, operator-actionable access failure."""


class NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # noqa: ANN001
        return None


def _default_opener(request: urllib.request.Request, timeout: int):
    return urllib.request.build_opener(NoRedirectHandler()).open(request, timeout=timeout)


def _validate_token(token: str) -> None:
    token_bytes = len(token.encode("utf-8"))
    if token_bytes < MIN_TOKEN_BYTES or token_bytes > MAX_TOKEN_BYTES:
        raise PreflightError("GitHub App token is missing or outside the accepted byte bounds")
    if any(character.isspace() or not character.isprintable() for character in token):
        raise PreflightError("GitHub App token must be a single printable value")


def _read_token_file(raw_path: str) -> str:
    if not raw_path:
        raise PreflightError("MSGINT_GITHUB_TOKEN_FILE is required")
    path = pathlib.Path(raw_path)
    if not path.is_absolute() or ".." in path.parts:
        raise PreflightError("GitHub App token path must be absolute and traversal-free")
    try:
        metadata = path.lstat()
    except OSError:
        raise PreflightError("GitHub App token file is unavailable") from None
    if not stat.S_ISREG(metadata.st_mode):
        raise PreflightError("GitHub App token path must name a regular file")
    if metadata.st_mode & 0o077:
        raise PreflightError("GitHub App token file permissions are too broad")
    if metadata.st_size < MIN_TOKEN_BYTES or metadata.st_size > MAX_TOKEN_BYTES:
        raise PreflightError("GitHub App token file is outside the accepted byte bounds")
    try:
        token = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        raise PreflightError("GitHub App token file could not be read safely") from None
    _validate_token(token)
    return token


def _validate_inputs(repository: str, revision: str, token: str) -> None:
    if repository != REPOSITORY:
        raise PreflightError("unexpected repository identity")
    if REVISION_RE.fullmatch(revision) is None:
        raise PreflightError("MSGINT_REVISION must be a lowercase 40-hex commit")
    _validate_token(token)


def _github_json(
    path: str,
    *,
    token: str,
    label: str,
    opener: Callable[[urllib.request.Request, int], Any],
    api_base: str,
) -> dict[str, Any]:
    request = urllib.request.Request(
        f"{api_base}{path}",
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "User-Agent": "k8s-cluster-msgint-access-preflight",
            "X-GitHub-Api-Version": API_VERSION,
        },
    )
    try:
        with opener(request, 15) as response:
            status_code = getattr(response, "status", None)
            if status_code != 200:
                raise PreflightError(f"{label} returned unexpected HTTP status")
            body = response.read(MAX_RESPONSE_BYTES + 1)
    except urllib.error.HTTPError as error:
        raise PreflightError(
            f"{label} returned HTTP {error.code}; install the K8S_SUBMODULE GitHub App "
            f"on {REPOSITORY} with repository contents read permission"
        ) from None
    except urllib.error.URLError:
        raise PreflightError(f"{label} could not reach the GitHub API") from None
    except OSError:
        raise PreflightError(f"{label} failed before GitHub access could be verified") from None

    if len(body) > MAX_RESPONSE_BYTES:
        raise PreflightError(f"{label} response exceeded the byte limit")
    try:
        payload = json.loads(body)
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise PreflightError(f"{label} returned invalid JSON") from None
    if not isinstance(payload, dict):
        raise PreflightError(f"{label} returned an invalid object")
    return payload


def verify_access(
    repository: str,
    revision: str,
    token: str,
    *,
    opener: Callable[[urllib.request.Request, int], Any] = _default_opener,
    api_base: str = API_BASE,
) -> None:
    _validate_inputs(repository, revision, token)
    repo = _github_json(
        f"/repos/{repository}",
        token=token,
        label="repository lookup",
        opener=opener,
        api_base=api_base,
    )
    if repo.get("full_name") != repository or repo.get("private") is not True:
        raise PreflightError("GitHub returned an unexpected repository identity or visibility")

    commit = _github_json(
        f"/repos/{repository}/git/commits/{revision}",
        token=token,
        label="immutable commit lookup",
        opener=opener,
        api_base=api_base,
    )
    if commit.get("sha") != revision:
        raise PreflightError("GitHub returned an unexpected commit identity")


def main() -> int:
    revision = os.environ.get("MSGINT_REVISION", "")
    token_file = os.environ.get("MSGINT_GITHUB_TOKEN_FILE", "")
    try:
        token = _read_token_file(token_file)
        verify_access(REPOSITORY, revision, token)
    except PreflightError as error:
        print(f"::error title=Messaging Intel GitHub App preflight failed::{error}")
        return 1
    finally:
        if "token" in locals():
            token = ""
    print(f"preflight=ok repository={REPOSITORY} revision={revision}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
