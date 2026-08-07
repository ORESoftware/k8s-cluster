"""Minor-candidate probing, compatibility remediation, and managed PR planning."""

from __future__ import annotations

from .models import *
from .providers import *
from .runtime import *
from .scanners import *
from .operations import *

class DependencyProcessingMixin:
    def process_dependency(
        self,
        *,
        destination: Path,
        metadata: Mapping[str, Any],
        default_branch: str,
        base_sha: str,
        link: PortfolioLink,
        policy: RepoPolicy,
        dep: DependencyRef,
        versions: Sequence[RemoteVersion],
        current: SemVer,
        summary: RepoSummary,
    ) -> None:
        patches = patch_only_versions(current, versions)
        if patches:
            summary.warnings.append(
                f"{dep.key}: ignored {len(patches)} patch-only release(s); newest {patches[-1].version}"
            )

        candidates = minor_line_candidates(current, versions)
        if not candidates:
            return
        if not dep.mutable and not policy.lock_commands:
            latest = candidates[-1]
            ticket = self.create_ticket(
                link=link,
                category="unsupported-minor-edge",
                repository=dep.repository,
                dep=dep,
                target=str(latest.version),
                title=(
                    f"[dependency-minor-blocked] {dep.repository}: "
                    f"{dep.name} {current} → {latest.version}"
                ),
                description=(
                    "A minor release exists, but this generic manifest edge has no safe, "
                    "repository-owned update command. Add `lock_commands` in "
                    "`.dependency-steward.toml`; do not hand-edit content hashes.\n\n"
                    f"- Kind: `{dep.kind}`\n- Manifest: `{dep.manifest_path}`\n"
                    f"- Exact base SHA: `{base_sha}`"
                ),
            )
            summary.tickets.append(ticket)
            return

        attempts: list[ProbeResult] = []
        attempt_by_version: dict[str, ProbeResult] = {}

        def probe(candidate: RemoteVersion) -> bool:
            key = str(candidate.version)
            if key in attempt_by_version:
                return attempt_by_version[key].passed
            reset_worktree(destination, base_sha)
            started = time.monotonic()
            try:
                apply_dependency(
                    destination, dep, candidate, token=self.github.token, policy=policy
                )
                result = run_shell_commands(
                    [*policy.prepare_commands, *policy.test_commands],
                    cwd=destination,
                    timeout_seconds=policy.timeout_seconds,
                    env={"CI": "1"},
                )
            except Exception as exc:
                result = CommandResult(
                    False,
                    "apply/update",
                    redact(str(exc)),
                    time.monotonic() - started,
                )
            item = ProbeResult(
                version=key,
                passed=result.passed,
                command=result.command,
                log_tail=result.log_tail,
                duration_seconds=time.monotonic() - started,
            )
            attempts.append(item)
            attempt_by_version[key] = item
            return item.passed

        best, _, non_monotonic = bisect_highest_passing(candidates, probe)
        if non_monotonic:
            summary.warnings.append(f"{dep.key}: non-monotonic test frontier; used fallback scan")

        latest = candidates[-1]
        remediated_patch: str | None = None
        latest_result = attempt_by_version.get(str(latest.version))
        if latest_result is None:
            probe(latest)
            latest_result = attempt_by_version[str(latest.version)]

        if not latest_result.passed:
            reset_worktree(destination, base_sha)
            apply_dependency(destination, dep, latest, token=self.github.token, policy=policy)
            failed = CommandResult(
                False,
                latest_result.command,
                latest_result.log_tail,
                latest_result.duration_seconds,
            )
            changed, patch = try_remediation(
                root=destination,
                repository=dep.repository,
                base_sha=base_sha,
                dep=dep,
                target=latest,
                policy=policy,
                failed=failed,
                endpoint=self.remediation_endpoint,
                endpoint_token=self.remediation_token,
                global_command=self.remediation_command,
            )
            if changed:
                retest = run_shell_commands(
                    [*policy.prepare_commands, *policy.test_commands],
                    cwd=destination,
                    timeout_seconds=policy.timeout_seconds,
                    env={"CI": "1"},
                )
                attempts.append(
                    ProbeResult(
                        version=str(latest.version),
                        passed=retest.passed,
                        command=retest.command,
                        log_tail=retest.log_tail,
                        duration_seconds=retest.duration_seconds,
                        remediated=True,
                    )
                )
                if retest.passed:
                    best = latest
                    remediated_patch = patch

        if best is None:
            ticket = self.create_ticket(
                link=link,
                category="minor-upgrade-blocked",
                repository=dep.repository,
                dep=dep,
                target=str(latest.version),
                title=(
                    f"[dependency-minor-blocked] {dep.repository}: "
                    f"{dep.name} {current} → {latest.version}"
                ),
                description=(
                    "All eligible minor-line candidates failed, and bounded compatibility "
                    "remediation did not produce a verified patch.\n\n"
                    f"- Kind: `{dep.kind}`\n- Manifest: `{dep.manifest_path}`\n"
                    f"- Exact base SHA: `{base_sha}`\n\n"
                    f"{format_attempts(attempts)}\n\n"
                    f"Last log:\n```text\n{attempts[-1].log_tail[-8000:]}\n```"
                ),
            )
            summary.tickets.append(ticket)
            return

        selected = next(item for item in candidates if item.version == best.version)
        reset_worktree(destination, base_sha)
        apply_dependency(destination, dep, selected, token=self.github.token, policy=policy)
        if remediated_patch and selected.version == latest.version:
            patch_path = destination / ".git" / "dependency-steward-selected.patch"
            patch_path.write_text(remediated_patch, encoding="utf-8")
            run_process(["git", "apply", "--check", str(patch_path)], cwd=destination)
            run_process(["git", "apply", str(patch_path)], cwd=destination)
        final = run_shell_commands(
            [*policy.prepare_commands, *policy.test_commands],
            cwd=destination,
            timeout_seconds=policy.timeout_seconds,
            env={"CI": "1"},
        )
        if not final.passed:
            raise StewardError(
                f"selected candidate {selected.version} did not reproduce:\n{final.log_tail}"
            )
        if not changed_files(destination):
            raise StewardError(f"candidate {selected.version} produced no tracked change")

        if selected.version < latest.version:
            blocked = self.create_ticket(
                link=link,
                category="newer-minor-blocked",
                repository=dep.repository,
                dep=dep,
                target=str(latest.version),
                title=(
                    f"[dependency-minor-partial] {dep.repository}: {dep.name} "
                    f"passes at {selected.version}, blocked at {latest.version}"
                ),
                description=(
                    "The steward will open the newest verified passing minor upgrade, but "
                    "a newer minor line remains incompatible.\n\n"
                    f"{format_attempts(attempts)}\n\nExact base SHA: `{base_sha}`"
                ),
            )
            summary.tickets.append(blocked)

        if not self.reserve_pr():
            ticket = self.create_ticket(
                link=link,
                category="nightly-pr-cap",
                repository=dep.repository,
                dep=dep,
                target=str(selected.version),
                title=f"[dependency-steward] {dep.repository}: verified update deferred by cap",
                description=(
                    f"A verified `{dep.name}` minor update to `{selected.version}` was found, "
                    "but the nightly PR safety cap was reached. It will be retried on the "
                    "next run."
                ),
            )
            summary.tickets.append(ticket)
            return

        branch = branch_name(dep, selected.version)
        title = f"chore(deps): bump {dep.name} to {selected.version}"
        body = pr_body(
            repository=dep.repository,
            dep=dep,
            current=current,
            target=selected,
            base_sha=base_sha,
            tests=policy.test_commands,
            attempts=attempts,
            remediated=bool(remediated_patch),
        )

        if not self.apply:
            patch = git_diff(destination)
            if not patch.strip():
                raise StewardError("refusing to plan an empty dependency update")
            validate_patch(patch)
            patch_sha = hashlib.sha256(patch.encode()).hexdigest()
            patch_relative = f"patches/{patch_sha}.patch"
            intent = PullIntent(
                repository=dep.repository,
                default_branch=default_branch,
                base_sha=base_sha,
                dependency=dep.graph_dict(),
                current_version=str(current),
                target_version=str(selected.version),
                target_tag=selected.tag,
                target_sha=selected.sha,
                branch=branch,
                title=title,
                body=body,
                patch_path=patch_relative,
                patch_sha256=patch_sha,
            )
            pull_key = (dep.repository, dep.key, str(selected.version))
            with self._lock:
                patch_file = self.artifacts / patch_relative
                patch_file.parent.mkdir(parents=True, exist_ok=True)
                patch_file.write_text(patch, encoding="utf-8")
                if pull_key not in self._pull_keys:
                    self._pull_keys.add(pull_key)
                    self.pull_intents.append(intent)
            summary.prs.append(f"planned:{dep.repository}:{branch}")
            return

        latest_base = self.github.branch_sha(dep.repository, default_branch)
        if latest_base != base_sha:
            raise StewardError(
                f"default branch moved during test: {base_sha} -> {latest_base}; retry next run"
            )
        push_branch(
            root=destination,
            branch=branch,
            base_sha=base_sha,
            token=self.github.token,
            message=title,
        )
        pulls = self.github.open_pulls(dep.repository)
        existing = next(
            (
                pull
                for pull in pulls
                if str((pull.get("head") or {}).get("ref")) == branch
                and parse_pr_marker(str(pull.get("body") or ""))
            ),
            None,
        )
        if existing:
            number = int(existing["number"])
            pull = self.github.update_pull(
                dep.repository, number, title=title, body=body
            )
        else:
            pull = self.github.create_pull(
                dep.repository,
                title=title,
                head=branch,
                base=default_branch,
                body=body,
            )
            number = int(pull["number"])
        try:
            self.github.add_labels(
                dep.repository,
                number,
                ["dependencies", "dependency-steward", "minor-update"],
            )
        except StewardError as exc:
            summary.warnings.append(f"could not apply labels to PR #{number}: {exc}")
        summary.prs.append(str(pull.get("html_url") or f"{dep.repository}#{number}"))

        obsolete = managed_pr_numbers_to_close(
            pulls,
            dependency_key=dep.key,
            target=selected.version,
            keep_number=number,
        )
        for old_number in obsolete:
            self.github.comment(
                dep.repository,
                old_number,
                (
                    f"Closed by `{JOB_MARKER}` because PR #{number} supersedes this "
                    f"managed dependency update with verified target `{selected.version}`."
                ),
            )
            self.github.update_pull(dep.repository, old_number, state="closed")
            summary.closed_prs.append(f"{dep.repository}#{old_number}")

