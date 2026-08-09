#!/usr/bin/env python3
"""Publish repository relationships across the current certified org fleet."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import sys
from typing import Any

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
_ORIGINAL_BUILD_PLAN = publisher.build_plan
PRIVATE_REFERENCE_REDACTION = (
    "Private repository details are intentionally withheld from this public "
    "document."
)


def is_private(repository: dict[str, Any]) -> bool:
    return bool(
        repository.get("private")
        or repository.get("visibility") == "private"
    )


def private_reference_tokens(
    organization: str,
    private_names: set[str],
) -> set[str]:
    """Return exact generated-registry tokens that would reveal private identities."""
    tokens: set[str] = set()
    for name in private_names:
        if not name:
            continue
        full_name = f"{organization}/{name}"
        quoted_name = json.dumps(name)
        quoted_full_name = json.dumps(full_name)
        tokens.update(
            {
                full_name,
                f"https://github.com/{full_name}",
                f'"name": {quoted_name}',
                f'"full_name": {quoted_full_name}',
                f'"from": {quoted_full_name}',
                f'"to": {quoted_full_name}',
                f'[`{name}`](https://github.com/{full_name})',
                f'`{full_name}`',
            }
        )
    return tokens


def redact_existing_private_reference_lines(
    content: str | None,
    tokens: set[str],
) -> str | None:
    """Replace only existing public lines that expose an exact private identity."""
    if not content or not tokens:
        return content

    redacted: list[str] = []
    for line in content.splitlines(keepends=True):
        if not any(token in line for token in tokens):
            redacted.append(line)
            continue

        if line.endswith("\r\n"):
            ending = "\r\n"
        elif line.endswith("\n"):
            ending = "\n"
        else:
            ending = ""
        stripped = line.lstrip()
        indentation = line[: len(line) - len(stripped)]
        list_prefix = "- " if stripped.startswith(("- ", "* ", "+ ")) else ""
        replacement = (
            f"{indentation}{list_prefix}{PRIVATE_REFERENCE_REDACTION}{ending}"
        )
        if redacted and redacted[-1].strip() == replacement.strip():
            continue
        redacted.append(replacement)
    return "".join(redacted)


def build_plan_with_exact_private_references(
    api: Any,
    organization: str,
    dotgithub: dict[str, Any],
    inventory: list[dict[str, Any]],
) -> tuple[str, dict[str, tuple[str, Any]], set[str], dict[str, Any]]:
    """Build a plan without treating incidental substrings as private-name leaks."""
    private_names = {
        str(repository.get("name", ""))
        for repository in inventory
        if is_private(repository)
    }
    masked_inventory: list[dict[str, Any]] = []
    for index, repository in enumerate(inventory):
        masked = dict(repository)
        if is_private(repository):
            masked_name = f"__withheld_private_repository_{index}__"
            masked["name"] = masked_name
            masked["full_name"] = f"{organization}/{masked_name}"
        masked_inventory.append(masked)

    branch, files, _, result = _ORIGINAL_BUILD_PLAN(
        api,
        organization,
        dotgithub,
        masked_inventory,
    )
    tokens = private_reference_tokens(organization, private_names)

    # Existing README/profile prose can predate this privacy contract. Redact
    # only lines containing exact private repository identities, then rebuild
    # the managed relationship block from trusted generated content. A leak in
    # that generated block still fails the preflight below.
    for path in publisher.README_PATHS:
        _, existing = files[path]
        existing_content = existing.content if existing else None
        privacy_safe_existing = redact_existing_private_reference_lines(
            existing_content,
            tokens,
        )
        files[path] = (
            publisher.merge_managed_block(
                privacy_safe_existing,
                publisher.relationship_readme_block(organization),
            ),
            existing,
        )

    if any(
        token in content
        for content, _ in files.values()
        for token in tokens
    ):
        raise RuntimeError(
            f"privacy preflight failed for {organization}/.github"
        )
    return branch, files, tokens, result


publisher.build_plan = build_plan_with_exact_private_references


def configure_expected_owner_login(value: str | None = None) -> str:
    """Bind preflight to the authenticated publisher without naming it publicly."""
    login = value if value is not None else os.environ.get("EXPECTED_OWNER_LOGIN", "")
    if not re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?", login):
        raise RuntimeError("EXPECTED_OWNER_LOGIN must be a valid GitHub login")
    governance.EXPECTED_ACTOR = login
    return login


def main() -> int:
    """Validate the current fleet contract, then delegate to the engine."""
    configure_expected_owner_login()
    current.validate_static()
    return publisher.main()


if __name__ == "__main__":
    raise SystemExit(main())
