#!/usr/bin/env python3
"""Retry expired/failed GitHub organization invitations for one user.

The script enumerates every organization where the authenticated account is an
active owner, checks GitHub's failed-invitations feed, cancels only failed
invitations matching the target user, and creates a fresh direct-member
invitation. Active memberships and valid pending invitations are untouched.
"""
from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import sys
import time
from typing import Any
from urllib.parse import quote, urlencode

import invite_org_member_all as base

# The failed-invitations endpoint is documented in the current REST version.
base.API_VERSION = "2026-03-10"


@dataclass
class RetryResult:
    organization: str
    result: str
    membership_state: str | None = None
    failed_invitation_ids: list[int] | None = None
    failed_reasons: list[str] | None = None
    detail: str = ""


@dataclass
class RetryReport:
    generated_at: str
    mode: str
    authenticated_login: str
    target_username: str
    owner_organizations: int
    counts: dict[str, int]
    organizations: list[RetryResult]


def get_target_user_id(api: base.Api, username: str) -> int:
    response = api.request("GET", f"/users/{quote(username, safe='')}")
    if response.status != 200 or not isinstance(response.data, dict):
        raise RuntimeError("target user lookup did not return a JSON object")
    user_id = response.data.get("id")
    if not isinstance(user_id, int) or user_id <= 0:
        raise RuntimeError("target user lookup did not include a numeric id")
    return user_id


def invitation_matches(item: Any, username: str, user_id: int) -> bool:
    if not isinstance(item, dict):
        return False
    lowered = username.lower()
    for key in ("login", "username"):
        value = item.get(key)
        if isinstance(value, str) and value.lower() == lowered:
            return True
    for key in ("invitee", "user"):
        nested = item.get(key)
        if not isinstance(nested, dict):
            continue
        login = nested.get("login")
        if isinstance(login, str) and login.lower() == lowered:
            return True
        nested_id = nested.get("id")
        if isinstance(nested_id, int) and nested_id == user_id:
            return True
    for key in ("invitee_id", "user_id"):
        value = item.get(key)
        if isinstance(value, int) and value == user_id:
            return True
    return False


def list_failed_invitations(api: base.Api, organization: str) -> list[dict[str, Any]]:
    encoded_org = quote(organization, safe="")
    output: list[dict[str, Any]] = []
    for page in range(1, 101):
        query = urlencode({"per_page": 100, "page": page})
        response = api.request(
            "GET",
            f"/orgs/{encoded_org}/failed_invitations?{query}",
            allowed_statuses={404},
        )
        if response.status == 404:
            raise RuntimeError("failed-invitations endpoint returned 404")
        if response.status != 200 or not isinstance(response.data, list):
            raise RuntimeError(
                f"failed-invitations endpoint returned HTTP {response.status} without a JSON array"
            )
        page_items = [item for item in response.data if isinstance(item, dict)]
        output.extend(page_items)
        if len(response.data) < 100:
            break
    return output


def current_membership(api: base.Api, organization: str, username: str) -> tuple[int, str | None]:
    response = api.request(
        "GET",
        base.membership_path(organization, username),
        allowed_statuses={404},
    )
    if response.status == 404:
        return 404, None
    if response.status != 200 or not isinstance(response.data, dict):
        raise RuntimeError(f"membership lookup returned unexpected HTTP {response.status}")
    state = response.data.get("state")
    return 200, state if isinstance(state, str) else "unknown"


def retry_one(
    api: base.Api,
    organization: str,
    username: str,
    target_user_id: int,
    *,
    execute: bool,
) -> RetryResult:
    try:
        membership_status, state = current_membership(api, organization, username)
        failed = list_failed_invitations(api, organization)
    except (base.ApiFailure, RuntimeError) as exc:
        return RetryResult(organization, "failed", detail=base.redact(str(exc)))

    matching = [item for item in failed if invitation_matches(item, username, target_user_id)]
    failed_ids = [item.get("id") for item in matching if isinstance(item.get("id"), int)]
    reasons = [
        str(item.get("failed_reason") or item.get("reason") or "unknown")
        for item in matching
    ]

    if membership_status == 200 and state == "active":
        return RetryResult(organization, "already_member", state)

    if matching:
        if len(failed_ids) != len(matching):
            return RetryResult(
                organization,
                "failed",
                state,
                failed_ids,
                reasons,
                "a matching failed invitation did not include a numeric invitation id",
            )
        if not execute:
            return RetryResult(
                organization,
                "would_retry_failed",
                state,
                failed_ids,
                reasons,
            )

        encoded_org = quote(organization, safe="")
        try:
            for invitation_id in failed_ids:
                deleted = api.request(
                    "DELETE",
                    f"/orgs/{encoded_org}/invitations/{invitation_id}",
                    allowed_statuses={404},
                )
                if deleted.status not in {204, 404}:
                    raise RuntimeError(
                        f"failed invitation deletion returned HTTP {deleted.status}"
                    )

            # Avoid GitHub's secondary content-creation rate limit when many
            # organizations need a retry in one pass.
            time.sleep(1.25)
            created = api.request(
                "POST",
                f"/orgs/{encoded_org}/invitations",
                payload={"invitee_id": target_user_id, "role": "direct_member"},
            )
            if created.status != 201:
                raise RuntimeError(f"fresh invitation returned HTTP {created.status}")
            verify_status, verify_state = current_membership(api, organization, username)
            if verify_status != 200 or verify_state != "pending":
                raise RuntimeError(
                    f"fresh invitation verification returned status={verify_status}, state={verify_state!r}"
                )
            return RetryResult(
                organization,
                "retried_failed",
                verify_state,
                failed_ids,
                reasons,
            )
        except (base.ApiFailure, RuntimeError) as exc:
            return RetryResult(
                organization,
                "failed",
                state,
                failed_ids,
                reasons,
                base.redact(str(exc)),
            )

    if membership_status == 200 and state == "pending":
        return RetryResult(organization, "already_pending", state)

    if membership_status == 404:
        if not execute:
            return RetryResult(organization, "would_invite_missing")
        encoded_org = quote(organization, safe="")
        try:
            time.sleep(1.25)
            created = api.request(
                "POST",
                f"/orgs/{encoded_org}/invitations",
                payload={"invitee_id": target_user_id, "role": "direct_member"},
            )
            if created.status != 201:
                raise RuntimeError(f"fresh invitation returned HTTP {created.status}")
            verify_status, verify_state = current_membership(api, organization, username)
            if verify_status != 200 or verify_state != "pending":
                raise RuntimeError(
                    f"fresh invitation verification returned status={verify_status}, state={verify_state!r}"
                )
            return RetryResult(organization, "invited_missing", verify_state)
        except (base.ApiFailure, RuntimeError) as exc:
            return RetryResult(organization, "failed", detail=base.redact(str(exc)))

    return RetryResult(
        organization,
        "failed",
        state,
        detail=f"unexpected membership state: {state!r}",
    )


def build_report(
    api: base.Api,
    username: str,
    *,
    execute: bool,
    expected_authenticated_login: str,
) -> RetryReport:
    authenticated_login = base.get_authenticated_login(api)
    if authenticated_login.lower() != expected_authenticated_login.lower():
        raise RuntimeError(
            f"authenticated GitHub account is {authenticated_login!r}, expected {expected_authenticated_login!r}"
        )
    target_user_id = get_target_user_id(api, username)
    organizations = base.discover_owner_organizations(api)
    if not organizations:
        raise RuntimeError("no active owner/admin organization memberships were returned")

    results = [
        retry_one(api, organization, username, target_user_id, execute=execute)
        for organization in organizations
    ]
    counts: dict[str, int] = {}
    for item in results:
        counts[item.result] = counts.get(item.result, 0) + 1
    return RetryReport(
        generated_at=datetime.now(timezone.utc).isoformat(),
        mode="execute" if execute else "dry-run",
        authenticated_login=authenticated_login,
        target_username=username,
        owner_organizations=len(organizations),
        counts=dict(sorted(counts.items())),
        organizations=results,
    )


def render_markdown(report: RetryReport) -> str:
    counts = ", ".join(f"{key}={value}" for key, value in sorted(report.counts.items())) or "none"
    lines = [
        "# GitHub failed organization invitation retry report",
        "",
        f"- Authenticated account: `{report.authenticated_login}`",
        f"- Target user: `{report.target_username}`",
        f"- Mode: `{report.mode}`",
        f"- Owner/admin organizations discovered: **{report.owner_organizations}**",
        f"- Results: {counts}",
        "",
        "| Organization | Result | State | Failed reason(s) | Detail |",
        "|---|---|---|---|---|",
    ]
    for item in report.organizations:
        reasons = ", ".join(item.failed_reasons or [])
        detail = item.detail.replace("|", "\\|").replace("\n", " ")
        lines.append(
            f"| `{item.organization}` | `{item.result}` | `{item.membership_state or ''}` | "
            f"{reasons.replace('|', '\\|')} | {detail} |"
        )
    lines.extend(["", "<!-- org-member-invitation-report-complete -->", ""])
    return "\n".join(lines)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--username", required=True, type=base.validate_username)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--expected-authenticated-login", default="ORESoftware", type=base.validate_username)
    parser.add_argument("--json-report", type=Path)
    parser.add_argument("--markdown-report", type=Path)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    token = os.environ.get("GH_TOKEN", "")
    try:
        api = base.GitHubApi(token, timeout_seconds=45, retries=4)
        report = build_report(
            api,
            args.username,
            execute=args.execute,
            expected_authenticated_login=args.expected_authenticated_login,
        )
        payload = asdict(report)
        markdown = render_markdown(report)
        if args.json_report:
            args.json_report.parent.mkdir(parents=True, exist_ok=True)
            args.json_report.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        if args.markdown_report:
            args.markdown_report.parent.mkdir(parents=True, exist_ok=True)
            args.markdown_report.write_text(markdown, encoding="utf-8")
        print(markdown, end="")
        return 1 if report.counts.get("failed", 0) else 0
    except (base.ApiFailure, RuntimeError, ValueError) as exc:
        print(f"failed-invitation retry failed: {base.redact(str(exc))}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
