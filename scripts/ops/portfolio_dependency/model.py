"""Policy, immutable models, and pure helpers for the dependency controller."""

from __future__ import annotations

import csv
import fnmatch
import hashlib
import json
import re
import subprocess
import threading
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterable, Mapping, Sequence

GITHUB_API_URL = "https://api.github.com"
LINEAR_GRAPHQL_URL = "https://api.linear.app/graphql"
USER_AGENT = "oresoftware-portfolio-dependency-bot/1"
DEFAULT_POLICY = Path("ops/registries/portfolio-dependency-policy.json")
DEFAULT_REGISTRY = Path("ops/registries/portfolio-project-links.csv")
MANAGED_COMMENT_PREFIX = "<!-- portfolio-dependency-bot:v1"
STABLE_SEMVER_RE = re.compile(r"^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
SAFE_SLUG_RE = re.compile(r"[^a-z0-9._-]+")
SHA_RE = re.compile(r"^[0-9a-f]{7,40}$")

class BotError(RuntimeError):
    """Expected operational failure, with credentials already redacted."""


class MutationUnsupported(BotError):
    """The controller discovered an edge but cannot safely rewrite it directly."""


@dataclass(frozen=True, order=True)
class SemVer:
    major: int
    minor: int
    patch: int

    @classmethod
    def parse(cls, value: str) -> "SemVer | None":
        match = STABLE_SEMVER_RE.fullmatch(value.strip())
        if not match:
            return None
        return cls(*(int(part) for part in match.groups()))

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"


@dataclass(frozen=True)
class PortfolioProject:
    portfolio_key: str
    github_org: str
    linear_project_id: str
    linear_project_name: str
    linear_project_url: str


@dataclass(frozen=True)
class Repository:
    full_name: str
    default_branch: str
    clone_url: str
    archived: bool = False
    fork: bool = False

    @property
    def owner(self) -> str:
        return self.full_name.split("/", 1)[0]

    @property
    def name(self) -> str:
        return self.full_name.split("/", 1)[1]


@dataclass(frozen=True)
class DependencyEdge:
    source_repo: str
    source_path: str
    kind: str
    dependency_key: str
    target_repo: str | None
    target_url: str | None
    current_version: str | None = None
    current_sha: str | None = None
    tracked_branch: str | None = None
    input_name: str | None = None
    metadata: Mapping[str, Any] = field(default_factory=dict)

    @property
    def stable_key(self) -> str:
        material = "\n".join(
            (
                self.source_repo,
                self.kind,
                self.source_path,
                self.dependency_key,
                self.target_repo or self.target_url or "unknown",
            )
        )
        return hashlib.sha256(material.encode("utf-8")).hexdigest()[:20]


@dataclass(frozen=True)
class Candidate:
    display: str
    version: SemVer | None = None
    git_sha: str | None = None
    git_ref: str | None = None
    change_class: str = "minor"


@dataclass
class TestOutcome:
    passed: bool
    profile: str | None
    job_id: str | None = None
    status: str = "not_run"
    detail: str = ""
    logs: str = ""


@dataclass
class EdgeResult:
    source_repo: str
    dependency_key: str
    kind: str
    status: str
    detail: str = ""
    old: str | None = None
    new: str | None = None
    pull_request_url: str | None = None
    linear_issue_url: str | None = None
    test_profile: str | None = None
    tested_candidates: list[Mapping[str, Any]] = field(default_factory=list)


@dataclass
class RunReport:
    started_at: str
    mode: str
    organizations: list[str]
    nodes: set[str] = field(default_factory=set)
    edges: list[DependencyEdge] = field(default_factory=list)
    results: list[EdgeResult] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    finished_at: str | None = None

    def json_value(self) -> Mapping[str, Any]:
        return {
            "schemaVersion": 1,
            "startedAt": self.started_at,
            "finishedAt": self.finished_at,
            "mode": self.mode,
            "organizations": self.organizations,
            "nodes": sorted(self.nodes),
            "edges": [asdict(edge) | {"stable_key": edge.stable_key} for edge in self.edges],
            "results": [asdict(result) for result in self.results],
            "errors": self.errors,
        }


@dataclass
class RuntimeState:
    report: RunReport
    maximum_pull_requests: int
    lock: threading.Lock = field(default_factory=threading.Lock)
    pull_requests_opened: int = 0

    def add_node(self, value: str) -> None:
        with self.lock:
            self.report.nodes.add(value)

    def add_edges(self, values: Iterable[DependencyEdge]) -> None:
        with self.lock:
            self.report.edges.extend(values)

    def add_result(self, value: EdgeResult) -> None:
        with self.lock:
            self.report.results.append(value)

    def add_error(self, value: str) -> None:
        with self.lock:
            self.report.errors.append(value)

    def reserve_pull_request(self) -> bool:
        with self.lock:
            if self.pull_requests_opened >= self.maximum_pull_requests:
                return False
            self.pull_requests_opened += 1
            return True


@dataclass(frozen=True)
class Policy:
    raw: Mapping[str, Any]

    @property
    def branch_patterns(self) -> tuple[str, ...]:
        return tuple(self.raw["updates"]["trackedBranches"])

    @property
    def branch_prefix(self) -> str:
        return str(self.raw["pullRequests"]["branchPrefix"])

    @property
    def managed_marker(self) -> str:
        return str(self.raw["pullRequests"]["managedMarker"])

    @property
    def max_candidates(self) -> int:
        return int(self.raw["updates"]["maxCandidatesPerDependency"])

    @property
    def org_concurrency(self) -> int:
        return int(self.raw["scope"]["maxOrganizationsInFlight"])

    @property
    def max_repositories_per_org(self) -> int:
        return int(self.raw["scope"]["maxRepositoriesPerOrganization"])

    @property
    def maximum_pull_requests(self) -> int:
        return int(self.raw["pullRequests"]["maximumPerRun"])

    @property
    def worker_poll_seconds(self) -> int:
        return int(self.raw["testing"]["pollSeconds"])

    @property
    def worker_timeout_seconds(self) -> int:
        return int(self.raw["testing"]["timeoutSeconds"])

    @property
    def repair_attempts(self) -> int:
        return int(self.raw["testing"]["repairAttempts"])

    def branch_is_minor_lane(self, branch: str) -> bool:
        return any(fnmatch.fnmatchcase(branch, pattern) for pattern in self.branch_patterns)


def load_policy(path: Path) -> Policy:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise BotError(f"cannot load policy {path}: {exc}") from exc
    if not isinstance(raw, Mapping):
        raise BotError("dependency policy must be a JSON object")
    errors = validate_policy(raw)
    if errors:
        raise BotError("invalid dependency policy: " + "; ".join(errors))
    return Policy(raw)


def validate_policy(raw: Mapping[str, Any]) -> list[str]:
    errors: list[str] = []
    if raw.get("schemaVersion") != 1:
        errors.append("schemaVersion must equal 1")
    schedule = raw.get("schedule")
    if not isinstance(schedule, Mapping):
        errors.append("schedule must be an object")
    else:
        if schedule.get("cron") != "0 2 * * *":
            errors.append("schedule.cron must remain 0 2 * * *")
        if schedule.get("timezone") != "America/Chicago":
            errors.append("schedule.timezone must remain America/Chicago")
    updates = raw.get("updates")
    if not isinstance(updates, Mapping):
        errors.append("updates must be an object")
    else:
        if updates.get("allowPatchOnly") is not False:
            errors.append("patch-only updates must remain disabled")
        if updates.get("allowMinor") is not True:
            errors.append("minor updates must remain enabled")
        if updates.get("allowMajor") is not False:
            errors.append("major updates must remain disabled")
        if updates.get("majorDisposition") != "linear_issue":
            errors.append("major releases must always become Linear issues")
        if updates.get("branchTipAdvancementIsMinor") is not True:
            errors.append("branch tip advancement must remain a minor lane")
        patterns = updates.get("trackedBranches")
        if not isinstance(patterns, list) or not {"main", "master", "release"}.issubset(
            set(str(value) for value in patterns)
        ):
            errors.append("trackedBranches must include main, master, and release")
    pull_requests = raw.get("pullRequests")
    if not isinstance(pull_requests, Mapping):
        errors.append("pullRequests must be an object")
    else:
        prefix = pull_requests.get("branchPrefix")
        if not isinstance(prefix, str) or not prefix.startswith("bot/"):
            errors.append("pullRequests.branchPrefix must be bot-owned")
        if pull_requests.get("closeSuperseded") is not True:
            errors.append("superseded managed PRs must be closed")
    return errors


def load_portfolio_projects(path: Path) -> list[PortfolioProject]:
    try:
        with path.open(newline="", encoding="utf-8") as handle:
            rows = list(csv.DictReader(handle))
    except OSError as exc:
        raise BotError(f"cannot read portfolio registry {path}: {exc}") from exc
    required = {
        "portfolio_key",
        "github_org",
        "linear_project_id",
        "linear_project_name",
        "linear_project_url",
    }
    if not rows:
        raise BotError("portfolio registry is empty")
    if not required.issubset(rows[0]):
        raise BotError("portfolio registry is missing dependency-bot columns")
    projects = [
        PortfolioProject(
            portfolio_key=row["portfolio_key"].strip(),
            github_org=row["github_org"].strip(),
            linear_project_id=row["linear_project_id"].strip(),
            linear_project_name=row["linear_project_name"].strip(),
            linear_project_url=row["linear_project_url"].strip(),
        )
        for row in rows
    ]
    if len({project.github_org.lower() for project in projects}) != len(projects):
        raise BotError("portfolio registry contains duplicate GitHub organizations")
    return projects


def strip_version_operator(value: str) -> str:
    value = value.strip()
    while value and value[0] in "^~=> <":
        value = value[1:].lstrip()
    return value


def select_semver_candidates(
    current: SemVer,
    tagged_versions: Iterable[tuple[SemVer, str, str]],
) -> tuple[list[Candidate], list[Candidate], list[Candidate]]:
    """Return (minor, patch-only, major) candidates.

    Exactly one release is retained per newer minor line: the highest patch in that line.
    Patch-only releases are reported for evidence but are never eligible.
    """

    newest_by_minor: dict[tuple[int, int], tuple[SemVer, str, str]] = {}
    patch_only: list[Candidate] = []
    major: list[Candidate] = []
    for version, tag, sha in tagged_versions:
        if version <= current:
            continue
        if version.major > current.major:
            major.append(Candidate(tag, version, sha, tag, "major"))
            continue
        if version.major < current.major:
            continue
        if version.minor == current.minor:
            patch_only.append(Candidate(tag, version, sha, tag, "patch"))
            continue
        key = (version.major, version.minor)
        previous = newest_by_minor.get(key)
        if previous is None or version > previous[0]:
            newest_by_minor[key] = (version, tag, sha)
    minor = [
        Candidate(tag, version, sha, tag, "minor")
        for version, tag, sha in sorted(newest_by_minor.values(), key=lambda item: item[0])
    ]
    patch_only.sort(key=lambda candidate: candidate.version or SemVer(0, 0, 0))
    major.sort(key=lambda candidate: candidate.version or SemVer(0, 0, 0))
    return minor, patch_only, major


def find_highest_passing(
    candidates: Sequence[Candidate],
    test: Callable[[Candidate], bool],
) -> tuple[int | None, Mapping[int, bool]]:
    """Bisect a presumed pass-prefix, then verify downward from newest.

    Dependency compatibility is often monotone but not guaranteed. The first phase performs the
    requested binary search. The second phase checks every newer untested candidate from newest
    downward, preserving correctness when a later version fixes an intermediate regression.
    """

    outcomes: dict[int, bool] = {}

    def run(index: int) -> bool:
        if index not in outcomes:
            outcomes[index] = bool(test(candidates[index]))
        return outcomes[index]

    low = 0
    high = len(candidates) - 1
    best: int | None = None
    while low <= high:
        middle = (low + high) // 2
        if run(middle):
            best = middle
            low = middle + 1
        else:
            high = middle - 1

    floor = -1 if best is None else best
    for index in range(len(candidates) - 1, floor, -1):
        if run(index):
            return index, outcomes
    return best, outcomes


def normalize_github_repo_url(value: str, source_owner: str | None = None) -> str | None:
    value = value.strip()
    if not value:
        return None
    if value.startswith("../") and source_owner:
        name = value[3:]
        if name.endswith(".git"):
            name = name[:-4]
        return f"{source_owner}/{name.strip('/')}"
    if value.startswith("git@github.com:"):
        path = value[len("git@github.com:") :]
    elif value.startswith("ssh://git@github.com/"):
        path = value[len("ssh://git@github.com/") :]
    elif value.startswith("https://github.com/"):
        path = value[len("https://github.com/") :]
    elif value.startswith("http://github.com/"):
        path = value[len("http://github.com/") :]
    else:
        return None
    path = path.split("?", 1)[0].split("#", 1)[0].strip("/")
    if path.endswith(".git"):
        path = path[:-4]
    parts = path.split("/")
    if len(parts) != 2 or not all(parts):
        return None
    return f"{parts[0]}/{parts[1]}"


def worker_repository_url(repository: Repository) -> str:
    """Return the canonical HTTPS URL used by the worker policy.

    GitHub repository paths are case-insensitive, while the worker's prefix rules are
    intentionally byte-for-byte. Lower-casing the owner/repository identity makes the
    fleet allowlist deterministic without putting credentials in the URL.
    """

    return f"https://github.com/{repository.full_name.lower()}.git"


def safe_slug(value: str, maximum: int = 48) -> str:
    slug = SAFE_SLUG_RE.sub("-", value.lower()).strip("-._")
    return (slug or "dependency")[:maximum]


def managed_marker(edge: DependencyEdge) -> str:
    return f"{MANAGED_COMMENT_PREFIX} key={edge.stable_key} -->"


def pr_is_managed(
    body: str | None,
    head_ref: str | None,
    edge: DependencyEdge,
    branch_prefix: str,
) -> bool:
    return bool(
        body
        and head_ref
        and head_ref.startswith(branch_prefix)
        and managed_marker(edge) in body
    )


def run_command(
    argv: Sequence[str],
    *,
    cwd: Path | None = None,
    env: Mapping[str, str] | None = None,
    timeout: int = 900,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    try:
        completed = subprocess.run(
            list(argv),
            cwd=cwd,
            env=dict(env) if env is not None else None,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise BotError(f"command {argv[0]!r} failed to start or timed out: {exc}") from exc
    if check and completed.returncode != 0:
        output = completed.stdout[-6000:]
        raise BotError(f"command {argv[0]!r} exited {completed.returncode}: {output}")
    return completed
