#!/usr/bin/env python3
"""Idempotently ensure a GitHub user has a pending or active membership in every
organization where the authenticated account is an active owner.

The script is dry-run by default. Pass --execute to create missing memberships.
It never removes members, cancels invitations, changes existing roles, or assigns
teams.
"""
from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import sys
import time
from typing import Any, Iterable, Mapping, Protocol
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode
from urllib.request import Request, urlopen

API_ROOT = "https://api.github.com"
API_VERSION = "2022-11-28"
USER_AGENT = "oresoftware-org-member-inviter/1"
USERNAME_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$")
TOKEN_PATTERNS = (
    re.compile(r"github_pat_[A-Za-z0-9_]{20,}", re.I),
    re.compile(r"gh[pousr]_[A-Za-z0-9_]{20,}", re.I),
    re.compile(r"(?i)(authorization:\s*bearer\s+)[A-Za-z0-9._-]{20,}"),
)


def redact(text: str) -> str:
    value = text
    value = TOKEN_PATTERNS[0].sub("github_pat_***", value)
    value = TOKEN_PATTERNS[1].sub("gh*_***", value)
    value = TOKEN_PATTERNS[2].sub(r"\1***", value)
    return value[:2000]


def validate_username(value: str) -> str:
    username = value.strip()
    if not USERNAME_RE.fullmatch(username) or "--" in username:
        raise argparse.ArgumentTypeError(f"invalid GitHub username: {value!r}")
    return username


@dataclass(frozen=True)
class ApiResponse:
    status: int
    data: Any
    headers: Mapping[str, str]


class ApiFailure(RuntimeError):
    def __init__(self, method: str, path: str, status: int, message: str, request_id: str = ""):
        self.method = method
        self.path = path
        self.status = status
        self.message = redact(message)
        self.request_id = request_id
        suffix = f" request_id={request_id}" if request_id else ""
        super().__init__(f"{method} {path} failed with HTTP {status}: {self.message}{suffix}")


class Api(Protocol):
    def request(
        self,
        method: str,
        path: str,
        *,
        payload: Mapping[str, Any] | None = None,
        allowed_statuses: Iterable[int] = (),
    ) -> ApiResponse: ...


class GitHubApi:
    def __init__(self, token: str, *, timeout_seconds: int = 30, retries: int = 3):
        if not token or any(ch.isspace() for ch in token):
            raise ValueError("GH_TOKEN must be a non-empty token without whitespace")
        self._token = token
        self._timeout_seconds = timeout_seconds
        self._retries = max(1, retries)

    @staticmethod
    def _decode_body(raw: bytes) -> Any:
        if not raw:
            return None
        text = raw.decode("utf-8", "replace")
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return text

    @staticmethod
    def _error_message(data: Any) -> str:
        if isinstance(data, dict):
            message = str(data.get("message") or "GitHub API request failed")
            errors = data.get("errors")
            if errors:
                message += f"; errors={errors!r}"
            documentation = data.get("documentation_url")
            if documentation:
                message += f"; documentation={documentation}"
            return message
        if data is None:
            return "GitHub API request failed without a response body"
        return str(data)

    def request(
        self,
        method: str,
        path: str,
        *,
        payload: Mapping[str, Any] | None = None,
        allowed_statuses: Iterable[int] = (),
    ) -> ApiResponse:
        if not path.startswith("/"):
            raise ValueError("GitHub API path must begin with '/'")
        allowed = set(allowed_statuses)
        body = None
        headers = {
            "Authorization": f"Bearer {self._token}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": USER_AGENT,
        }
        if payload is not None:
            body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
            headers["Content-Type"] = "application/json"

        for attempt in range(1, self._retries + 1):
            request = Request(API_ROOT + path, data=body, headers=headers, method=method)
            try:
                with urlopen(request, timeout=self._timeout_seconds) as response:
                    raw = response.read()
                    return ApiResponse(
                        status=int(response.status),
                        data=self._decode_body(raw),
                        headers=dict(response.headers.items()),
                    )
            except HTTPError as exc:
                raw = exc.read()
                data = self._decode_body(raw)
                response_headers = dict(exc.headers.items()) if exc.headers else {}
                if exc.code in allowed:
                    return ApiResponse(exc.code, data, response_headers)
                retryable = exc.code in {429, 500, 502, 503, 504}
                if retryable and attempt < self._retries:
                    retry_after = response_headers.get("Retry-After", "")
                    try:
                        delay = min(10.0, max(1.0, float(retry_after)))
                    except ValueError:
                        delay = float(attempt)
                    time.sleep(delay)
                    continue
                raise ApiFailure(
                    method,
                    path,
                    exc.code,
                    self._error_message(data),
                    response_headers.get("X-GitHub-Request-Id", ""),
                ) from None
            except URLError as exc:
                if attempt < self._retries:
                    time.sleep(float(attempt))
                    continue
                raise ApiFailure(method, path, 0, f"network error: {exc.reason}") from None
        raise AssertionError("unreachable")


@dataclass
class OrganizationResult:
    organization: str
    result: str
    membership_state: str | None = None
    membership_role: str | None = None
    detail: str = ""


@dataclass
class Report:
    generated_at: str
    mode: str
    authenticated_login: str
    target_username: str
    owner_organizations: int
    counts: dict[str, int]
    organizations: list[OrganizationResult]


def get_authenticated_login(api: Api) -> str:
    response = api.request("GET", "/user")
    if response.status != 200 or not isinstance(response.data, dict):
        raise RuntimeError("GitHub /user response was not a JSON object")
    login = response.data.get("login")
    if not isinstance(login, str) or not login:
        raise RuntimeError("GitHub /user response did not include a login")
    return login


def discover_owner_organizations(api: Api) -> list[str]:
    organizations: dict[str, str] = {}
    for page in range(1, 101):
        query = urlencode({"state": "active", "per_page": 100, "page": page})
        response = api.request("GET", f"/user/memberships/orgs?{query}")
        if response.status != 200 or not isinstance(response.data, list):
            raise RuntimeError("organization memberships response was not a JSON array")
        for membership in response.data:
            if not isinstance(membership, dict):
                continue
            if membership.get("state") != "active" or membership.get("role") != "admin":
                continue
            organization = membership.get("organization")
            login = organization.get("login") if isinstance(organization, dict) else None
            if isinstance(login, str) and login:
                organizations.setdefault(login.lower(), login)
        if len(response.data) < 100:
            break
    return sorted(organizations.values(), key=str.lower)


def membership_path(organization: str, username: str) -> str:
    return f"/orgs/{quote(organization, safe='')}/memberships/{quote(username, safe='')}"


def reconcile_one(api: Api, organization: str, username: str, *, execute: bool) -> OrganizationResult:
    path = membership_path(organization, username)
    try:
        current = api.request("GET", path, allowed_statuses={404})
    except ApiFailure as exc:
        return OrganizationResult(organization, "failed", detail=str(exc))

    if current.status == 200:
        data = current.data if isinstance(current.data, dict) else {}
        state = data.get("state") if isinstance(data.get("state"), str) else "unknown"
        role = data.get("role") if isinstance(data.get("role"), str) else None
        if state == "active":
            return OrganizationResult(organization, "already_member", state, role)
        if state == "pending":
            return OrganizationResult(
                organization,
                "already_invited",
                state,
                role,
                "An invitation is already pending; it was preserved rather than cancelled and recreated.",
            )
        return OrganizationResult(
            organization,
            "failed",
            state,
            role,
            f"unexpected existing membership state: {state}",
        )

    if current.status != 404:
        return OrganizationResult(organization, "failed", detail=f"unexpected membership lookup HTTP {current.status}")

    if not execute:
        return OrganizationResult(organization, "would_invite", detail="No active or pending membership exists.")

    try:
        created = api.request("PUT", path, payload={"role": "member"})
    except ApiFailure as exc:
        return OrganizationResult(organization, "failed", detail=str(exc))

    data = created.data if isinstance(created.data, dict) else {}
    state = data.get("state") if isinstance(data.get("state"), str) else None
    role = data.get("role") if isinstance(data.get("role"), str) else None
    if created.status == 200 and state in {"pending", "active"}:
        result = "invited" if state == "pending" else "added"
        return OrganizationResult(organization, result, state, role)
    return OrganizationResult(
        organization,
        "failed",
        state,
        role,
        f"membership write returned HTTP {created.status} with state={state!r}",
    )


def build_report(
    api: Api,
    username: str,
    *,
    execute: bool,
    expected_authenticated_login: str,
) -> Report:
    authenticated_login = get_authenticated_login(api)
    if authenticated_login.lower() != expected_authenticated_login.lower():
        raise RuntimeError(
            f"authenticated GitHub account is {authenticated_login!r}, expected {expected_authenticated_login!r}"
        )

    organizations = discover_owner_organizations(api)
    if not organizations:
        raise RuntimeError("no active owner/admin organization memberships were returned")

    results = [reconcile_one(api, organization, username, execute=execute) for organization in organizations]
    counts: dict[str, int] = {}
    for item in results:
        counts[item.result] = counts.get(item.result, 0) + 1
    return Report(
        generated_at=datetime.now(timezone.utc).isoformat(),
        mode="execute" if execute else "dry-run",
        authenticated_login=authenticated_login,
        target_username=username,
        owner_organizations=len(organizations),
        counts=dict(sorted(counts.items())),
        organizations=results,
    )


def render_markdown(report: Report) -> str:
    counts = ", ".join(f"{key}={value}" for key, value in sorted(report.counts.items())) or "none"
    lines = [
        "# GitHub organization membership invitation report",
        "",
        f"- Authenticated account: `{report.authenticated_login}`",
        f"- Target user: `{report.target_username}`",
        f"- Mode: `{report.mode}`",
        f"- Owner/admin organizations discovered: **{report.owner_organizations}**",
        f"- Results: {counts}",
        "",
        "| Organization | Result | State | Role | Detail |",
        "|---|---|---|---|---|",
    ]
    for item in report.organizations:
        detail = item.detail.replace("|", "\\|").replace("\n", " ")
        lines.append(
            f"| `{item.organization}` | `{item.result}` | "
            f"`{item.membership_state or ''}` | `{item.membership_role or ''}` | {detail} |"
        )
    lines.extend(["", "<!-- org-member-invitation-report-complete -->", ""])
    return "\n".join(lines)


def write_report(report: Report, json_path: Path | None, markdown_path: Path | None) -> str:
    payload = asdict(report)
    markdown = render_markdown(report)
    if json_path is not None:
        json_path.parent.mkdir(parents=True, exist_ok=True)
        json_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if markdown_path is not None:
        markdown_path.parent.mkdir(parents=True, exist_ok=True)
        markdown_path.write_text(markdown, encoding="utf-8")
    return markdown


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--username", required=True, type=validate_username)
    parser.add_argument("--execute", action="store_true", help="create missing organization memberships")
    parser.add_argument("--expected-authenticated-login", default="ORESoftware", type=validate_username)
    parser.add_argument("--json-report", type=Path)
    parser.add_argument("--markdown-report", type=Path)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    token = os.environ.get("GH_TOKEN", "")
    try:
        api = GitHubApi(token)
        report = build_report(
            api,
            args.username,
            execute=args.execute,
            expected_authenticated_login=args.expected_authenticated_login,
        )
        markdown = write_report(report, args.json_report, args.markdown_report)
        print(markdown, end="")
        return 1 if report.counts.get("failed", 0) else 0
    except (ApiFailure, RuntimeError, ValueError) as exc:
        print(f"org-member-inviter failed: {redact(str(exc))}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
