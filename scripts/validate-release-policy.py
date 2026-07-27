#!/usr/bin/env python3
"""Validate ClipTown's non-production release channel policy."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
POLICY_PATH = ROOT / "release" / "channels.json"
ALLOWED_STATUSES = {"blocked", "dry-run", "verified"}
EXPECTED_CHANNELS = {
    "cli-github-releases",
    "cli-homebrew",
    "desktop-macos",
    "desktop-windows",
    "desktop-linux",
    "mobile-ios",
    "mobile-android",
    "browser-chrome",
}
FORBIDDEN_URL_TOKENS = ("/latest/", "/main/", "/master/", "head", "replace_with", "todo")


def fail(message: str) -> None:
    print(f"release-policy error: {message}", file=sys.stderr)
    raise SystemExit(1)


def validate_https_url(value: str, context: str) -> None:
    parsed = urlparse(value)
    if parsed.scheme != "https" or not parsed.netloc:
        fail(f"{context} must be an absolute HTTPS URL")
    lowered = value.lower()
    if any(token in lowered for token in FORBIDDEN_URL_TOKENS):
        fail(f"{context} must not use floating or placeholder URL tokens")


def main() -> None:
    try:
        policy = json.loads(POLICY_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load {POLICY_PATH.relative_to(ROOT)}: {error}")

    if policy.get("schema_version") != 1:
        fail("schema_version must be 1")
    if policy.get("versioning") != "semver":
        fail("versioning must be semver")

    channels = policy.get("channels")
    if not isinstance(channels, list):
        fail("channels must be an array")

    identifiers: list[str] = []
    for index, channel in enumerate(channels):
        if not isinstance(channel, dict):
            fail(f"channels[{index}] must be an object")
        identifier = channel.get("id")
        status = channel.get("status")
        owner = channel.get("owner_repository")
        public_url = channel.get("public_url")
        evidence = channel.get("required_evidence")

        if not isinstance(identifier, str) or not identifier:
            fail(f"channels[{index}].id must be a non-empty string")
        identifiers.append(identifier)
        if status not in ALLOWED_STATUSES:
            fail(f"{identifier}: unknown status {status!r}")
        if not isinstance(owner, str) or not owner.startswith("cliptown/"):
            fail(f"{identifier}: owner_repository must be in the cliptown organization")
        if not isinstance(evidence, list) or len(evidence) < 3:
            fail(f"{identifier}: required_evidence must contain at least three gates")
        if any(not isinstance(item, str) or not item for item in evidence):
            fail(f"{identifier}: evidence entries must be non-empty strings")
        if len(set(evidence)) != len(evidence):
            fail(f"{identifier}: evidence entries must be unique")

        if status == "verified":
            if not isinstance(public_url, str):
                fail(f"{identifier}: verified channels require public_url")
            validate_https_url(public_url, f"{identifier}.public_url")
        elif public_url is not None:
            fail(f"{identifier}: blocked and dry-run channels must not publish public_url")

    if len(set(identifiers)) != len(identifiers):
        fail("channel ids must be unique")
    if set(identifiers) != EXPECTED_CHANNELS:
        missing = sorted(EXPECTED_CHANNELS - set(identifiers))
        extra = sorted(set(identifiers) - EXPECTED_CHANNELS)
        fail(f"channel inventory mismatch; missing={missing}, extra={extra}")

    support = policy.get("support")
    if not isinstance(support, dict):
        fail("support must be an object")
    support_status = support.get("status")
    support_url = support.get("public_url")
    if support.get("environment_variable") != "PUBLIC_PATREON_URL":
        fail("support.environment_variable must be PUBLIC_PATREON_URL")
    if support_status == "verified":
        if not isinstance(support_url, str):
            fail("verified support destination requires public_url")
        validate_https_url(support_url, "support.public_url")
    elif support_status == "unverified":
        if support_url is not None:
            fail("unverified support destination must not expose public_url")
    else:
        fail("support.status must be verified or unverified")

    print(f"release policy valid: {len(channels)} channels, support={support_status}")


if __name__ == "__main__":
    main()
