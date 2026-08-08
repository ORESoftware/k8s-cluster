#!/usr/bin/env python3
"""Publish repository relationships across the current certified org fleet."""

from __future__ import annotations

from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import bootstrap_current_org_dotgithub_repositories as current  # noqa: E402
import bootstrap_org_dotgithub_repositories as governance  # noqa: E402

EXPECTED_COUNT = current.EXPECTED_COUNT
ORGANIZATIONS = tuple(current.ORGANIZATIONS)

if EXPECTED_COUNT != 62:
    raise RuntimeError(
        f"current organization publisher expects {EXPECTED_COUNT}, not 62"
    )
if len(ORGANIZATIONS) != EXPECTED_COUNT:
    raise RuntimeError(
        f"current organization fleet contains {len(ORGANIZATIONS)} entries"
    )
if len({organization.lower() for organization in ORGANIZATIONS}) != EXPECTED_COUNT:
    raise RuntimeError("current organization fleet contains duplicates")

# The relationship engine deliberately reuses the older hardened GitHub API and
# privacy-preserving file primitives. Patch only its bounded fleet constant
# before importing the engine so both preflight and publication cover the
# current certified inventory.
governance.ORGANIZATIONS = ORGANIZATIONS

import publish_org_repository_relationships as publisher  # noqa: E402

publisher.ORGANIZATIONS = ORGANIZATIONS


def main() -> int:
    """Validate the current fleet contract, then delegate to the engine."""
    current.validate_static()
    return publisher.main()


if __name__ == "__main__":
    raise SystemExit(main())
