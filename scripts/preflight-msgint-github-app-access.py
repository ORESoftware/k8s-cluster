#!/usr/bin/env python3
"""Verify a short-lived GitHub App token can read one immutable private revision."""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.error
import urllib.request
from collections.abc import Callable
from typing import Any

API_BASE = "https://api.github.com"
MAX_RESPONSE_BYTES = 1024 * 1024
MAX_TOKEN_BYTES = 4096
REVISION_RE = re.compile(r"[a-f0-9]{40}")


class PreflightError(RuntimeError):
    """A redacted, operator-actionable access failure."""


class NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # noqa: ANN001
        return None


def _default_opener(request: urllib.request.Request, timeout: int):
    return urllib.request.build_opener(NoRedirectHandler()).open(request, timeout=timeout)


def _validate_inputs(repository: str, revision: str, token: str) -> None:
    if repository != "messaging-intel/msgint-connectors":
        raise PreflightError("unexpected repository identity")
    if REVISION_RE.fullmatch(revision) is None:
        raise PreflightError("MSGINT_REVISION must be a lowercase 40-hex commit")
    if not token or len(token.encode("utf-8")) > MAX_TOKEN_BYTES:
        raise PreflightError("GitHub App token is missing or exceeds the byte limit")
    if any(character.isspace() or character.iscontrol() for character in token):
        raise PreflightError("GitHub App token must be a single printable value")


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
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with opener(request, 15) as response:
            status = getattr(response, "status", None)
            if status != 200:
                raise PreflightError(f"{label} returned unexpected HTTP status")
            body = response.read(MAX_RESPONSE_BYTES + 1)
    except urllib.error.HTTPError as error:
        raise PreflightError(
            f"{label} returned HTTP {error.code}; install the K8S_SUBMODULE GitHub App "
            "on messaging-intel/msgint-connectors with repository contents read permission"
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
    if repo.get("full_name") != repository:
        raise PreflightError("GitHub returned an unexpected repository identity")

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
    repository = "messaging-intel/msgint-connectors"
    revision = os.environ.get("MSGINT_REVISION", "")
    token = os.environ.get("MSGINT_GITHUB_TOKEN", "")
    try:
        verify_access(repository, revision, token)
    except PreflightError as error:
        print(f"::error title=Messaging Intel GitHub App preflight failed::{error}")
        return 1
    print(f"preflight=ok repository={repository} revision={revision}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
