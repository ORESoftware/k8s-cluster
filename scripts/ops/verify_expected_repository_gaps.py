#!/usr/bin/env python3
"""Fail-closed pre/post verification for the reviewed repository-gap publisher.

This module is deliberately read-only. It proves that the exact reviewed four-
repository gap set is absent before publication, then proves that those gaps are
private `main` repositories and every pre-existing reviewed repository retained
its repository ID and exact `main` SHA afterward.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import re
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

API = "https://api.github.com"
MAX_RESPONSE_BYTES = 4 * 1024 * 1024
SOURCE_REPOSITORY = "ORESoftware/ai-agent-coordinator.rs"
SOURCE_SHA = "5d9a0c2cb44dff607bc3953954ce4b9af08e5789"
MANIFEST_PATH = "repository-fleets/hypesiege-streempilot.json"
GENERATOR_SHA256 = "a57b00961ee57ae09bf3bb2e2d09afbdd1ddbbbde832b027802f82a1fc5dfa84"
EXPECTED_MISSING = (
    "StreemPilot/streempilot-media-router.rs",
    "hypesiege/hypesiege-scheduler.rs",
    "hypesiege/hypesiege-publishing-worker.rs",
    "hypesiege/hypesiege-analytics.rs",
)
SHA_RE = re.compile(r"^[0-9a-f]{40}$")


class GapVerificationError(RuntimeError):
    """Raised when the reviewed fleet or live remote state violates policy."""


def _token() -> str:
    value = os.environ.get("GH_TOKEN") or os.environ.get(
        "GITHUB_REPOSITORY_ADMIN_TOKEN"
    )
    if not value or any(character.isspace() for character in value):
        raise GapVerificationError("a non-whitespace GitHub credential is required")
    return value


def api_get(path: str) -> dict[str, Any] | None:
    request = urllib.request.Request(API + path, method="GET")
    request.add_header("Accept", "application/vnd.github+json")
    request.add_header("Authorization", f"Bearer {_token()}")
    request.add_header("X-GitHub-Api-Version", "2022-11-28")
    request.add_header("User-Agent", "reviewed-repository-gap-verifier")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            raw = response.read(MAX_RESPONSE_BYTES + 1)
            if len(raw) > MAX_RESPONSE_BYTES:
                raise GapVerificationError(f"GitHub response exceeded bound for {path}")
            payload = json.loads(raw) if raw else {}
    except urllib.error.HTTPError as error:
        error.read(4096)
        if error.code == 404:
            return None
        raise GapVerificationError(
            f"GitHub API GET {path} failed with HTTP {error.code}"
        ) from error
    except (json.JSONDecodeError, urllib.error.URLError) as error:
        raise GapVerificationError(f"invalid GitHub response for {path}") from error
    if not isinstance(payload, dict):
        raise GapVerificationError(f"GitHub response was not an object for {path}")
    return payload


def validate_manifest(payload: dict[str, Any]) -> list[dict[str, Any]]:
    if payload.get("schema_version") != 2:
        raise GapVerificationError("reviewed manifest schema changed")
    if payload.get("repository_count") != 32:
        raise GapVerificationError("reviewed manifest must contain 32 repositories")
    if payload.get("generator_sha256") != GENERATOR_SHA256:
        raise GapVerificationError("reviewed generator digest changed")
    if payload.get("organizations") != {"hypesiege": 15, "streempilot": 17}:
        raise GapVerificationError("reviewed organization counts changed")

    records = payload.get("repositories")
    if not isinstance(records, list) or len(records) != 32:
        raise GapVerificationError("reviewed repository ledger is malformed")

    names: list[str] = []
    normalized: list[dict[str, Any]] = []
    for record in records:
        if not isinstance(record, dict):
            raise GapVerificationError("reviewed repository record is not an object")
        full_name = record.get("full_name")
        commit = record.get("commit")
        if not isinstance(full_name, str) or full_name.count("/") != 1:
            raise GapVerificationError("reviewed repository identity is invalid")
        if not isinstance(commit, str) or SHA_RE.fullmatch(commit) is None:
            raise GapVerificationError(f"reviewed commit is invalid for {full_name}")
        if record.get("visibility") != "public":
            raise GapVerificationError(
                "sealed product-intent manifest is no longer uniformly public"
            )
        names.append(full_name)
        normalized.append(record)

    if len({name.casefold() for name in names}) != len(names):
        raise GapVerificationError("reviewed repository identities are duplicated")
    missing_from_manifest = sorted(set(EXPECTED_MISSING) - set(names))
    if missing_from_manifest:
        raise GapVerificationError(
            f"expected gap identities are absent from the reviewed manifest: {missing_from_manifest}"
        )
    return normalized


def assert_expected_missing(observed: list[str] | tuple[str, ...]) -> None:
    observed_set = {value.casefold() for value in observed}
    expected_set = {value.casefold() for value in EXPECTED_MISSING}
    if observed_set != expected_set:
        unexpected = sorted(observed_set - expected_set)
        absent = sorted(expected_set - observed_set)
        raise GapVerificationError(
            f"remote gap set changed: unexpected={unexpected} absent={absent}"
        )


def validate_repository_state(
    full_name: str,
    repository: dict[str, Any],
    reference: dict[str, Any],
) -> dict[str, Any]:
    repository_id = repository.get("id")
    if not isinstance(repository_id, int) or repository_id <= 0:
        raise GapVerificationError(f"{full_name} has no stable repository ID")
    if repository.get("private") is not True or repository.get("visibility") != "private":
        raise GapVerificationError(f"{full_name} is not private")
    if repository.get("default_branch") != "main":
        raise GapVerificationError(f"{full_name} default branch is not main")
    object_value = reference.get("object")
    head = object_value.get("sha") if isinstance(object_value, dict) else None
    if not isinstance(head, str) or SHA_RE.fullmatch(head) is None:
        raise GapVerificationError(f"{full_name} has no exact main SHA")
    return {
        "repository_id": repository_id,
        "visibility": "private",
        "default_branch": "main",
        "main_sha": head,
        "html_url": repository.get("html_url"),
    }


def fetch_manifest() -> list[dict[str, Any]]:
    query = urllib.parse.urlencode({"ref": SOURCE_SHA})
    response = api_get(
        f"/repos/{SOURCE_REPOSITORY}/contents/{MANIFEST_PATH}?{query}"
    )
    if response is None or response.get("type") != "file":
        raise GapVerificationError("reviewed fleet manifest could not be fetched")
    encoded = response.get("content")
    if response.get("encoding") != "base64" or not isinstance(encoded, str):
        raise GapVerificationError("reviewed fleet manifest encoding changed")
    try:
        compact = "".join(encoded.split())
        raw = base64.b64decode(compact, validate=True)
        payload = json.loads(raw)
    except (ValueError, json.JSONDecodeError) as error:
        raise GapVerificationError("reviewed fleet manifest is not valid JSON") from error
    if not isinstance(payload, dict):
        raise GapVerificationError("reviewed fleet manifest is not an object")
    return validate_manifest(payload)


def fetch_repository_state(full_name: str) -> dict[str, Any] | None:
    repository = api_get(f"/repos/{full_name}")
    if repository is None:
        return None
    reference = api_get(f"/repos/{full_name}/git/ref/heads/main")
    if reference is None:
        raise GapVerificationError(f"{full_name} has no main branch")
    return validate_repository_state(full_name, repository, reference)


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def run_preflight(output: Path) -> None:
    records = fetch_manifest()
    missing: list[str] = []
    existing: dict[str, dict[str, Any]] = {}
    sealed_commits: dict[str, str] = {}
    for record in records:
        full_name = str(record["full_name"])
        sealed_commits[full_name] = str(record["commit"])
        state = fetch_repository_state(full_name)
        if state is None:
            missing.append(full_name)
        else:
            existing[full_name] = state

    assert_expected_missing(missing)
    write_json(
        output,
        {
            "schema_version": 1,
            "source_repository": SOURCE_REPOSITORY,
            "source_sha": SOURCE_SHA,
            "expected_missing": list(EXPECTED_MISSING),
            "observed_missing": sorted(missing, key=str.casefold),
            "existing": existing,
            "sealed_commits": sealed_commits,
        },
    )
    print(f"VERIFIED_EXPECTED_GAPS missing={len(missing)} preserved={len(existing)}")


def run_postflight(preflight_path: Path, output: Path) -> None:
    preflight = json.loads(preflight_path.read_text(encoding="utf-8"))
    if not isinstance(preflight, dict):
        raise GapVerificationError("preflight evidence is not an object")
    expected = preflight.get("expected_missing")
    if not isinstance(expected, list):
        raise GapVerificationError("preflight expected-gap evidence is malformed")
    assert_expected_missing([str(value) for value in expected])

    existing = preflight.get("existing")
    if not isinstance(existing, dict):
        raise GapVerificationError("preflight existing-state evidence is malformed")

    records = fetch_manifest()
    current: dict[str, dict[str, Any]] = {}
    for record in records:
        full_name = str(record["full_name"])
        state = fetch_repository_state(full_name)
        if state is None:
            raise GapVerificationError(f"{full_name} is still missing after publication")
        current[full_name] = state

    for full_name, before in existing.items():
        if not isinstance(before, dict):
            raise GapVerificationError(f"preflight state is malformed for {full_name}")
        after = current.get(full_name)
        if after is None:
            raise GapVerificationError(f"preserved repository disappeared: {full_name}")
        if after.get("repository_id") != before.get("repository_id"):
            raise GapVerificationError(f"repository ID changed for {full_name}")
        if after.get("main_sha") != before.get("main_sha"):
            raise GapVerificationError(f"main changed during publication for {full_name}")

    resolved = {name: current[name] for name in EXPECTED_MISSING}
    write_json(
        output,
        {
            "schema_version": 1,
            "source_repository": SOURCE_REPOSITORY,
            "source_sha": SOURCE_SHA,
            "resolved_gaps": resolved,
            "preserved_count": len(existing),
            "total_verified": len(current),
        },
    )
    print(
        "VERIFIED_EXPECTED_GAPS_POSTFLIGHT "
        f"resolved={len(resolved)} preserved={len(existing)} total={len(current)}"
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    preflight = subparsers.add_parser("preflight")
    preflight.add_argument("--output", type=Path, required=True)
    postflight = subparsers.add_parser("postflight")
    postflight.add_argument("--preflight", type=Path, required=True)
    postflight.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.command == "preflight":
            run_preflight(args.output)
        else:
            run_postflight(args.preflight, args.output)
    except (GapVerificationError, OSError, json.JSONDecodeError) as error:
        print(f"repository-gap verification failed: {error}", file=os.sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
