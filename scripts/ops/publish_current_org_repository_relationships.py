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

governance.ORGANIZATIONS = ORGANIZATIONS

import publish_org_repository_relationships as publisher  # noqa: E402

publisher.ORGANIZATIONS = ORGANIZATIONS
_ORIGINAL_BUILD_PLAN = publisher.build_plan
_ORIGINAL_RETRY_DELAY = governance.GitHubApi._retry_delay
MAX_PRIMARY_RATE_LIMIT_WAIT_SECONDS = 3600.0
PRIMARY_RATE_LIMIT_SAFETY_SECONDS = 2.0
PRIVATE_REFERENCE_REDACTION = (
    "Private repository details are intentionally withheld from this public "
    "document."
)
_REFERENCE_BOUNDARY = r"(?:$|[\s`'\"\)\]\}>/?#]|[.,;:!?](?=$|\s))"
PrivatePatterns = tuple[re.Pattern[str], ...]


def retry_delay_with_primary_rate_limit(
    headers: dict[str, str],
    attempt: int,
) -> float:
    """Wait through GitHub's primary quota reset before retrying safely."""
    normalized_headers = {
        key.lower(): value.strip()
        for key, value in headers.items()
    }
    remaining = normalized_headers.get("x-ratelimit-remaining")
    reset = normalized_headers.get("x-ratelimit-reset")
    if remaining == "0" and reset and reset.isdigit():
        delay = (
            float(reset)
            - governance.time.time()
            + PRIMARY_RATE_LIMIT_SAFETY_SECONDS
        )
        return max(
            1.0,
            min(delay, MAX_PRIMARY_RATE_LIMIT_WAIT_SECONDS),
        )
    return _ORIGINAL_RETRY_DELAY(headers, attempt)


governance.GitHubApi._retry_delay = staticmethod(
    retry_delay_with_primary_rate_limit
)


class PrivateReferences(set[str]):
    """Compatibility tokens plus boundary-aware compiled matchers."""

    def __init__(
        self,
        tokens: set[str],
        patterns: PrivatePatterns,
    ) -> None:
        super().__init__(tokens)
        self.patterns = patterns


def is_private(repository: dict[str, Any]) -> bool:
    return bool(
        repository.get("private")
        or repository.get("visibility") == "private"
    )


def private_reference_tokens(
    organization: str,
    private_names: set[str],
) -> set[str]:
    """Return diagnostic-safe exact tokens retained for compatibility tests."""
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


def private_reference_patterns(
    organization: str,
    private_names: set[str],
) -> PrivatePatterns:
    """Match exact private identities without matching longer public names."""
    patterns: list[re.Pattern[str]] = []
    for name in sorted(private_names):
        if not name:
            continue
        full_name = f"{organization}/{name}"
        patterns.extend(
            (
                re.compile(
                    rf"(?<![A-Za-z0-9_.-]){re.escape(full_name)}"
                    rf"{_REFERENCE_BOUNDARY}"
                ),
                re.compile(
                    rf"(?:https://github\.com/|git@github\.com:)"
                    rf"{re.escape(full_name)}(?:\.git)?{_REFERENCE_BOUNDARY}"
                ),
                re.compile(
                    rf'"name"\s*:\s*{re.escape(json.dumps(name))}'
                    r"(?=\s*[,}])"
                ),
                re.compile(
                    rf'"(?:full_name|from|to)"\s*:\s*'
                    rf"{re.escape(json.dumps(full_name))}(?=\s*[,}}])"
                ),
            )
        )
        if name.lower() != organization.lower():
            patterns.append(re.compile(rf"`{re.escape(name)}`"))
    return tuple(patterns)


def private_references(
    organization: str,
    private_names: set[str],
) -> PrivateReferences:
    return PrivateReferences(
        private_reference_tokens(organization, private_names),
        private_reference_patterns(organization, private_names),
    )


def contains_private_reference(
    content: str,
    references: PrivateReferences,
) -> bool:
    return any(pattern.search(content) for pattern in references.patterns)


def redact_existing_private_reference_lines(
    content: str | None,
    references: PrivateReferences,
) -> str | None:
    """Replace only existing public lines that expose an exact private identity."""
    if not content or not references:
        return content

    redacted: list[str] = []
    for line in content.splitlines(keepends=True):
        if not contains_private_reference(line, references):
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
) -> tuple[str, dict[str, tuple[str, Any]], PrivateReferences, dict[str, Any]]:
    """Build a plan without treating longer public names as private leaks."""
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
    references = private_references(organization, private_names)

    for path in publisher.README_PATHS:
        _, existing = files[path]
        existing_content = existing.content if existing else None
        privacy_safe_existing = redact_existing_private_reference_lines(
            existing_content,
            references,
        )
        files[path] = (
            publisher.merge_managed_block(
                privacy_safe_existing,
                publisher.relationship_readme_block(organization),
            ),
            existing,
        )

    if any(
        contains_private_reference(content, references)
        for content, _ in files.values()
    ):
        raise RuntimeError(
            f"privacy preflight failed for {organization}/.github"
        )
    return branch, files, references, result


def run_plan_with_exact_private_references(
    api: Any,
    plan: tuple[
        str,
        str,
        dict[str, tuple[str, Any]],
        PrivateReferences,
        dict[str, Any],
    ],
    execute: bool,
) -> None:
    """Write and verify files with boundary-aware private-reference checks."""
    organization, branch, files, references, result = plan
    for path, (desired, existing) in files.items():
        changed = not existing or existing.content != desired
        target = result["changed_files"] if changed else result["unchanged_files"]
        target.append(path)
        if execute and changed:
            publisher.write_file(
                api,
                organization,
                path,
                branch,
                desired,
                existing,
            )
            print(f"UPDATED {organization}/.github:{path}")

    if not execute:
        return
    for path, (desired, _) in files.items():
        observed = publisher.base.fetch_file(api, organization, path, branch)
        leaked = observed and contains_private_reference(
            observed.content,
            references,
        )
        if not observed or observed.content != desired or leaked:
            raise RuntimeError(
                f"relationship verification failed for {organization}/.github"
            )
    result["verified"] = True
    print(f"VERIFIED {organization}/.github relationships")


publisher.build_plan = build_plan_with_exact_private_references
publisher.run_plan = run_plan_with_exact_private_references


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
