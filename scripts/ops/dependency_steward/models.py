"""Core data models, policy helpers, markers, and portfolio registry loading."""

from __future__ import annotations

import argparse
import base64
import configparser
import csv
import dataclasses
import datetime as dt
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict

from dataclasses import dataclass, field
from functools import total_ordering
from pathlib import Path
from typing import Any, Callable, Iterable, Mapping, Sequence
from zoneinfo import ZoneInfo

CENTRAL_TIMEZONE = "America/Chicago"
CENTRAL_LOCAL_HOUR = 2
JOB_MARKER = "dependency-steward:v1"
PR_MARKER_RE = re.compile(
    r"<!--\s*dependency-steward:v1\s+(\{.*?\})\s*-->", re.DOTALL
)
SEMVER_RE = re.compile(
    r"(?<![0-9A-Za-z])v?(0|[1-9][0-9]*)\."
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?(?![0-9A-Za-z])"
)
GITHUB_REPO_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
REDACT_RE = re.compile(
    r"(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|"
    r"lin_api_[A-Za-z0-9]{20,}|Bearer\s+[A-Za-z0-9._~+/=-]{20,})"
)
SENSITIVE_ENV_EXACT = {
    "DEPENDENCY_STEWARD_GITHUB_TOKEN",
    "DEPENDENCY_STEWARD_READ_TOKEN",
    "PROJECT_SYNC_GITHUB_TOKEN",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "LINEAR_API_KEY",
    "DEPENDENCY_STEWARD_REMEDIATION_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "ACTIONS_RUNTIME_TOKEN",
    "ACTIONS_CACHE_URL",
    "ACTIONS_RESULTS_URL",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "KUBECONFIG",
    "DOCKER_CONFIG",
    "GITHUB_ENV",
    "GITHUB_OUTPUT",
    "GITHUB_PATH",
    "GITHUB_STATE",
    "GITHUB_STEP_SUMMARY",
}
SENSITIVE_ENV_MARKERS = (
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "PRIVATE_KEY",
    "CREDENTIAL",
    "API_KEY",
    "AUTHORIZATION",
    "ACCESS_KEY",
    "SESSION",
    "COOKIE",
    "JWT",
)
SKIP_DIRS = {
    ".git",
    ".direnv",
    ".venv",
    ".vendor",
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    "Pods",
}


class StewardError(RuntimeError):
    """Expected fail-closed controller error."""


@total_ordering
@dataclass(frozen=True)
class SemVer:
    major: int
    minor: int
    patch: int

    @classmethod
    def parse(cls, value: str | None) -> "SemVer | None":
        if not value:
            return None
        match = SEMVER_RE.search(value.strip())
        if not match or match.group(4):
            return None
        return cls(*(int(match.group(index)) for index in (1, 2, 3)))

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, SemVer):
            return NotImplemented
        return (self.major, self.minor, self.patch) < (
            other.major,
            other.minor,
            other.patch,
        )


@dataclass(frozen=True)
class RemoteVersion:
    version: SemVer
    tag: str
    sha: str


@dataclass
class DependencyRef:
    repository: str
    kind: str
    key: str
    name: str
    source_url: str
    manifest_path: str
    current_ref: str | None = None
    current_version: SemVer | None = None
    mutable: bool = True
    locator: dict[str, str] = field(default_factory=dict)
    note: str | None = None

    def graph_dict(self) -> dict[str, Any]:
        return {
            "repository": self.repository,
            "kind": self.kind,
            "key": self.key,
            "name": self.name,
            "source_url": self.source_url,
            "manifest_path": self.manifest_path,
            "current_ref": self.current_ref,
            "current_version": (
                str(self.current_version) if self.current_version else None
            ),
            "mutable": self.mutable,
            "note": self.note,
        }


@dataclass
class ProbeResult:
    version: str
    passed: bool
    command: str
    log_tail: str
    duration_seconds: float
    remediated: bool = False


@dataclass
class CommandResult:
    passed: bool
    command: str
    log_tail: str
    duration_seconds: float


@dataclass
class RepoPolicy:
    test_commands: list[str]
    prepare_commands: list[str]
    lock_commands: list[str]
    remediate_command: str | None
    timeout_seconds: int
    excluded_dependencies: set[str]


@dataclass
class RepoSummary:
    repository: str
    base_sha: str | None = None
    status: str = "pending"
    manifests: int = 0
    edges: int = 0
    prs: list[str] = field(default_factory=list)
    tickets: list[str] = field(default_factory=list)
    closed_prs: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)


@dataclass
class PortfolioLink:
    portfolio_key: str
    github_org: str
    linear_project_id: str
    linear_project_name: str


@dataclass(frozen=True)
class TicketIntent:
    project_id: str
    project_name: str
    repository: str
    category: str
    dependency_key: str
    target: str
    title: str
    description: str
    marker: str


@dataclass(frozen=True)
class PullIntent:
    repository: str
    default_branch: str
    base_sha: str
    dependency: dict[str, Any]
    current_version: str
    target_version: str
    target_tag: str
    target_sha: str
    branch: str
    title: str
    body: str
    patch_path: str
    patch_sha256: str


def redact(value: str) -> str:
    return REDACT_RE.sub("[REDACTED]", value)


def sensitive_environment_key(key: str) -> bool:
    upper = key.upper()
    return upper in SENSITIVE_ENV_EXACT or any(marker in upper for marker in SENSITIVE_ENV_MARKERS)


def sanitized_environment(extra: Mapping[str, str] | None = None) -> dict[str, str]:
    """Build a child-process environment without provider or runner credentials."""

    result = {
        key: value
        for key, value in os.environ.items()
        if not sensitive_environment_key(key)
    }
    if extra:
        # Extra values are constructed by this controller and intentionally override
        # inherited non-secret values such as CI and the exact candidate metadata.
        result.update({str(key): str(value) for key, value in extra.items()})
    return result


def display_command(args: Sequence[str]) -> str:
    safe: list[str] = []
    for arg in args:
        value = str(arg)
        if "EXTRAHEADER=AUTHORIZATION" in value.upper():
            key = value.split("=", 1)[0]
            safe.append(f"{key}=[REDACTED]")
        else:
            safe.append(redact(value))
    return shlex.join(safe)


def scheduled_cron_is_active(
    now: dt.datetime,
    cron_expression: str,
    *,
    local_hour: int = CENTRAL_LOCAL_HOUR,
    timezone: str = CENTRAL_TIMEZONE,
) -> bool:
    """Select the dual UTC cron lane representing local Central time."""

    fields = cron_expression.split()
    if len(fields) != 5 or fields[0] != "0":
        raise ValueError(f"unsupported cron expression: {cron_expression}")
    scheduled_utc_hour = int(fields[1])
    local = now.astimezone(ZoneInfo(timezone))
    offset = local.utcoffset()
    if offset is None:
        raise ValueError(f"timezone {timezone} has no UTC offset")
    expected = (local_hour - int(offset.total_seconds() // 3600)) % 24
    return scheduled_utc_hour == expected


def minor_line_candidates(
    current: SemVer, versions: Iterable[RemoteVersion]
) -> list[RemoteVersion]:
    """Return the newest patch in each newer minor line of the same major."""

    by_minor: dict[int, RemoteVersion] = {}
    for item in versions:
        if item.version.major != current.major or item.version.minor <= current.minor:
            continue
        existing = by_minor.get(item.version.minor)
        if existing is None or existing.version < item.version:
            by_minor[item.version.minor] = item
    return sorted(by_minor.values(), key=lambda item: item.version)


def newer_major_versions(
    current: SemVer, versions: Iterable[RemoteVersion]
) -> list[RemoteVersion]:
    by_major: dict[int, RemoteVersion] = {}
    for item in versions:
        if item.version.major <= current.major:
            continue
        existing = by_major.get(item.version.major)
        if existing is None or existing.version < item.version:
            by_major[item.version.major] = item
    return sorted(by_major.values(), key=lambda item: item.version)


def patch_only_versions(
    current: SemVer, versions: Iterable[RemoteVersion]
) -> list[RemoteVersion]:
    return sorted(
        (
            item
            for item in versions
            if item.version.major == current.major
            and item.version.minor == current.minor
            and item.version.patch > current.patch
        ),
        key=lambda item: item.version,
    )


def bisect_highest_passing(
    candidates: Sequence[RemoteVersion],
    probe: Callable[[RemoteVersion], bool],
) -> tuple[RemoteVersion | None, dict[str, bool], bool]:
    """Find the newest passing minor line with a guarded compatibility bisect.

    The primary search assumes the normal compatibility frontier: older minor
    lines pass and newer lines fail. The immediate boundary is then checked. If
    a later pass violates that monotonic assumption, a bounded descending scan
    identifies the true newest passing candidate.
    """

    attempts: dict[str, bool] = {}

    def checked(item: RemoteVersion) -> bool:
        key = str(item.version)
        if key not in attempts:
            attempts[key] = bool(probe(item))
        return attempts[key]

    low, high = 0, len(candidates) - 1
    best_index: int | None = None
    while low <= high:
        middle = (low + high) // 2
        if checked(candidates[middle]):
            best_index = middle
            low = middle + 1
        else:
            high = middle - 1

    # Compatibility across minor lines is normally monotonic, which makes the
    # bisect useful. Real repositories can violate that assumption, however.
    # Verify the unproven suffix from newest to oldest; the first later pass is
    # the true newest passing candidate and marks the frontier non-monotonic.
    non_monotonic = False
    suffix_floor = 0 if best_index is None else best_index + 1
    for index in range(len(candidates) - 1, suffix_floor - 1, -1):
        if checked(candidates[index]):
            if best_index is None or index > best_index:
                non_monotonic = best_index is not None or any(
                    result is False for result in attempts.values()
                )
                best_index = index
            break

    return (
        candidates[best_index] if best_index is not None else None,
        attempts,
        non_monotonic,
    )


def pr_marker(payload: Mapping[str, Any]) -> str:
    compact = json.dumps(payload, sort_keys=True, separators=(",", ":"))
    return f"<!-- {JOB_MARKER} {compact} -->"


def parse_pr_marker(body: str | None) -> dict[str, Any] | None:
    if not body:
        return None
    match = PR_MARKER_RE.search(body)
    if not match:
        return None
    try:
        value = json.loads(match.group(1))
    except json.JSONDecodeError:
        return None
    return value if isinstance(value, dict) else None


def managed_pr_numbers_to_close(
    pulls: Sequence[Mapping[str, Any]],
    *,
    dependency_key: str,
    target: SemVer,
    keep_number: int | None = None,
) -> list[int]:
    """Select only clearly superseded PRs created by this exact controller."""

    obsolete: list[int] = []
    for pull in pulls:
        number = int(pull.get("number", 0))
        if not number or number == keep_number:
            continue
        marker = parse_pr_marker(str(pull.get("body") or ""))
        if not marker or marker.get("key") != dependency_key:
            continue
        old_target = SemVer.parse(str(marker.get("target") or ""))
        if old_target is not None and old_target <= target:
            obsolete.append(number)
    return sorted(set(obsolete))


def canonical_github_repo(url_or_coordinate: str) -> str | None:
    value = url_or_coordinate.strip()
    if GITHUB_REPO_RE.fullmatch(value):
        return value.removesuffix(".git")
    ssh = re.match(r"git@github\.com:([^/]+/[^/]+?)(?:\.git)?$", value)
    if ssh:
        return ssh.group(1)
    parsed = urllib.parse.urlparse(value)
    if parsed.hostname and parsed.hostname.lower() == "github.com":
        path = parsed.path.strip("/").removesuffix(".git")
        if GITHUB_REPO_RE.fullmatch(path):
            return path
    return None


def github_url(repo: str) -> str:
    return f"https://github.com/{repo}.git"


def resolve_submodule_url(parent_repo: str, value: str) -> str:
    if not value.startswith("."):
        return value
    owner, _ = parent_repo.split("/", 1)
    base = f"https://github.com/{owner}/placeholder.git"
    return urllib.parse.urljoin(base, value)


def safe_slug(value: str, limit: int = 44) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-") or "dependency"
    digest = hashlib.sha256(value.encode()).hexdigest()[:8]
    return f"{slug[:limit]}-{digest}"


def branch_name(dep: DependencyRef, target: SemVer) -> str:
    return (
        "automation/dependency-minor/"
        f"{safe_slug(dep.key)}/{target.major}.{target.minor}"
    )


def graph_to_dot(edges: Sequence[DependencyRef]) -> str:
    def quote(value: str) -> str:
        return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'

    lines = ["digraph portfolio_dependencies {", "  rankdir=LR;"]
    for edge in sorted(edges, key=lambda item: (item.repository, item.key, item.kind)):
        target = canonical_github_repo(edge.source_url) or edge.name
        label = edge.kind
        if edge.current_version:
            label += f" {edge.current_version}"
        lines.append(
            f"  {quote(edge.repository)} -> {quote(target)} "
            f"[label={quote(label)}];"
        )
    lines.append("}")
    return "\n".join(lines) + "\n"


def load_portfolio_registry(path: Path) -> list[PortfolioLink]:
    rows: list[PortfolioLink] = []
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        required = {
            "portfolio_key",
            "github_org",
            "linear_project_id",
            "linear_project_name",
        }
        if not required.issubset(reader.fieldnames or ()):
            raise StewardError(f"registry lacks columns: {sorted(required)}")
        for row in reader:
            rows.append(
                PortfolioLink(
                    portfolio_key=row["portfolio_key"].strip(),
                    github_org=row["github_org"].strip(),
                    linear_project_id=row["linear_project_id"].strip(),
                    linear_project_name=row["linear_project_name"].strip(),
                )
            )
    if not rows:
        raise StewardError(f"registry is empty: {path}")
    lowered = [row.github_org.lower() for row in rows]
    if len(lowered) != len(set(lowered)):
        raise StewardError("registry contains duplicate GitHub organizations")
    return rows
