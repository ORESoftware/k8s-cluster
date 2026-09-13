"""Repository inventory, baseline testing, and escalation orchestration."""

from __future__ import annotations

from .models import *
from .providers import *
from .runtime import *
from .scanners import *
from .operations import *

class RepositoryControllerBase:
    def __init__(
        self,
        *,
        github: GitHubClient,
        linear: LinearClient,
        portfolio: Sequence[PortfolioLink],
        work_root: Path,
        artifacts: Path,
        apply: bool,
        max_prs: int,
        remediation_endpoint: str | None,
        remediation_token: str | None,
        remediation_command: str | None,
        allow_external: bool,
    ) -> None:
        self.github = github
        self.linear = linear
        self.portfolio = list(portfolio)
        self.by_org = {item.github_org.lower(): item for item in portfolio}
        self.allowed_orgs = set(self.by_org)
        self.work_root = work_root
        self.artifacts = artifacts
        self.apply = apply
        self.max_prs = max_prs
        self.remediation_endpoint = remediation_endpoint
        self.remediation_token = remediation_token
        self.remediation_command = remediation_command
        self.allow_external = allow_external
        self.edges: list[DependencyRef] = []
        self.summaries: list[RepoSummary] = []
        self._lock = threading.Lock()
        self._pr_count = 0
        self.provider_errors: list[str] = []
        self.ticket_intents: list[TicketIntent] = []
        self.pull_intents: list[PullIntent] = []
        self._ticket_markers: set[str] = set()
        self._pull_keys: set[tuple[str, str, str]] = set()

    def reserve_pr(self) -> bool:
        with self._lock:
            if self._pr_count >= self.max_prs:
                return False
            self._pr_count += 1
            return True

    def record_edges(self, edges: Sequence[DependencyRef]) -> None:
        with self._lock:
            self.edges.extend(edges)

    def record_summary(self, summary: RepoSummary) -> None:
        with self._lock:
            self.summaries.append(summary)

    def record_provider_error(self, error: str) -> None:
        with self._lock:
            self.provider_errors.append(error)

    def internal_dependency(self, dep: DependencyRef) -> bool:
        coordinate = canonical_github_repo(dep.source_url)
        if not coordinate:
            return False
        owner = coordinate.split("/", 1)[0].lower()
        return owner in self.allowed_orgs

    def create_ticket(
        self,
        *,
        link: PortfolioLink,
        category: str,
        repository: str,
        dep: DependencyRef,
        target: str,
        title: str,
        description: str,
    ) -> str:
        marker = issue_marker(category, repository, dep, target)
        intent = TicketIntent(
            project_id=link.linear_project_id,
            project_name=link.linear_project_name,
            repository=repository,
            category=category,
            dependency_key=dep.key,
            target=target,
            title=title,
            description=description,
            marker=marker,
        )
        with self._lock:
            if marker not in self._ticket_markers:
                self._ticket_markers.add(marker)
                self.ticket_intents.append(intent)
        if not self.apply:
            return f"planned:{marker}"
        return self.linear.ensure_issue(
            project_id=link.linear_project_id,
            title=title,
            description=description,
            marker=marker,
        )

    def process_repository(self, metadata: Mapping[str, Any]) -> RepoSummary:
        full_name = str(metadata["full_name"])
        summary = RepoSummary(repository=full_name)
        owner = full_name.split("/", 1)[0]
        link = self.by_org[owner.lower()]
        default_branch = str(metadata.get("default_branch") or "main")
        try:
            base_sha = self.github.branch_sha(full_name, default_branch)
            summary.base_sha = base_sha
            destination = self.work_root / safe_slug(full_name, 60)
            if destination.exists():
                shutil.rmtree(destination)
            clone_exact_repository(
                full_name=full_name,
                clone_url=str(metadata["clone_url"]),
                branch=default_branch,
                sha=base_sha,
                token=self.github.token,
                destination=destination,
            )
            edges = scan_repository(destination, full_name)
            summary.edges = len(edges)
            summary.manifests = len({edge.manifest_path for edge in edges})
            self.record_edges(edges)
            policy = load_repo_policy(destination)
            actionable = [
                edge
                for edge in edges
                if edge.kind != "zpkg-lock"
                and edge.key not in policy.excluded_dependencies
                and (self.internal_dependency(edge) or self.allow_external)
            ]
            if not actionable:
                summary.status = "inventoried"
                return summary

            # Major-release escalation is independent of test health. Inventory
            # every actionable edge and file the mapped Linear issue before a
            # missing or failing repository test contract can stop minor work.
            version_catalog: dict[int, tuple[list[RemoteVersion], SemVer]] = {}
            for dep in actionable:
                versions = remote_versions(dep.source_url, token=self.github.token)
                if not versions:
                    summary.warnings.append(f"{dep.key}: no stable SemVer tags")
                    continue
                current = resolve_current_version(dep, versions)
                if not current:
                    summary.warnings.append(f"{dep.key}: current version cannot be resolved")
                    ticket = self.create_ticket(
                        link=link,
                        category="unresolved-version",
                        repository=full_name,
                        dep=dep,
                        target="unresolved",
                        title=(
                            f"[dependency-steward] {full_name}: resolve the current "
                            f"version of {dep.name}"
                        ),
                        description=(
                            "Stable SemVer tags exist, but the manifest's current ref cannot "
                            "be mapped to one. The controller cannot safely distinguish patch, "
                            "minor, and major movement until the pin is made explicit.\n\n"
                            f"- Kind: `{dep.kind}`\n- Manifest: `{dep.manifest_path}`\n"
                            f"- Current ref: `{dep.current_ref}`\n"
                            f"- Exact base SHA: `{base_sha}`"
                        ),
                    )
                    summary.tickets.append(ticket)
                    continue
                dep.current_version = current
                version_catalog[id(dep)] = (versions, current)
                major = newer_major_versions(current, versions)
                if major:
                    latest_major = major[-1]
                    ticket = self.create_ticket(
                        link=link,
                        category="major-upgrade",
                        repository=dep.repository,
                        dep=dep,
                        target=f"major-{latest_major.version.major}",
                        title=(
                            f"[dependency-major] {dep.repository}: {dep.name} "
                            f"{current} → {latest_major.version}"
                        ),
                        description=(
                            "A major dependency release exists. Policy forbids automated major "
                            "changes, even when tests might pass. Plan migration, compatibility, "
                            "rollout, and rollback explicitly.\n\n"
                            f"- Dependency kind: `{dep.kind}`\n"
                            f"- Manifest: `{dep.manifest_path}`\n"
                            f"- Current: `{current}`\n"
                            f"- Latest observed major: `{latest_major.version}` "
                            f"(`{latest_major.tag}`)\n"
                            f"- Exact repository SHA: `{base_sha}`"
                        ),
                    )
                    summary.tickets.append(ticket)

            if not policy.test_commands:
                synthetic = actionable[0]
                ticket = self.create_ticket(
                    link=link,
                    category="missing-tests",
                    repository=full_name,
                    dep=synthetic,
                    target="test-contract",
                    title=f"[dependency-steward] {full_name}: define a test contract",
                    description=(
                        "The nightly dependency steward found mutable dependency edges but "
                        "could not identify a repository test command. It will never treat "
                        "the absence of tests as success. Add `[dependency_steward].test_commands` "
                        "to `.dependency-steward.toml` or a recognized project test entry.\n\n"
                        f"Exact base SHA: `{base_sha}`"
                    ),
                )
                summary.tickets.append(ticket)
                summary.status = "ticketed-missing-tests"
                return summary

            baseline = run_shell_commands(
                [*policy.prepare_commands, *policy.test_commands],
                cwd=destination,
                timeout_seconds=policy.timeout_seconds,
                env={"CI": "1"},
            )
            if not baseline.passed:
                synthetic = actionable[0]
                ticket = self.create_ticket(
                    link=link,
                    category="baseline-failure",
                    repository=full_name,
                    dep=synthetic,
                    target=base_sha[:12],
                    title=f"[dependency-steward] {full_name}: baseline tests fail",
                    description=(
                        "Dependency upgrades are blocked because the exact default-branch "
                        "head does not pass its own detected test contract.\n\n"
                        f"Exact base SHA: `{base_sha}`\n\n"
                        f"Failed command: `{baseline.command}`\n\n"
                        f"```text\n{baseline.log_tail[-8000:]}\n```"
                    ),
                )
                summary.tickets.append(ticket)
                summary.status = "ticketed-baseline-failure"
                return summary

            for dep in actionable:
                catalog = version_catalog.get(id(dep))
                if catalog is None:
                    continue
                versions, current = catalog
                try:
                    self.process_dependency(
                        destination=destination,
                        metadata=metadata,
                        default_branch=default_branch,
                        base_sha=base_sha,
                        link=link,
                        policy=policy,
                        dep=dep,
                        versions=versions,
                        current=current,
                        summary=summary,
                    )
                except Exception as exc:  # continue other independent edges
                    message = redact(f"{dep.key}: {exc}")
                    summary.errors.append(message)
                    try:
                        ticket = self.create_ticket(
                            link=link,
                            category="controller-error",
                            repository=full_name,
                            dep=dep,
                            target="controller-error",
                            title=f"[dependency-steward] {full_name}: {dep.name} controller error",
                            description=(
                                "The nightly dependency steward could not safely complete this "
                                "edge. No unverified PR was opened.\n\n"
                                f"Exact base SHA: `{base_sha}`\n\n"
                                f"```text\n{message[-8000:]}\n```"
                            ),
                        )
                        summary.tickets.append(ticket)
                    except Exception as ticket_exc:
                        self.record_provider_error(
                            f"Linear escalation failed for {full_name}/{dep.key}: {ticket_exc}"
                        )
            summary.status = "completed" if not summary.errors else "completed-with-errors"
            return summary
        except Exception as exc:
            summary.status = "failed"
            summary.errors.append(redact(str(exc)))
            self.record_provider_error(f"repository failed: {full_name}: {exc}")
            return summary

