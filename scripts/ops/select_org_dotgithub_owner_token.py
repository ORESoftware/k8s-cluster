#!/usr/bin/env python3
"""Select a protected GitHub owner credential for the fixed .github fleet.

The selector reads a JSON secret from stdin, recursively finds GitHub-token-like
string fields, and validates candidates without printing credential values. A
candidate is accepted only when it authenticates as ORESoftware and reports an
active admin membership in every organization in the bounded fleet.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator
import hashlib
import json
import re
import sys
from typing import Any
import urllib.error
import urllib.parse
import urllib.request

ORGANIZATIONS: tuple[str, ...] = (
    "channelsiege",
    "OmniBlitz",
    "streamkore",
    "hypeblitz",
    "3FA-app",
    "messaging-intel",
    "akrion-sim",
    "athlet-o",
    "benefactor-cc",
    "canonical-cloud",
    "claritas-viz",
    "cliptown",
    "daedalus-fab",
    "declarative-migrations",
    "fiducia-cloud",
    "anticaptrad",
    "opto-sync",
    "quaestor-ledger",
    "sagitta-stack",
    "shared-auth",
    "scintilla-run",
    "rust-ssr-demos",
    "sonus-auris",
    "usa-acc",
    "voxletra",
    "zed-pkg",
    "zed-pkg-test",
    "memebank",
    "meta-agents-demo",
    "networking-components",
    "StreemPilot",
    "unreal-unity-poc",
    "file-tunnel",
    "hypesiege",
    "discrete-event-systems",
    "drone-mngr",
)

EXPECTED_LOGIN = "ORESoftware"
TOKEN_NAME_PATTERN = re.compile(r"(?:github|gh|token|pat)", re.IGNORECASE)
REJECTED_TOKEN_SHA256: frozenset[str] = frozenset(
    {"777160bba7726b4740510c570437121769c3c2ed18d7c3ee06ff060b304f0fca"}
)
PREFERRED_LEAF_NAMES: dict[str, int] = {
    "GH_PAT": 0,
    "GITHUB_PAT": 1,
    "GITHUB_TOKEN": 2,
    "GH_TOKEN": 3,
}

JsonRequester = Callable[[str, str], tuple[int | None, Any]]


class CredentialSelectionError(RuntimeError):
    """A safe, value-free credential-selection failure."""


def _valid_token(value: str) -> bool:
    if not value or any(character.isspace() for character in value):
        return False
    fingerprint = hashlib.sha256(value.encode("utf-8")).hexdigest()
    return fingerprint not in REJECTED_TOKEN_SHA256


def iter_candidates(value: Any, path: tuple[str, ...] = ()) -> Iterator[tuple[str, str]]:
    """Yield recursively discovered token-like string fields."""

    if isinstance(value, dict):
        for key, child in value.items():
            yield from iter_candidates(child, path + (str(key),))
        return
    if isinstance(value, list):
        for index, child in enumerate(value):
            yield from iter_candidates(child, path + (str(index),))
        return
    if not isinstance(value, str) or not path:
        return

    name = ".".join(path)
    if TOKEN_NAME_PATTERN.search(name) and _valid_token(value):
        yield name, value


def request_json(token: str, path: str) -> tuple[int | None, Any]:
    """Perform a bounded GitHub API GET while discarding error bodies."""

    request = urllib.request.Request(
        "https://api.github.com" + path,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "org-dotgithub-protected-credential-selector",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        error.read(4096)
        return error.code, None
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError):
        return None, None


def _candidate_priority(name: str) -> tuple[int, str]:
    leaf = name.rsplit(".", 1)[-1].upper()
    return PREFERRED_LEAF_NAMES.get(leaf, 100), name


def select_owner_admin_token(
    payload: Any,
    *,
    requester: JsonRequester = request_json,
) -> tuple[str, str]:
    """Return ``(field_name, token)`` for a fully validated fleet owner token."""

    candidates = sorted(iter_candidates(payload), key=lambda item: _candidate_priority(item[0]))
    seen_tokens: set[str] = set()

    for name, token in candidates:
        if token in seen_tokens:
            continue
        seen_tokens.add(token)

        status, user = requester(token, "/user")
        if status != 200 or not isinstance(user, dict) or user.get("login") != EXPECTED_LOGIN:
            continue

        owns_entire_fleet = True
        for organization in ORGANIZATIONS:
            encoded_organization = urllib.parse.quote(organization, safe="")
            membership_status, membership = requester(
                token,
                f"/user/memberships/orgs/{encoded_organization}",
            )
            if not (
                membership_status == 200
                and isinstance(membership, dict)
                and membership.get("role") == "admin"
                and membership.get("state") == "active"
            ):
                owns_entire_fleet = False
                break

        if owns_entire_fleet:
            return name, token

    raise CredentialSelectionError(
        "no protected GitHub credential authenticates as the active owner-admin "
        "for the fixed 36-organization fleet"
    )


def main() -> int:
    try:
        payload = json.load(sys.stdin)
        _name, token = select_owner_admin_token(payload)
    except (json.JSONDecodeError, OSError, CredentialSelectionError):
        return 65

    sys.stdout.write(token)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
