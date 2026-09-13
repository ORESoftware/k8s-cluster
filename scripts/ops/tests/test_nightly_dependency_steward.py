from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

OPS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(OPS_DIR))

from nightly_dependency_steward import (  # noqa: E402
    DependencyRef,
    RemoteVersion,
    SemVer,
    StewardError,
    bisect_highest_passing,
    display_command,
    graph_to_dot,
    managed_pr_numbers_to_close,
    minor_line_candidates,
    newer_major_versions,
    parse_pr_marker,
    patch_only_versions,
    pr_marker,
    push_branch,
    scan_flake_lock,
    scan_nix_manifest,
    scan_zpkg_manifest,
    sanitized_environment,
    scheduled_cron_is_active,
    update_zpkg_dependency,
    validate_patch,
)

from publish_nightly_dependency_steward import (  # noqa: E402
    load_plan,
    resolve_patch,
)


def version(value: str, sha: str | None = None) -> RemoteVersion:
    parsed = SemVer.parse(value)
    assert parsed is not None
    return RemoteVersion(parsed, f"v{parsed}", sha or (str(parsed.major) * 40)[:40])


class SemVerPolicyTests(unittest.TestCase):
    def test_parser_accepts_stable_tags_and_rejects_prereleases(self) -> None:
        self.assertEqual(SemVer.parse("v1.2.3"), SemVer(1, 2, 3))
        self.assertEqual(SemVer.parse("^0.4.8"), SemVer(0, 4, 8))
        self.assertIsNone(SemVer.parse("v2.0.0-rc.1"))

    def test_minor_candidates_ignore_patch_only_and_major(self) -> None:
        current = SemVer(1, 2, 3)
        available = [
            version("1.2.4"),
            version("1.3.0"),
            version("1.3.8"),
            version("1.4.1"),
            version("2.0.0"),
        ]
        self.assertEqual(
            [item.version for item in minor_line_candidates(current, available)],
            [SemVer(1, 3, 8), SemVer(1, 4, 1)],
        )
        self.assertEqual(
            [item.version for item in patch_only_versions(current, available)],
            [SemVer(1, 2, 4)],
        )
        self.assertEqual(
            [item.version for item in newer_major_versions(current, available)],
            [SemVer(2, 0, 0)],
        )

    def test_zero_major_still_uses_minor_policy(self) -> None:
        current = SemVer(0, 1, 9)
        available = [version("0.1.10"), version("0.2.0"), version("1.0.0")]
        self.assertEqual(
            [item.version for item in minor_line_candidates(current, available)],
            [SemVer(0, 2, 0)],
        )
        self.assertEqual(
            [item.version for item in newer_major_versions(current, available)],
            [SemVer(1, 0, 0)],
        )


class BisectTests(unittest.TestCase):
    def test_finds_highest_passing_monotonic_frontier(self) -> None:
        candidates = [version(f"1.{minor}.0") for minor in range(3, 10)]
        probed: list[SemVer] = []

        def probe(item: RemoteVersion) -> bool:
            probed.append(item.version)
            return item.version.minor <= 7

        best, attempts, fallback = bisect_highest_passing(candidates, probe)
        self.assertEqual(best.version if best else None, SemVer(1, 7, 0))
        self.assertFalse(fallback)
        self.assertLess(len(attempts), len(candidates) + 1)
        self.assertIn(SemVer(1, 7, 0), probed)

    def test_non_monotonic_results_trigger_descending_fallback(self) -> None:
        candidates = [version(f"1.{minor}.0") for minor in range(3, 8)]
        passing = {3, 4, 7}
        best, _, fallback = bisect_highest_passing(
            candidates, lambda item: item.version.minor in passing
        )
        self.assertTrue(fallback)
        self.assertEqual(best.version if best else None, SemVer(1, 7, 0))


class ScheduleTests(unittest.TestCase):
    def test_summer_lane_is_0700_utc(self) -> None:
        now = datetime(2026, 8, 6, 20, tzinfo=timezone.utc)
        self.assertTrue(scheduled_cron_is_active(now, "0 7 * * *"))
        self.assertFalse(scheduled_cron_is_active(now, "0 8 * * *"))

    def test_winter_lane_is_0800_utc(self) -> None:
        now = datetime(2026, 1, 6, 20, tzinfo=timezone.utc)
        self.assertFalse(scheduled_cron_is_active(now, "0 7 * * *"))
        self.assertTrue(scheduled_cron_is_active(now, "0 8 * * *"))

    def test_delayed_runner_uses_event_expression(self) -> None:
        delayed = datetime(2026, 8, 6, 23, tzinfo=timezone.utc)
        self.assertTrue(scheduled_cron_is_active(delayed, "0 7 * * *"))


class PullRequestOwnershipTests(unittest.TestCase):
    def test_only_controller_owned_superseded_prs_close(self) -> None:
        managed_old = pr_marker({"key": "zpkg:a/b", "target": "1.3.5"})
        managed_new = pr_marker({"key": "zpkg:a/b", "target": "1.5.0"})
        other_dep = pr_marker({"key": "zpkg:c/d", "target": "9.0.0"})
        pulls = [
            {"number": 1, "body": managed_old},
            {"number": 2, "body": managed_new},
            {"number": 3, "body": other_dep},
            {"number": 4, "body": "Dependabot PR without steward marker"},
        ]
        self.assertEqual(
            managed_pr_numbers_to_close(
                pulls,
                dependency_key="zpkg:a/b",
                target=SemVer(1, 4, 2),
            ),
            [1],
        )
        self.assertEqual(parse_pr_marker(managed_old)["target"], "1.3.5")


class ManifestTests(unittest.TestCase):
    def test_zpkg_manifest_scan_and_preserving_update(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / ".zpkg.toml"
            manifest.write_text(
                """[package]
name = "consumer"
version = "1.0.0"

[dependencies]
"alpha/interfaces" = "^1.2.3"
beta-lib = { version = "~0.4.1", git = "https://github.com/beta/lib.git" }
""",
                encoding="utf-8",
            )
            edges = scan_zpkg_manifest(manifest, root, "consumer/app")
            self.assertEqual(len(edges), 2)
            self.assertEqual(edges[0].current_version, SemVer(1, 2, 3))
            update_zpkg_dependency(manifest, "alpha/interfaces", SemVer(1, 5, 9))
            updated = manifest.read_text(encoding="utf-8")
            self.assertIn('"alpha/interfaces" = "^1.5.9"', updated)
            self.assertIn('version = "1.0.0"', updated)

    def test_flake_lock_scans_github_inputs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lock = root / "flake.lock"
            lock.write_text(
                json.dumps(
                    {
                        "nodes": {
                            "nixpkgs": {
                                "locked": {
                                    "type": "github",
                                    "owner": "NixOS",
                                    "repo": "nixpkgs",
                                    "rev": "a" * 40,
                                }
                            },
                            "local": {"locked": {"type": "path", "path": "./x"}},
                        }
                    }
                ),
                encoding="utf-8",
            )
            edges = scan_flake_lock(lock, root, "owner/repo")
            self.assertEqual(len(edges), 1)
            self.assertEqual(edges[0].locator["input"], "nixpkgs")
            self.assertEqual(edges[0].source_url, "https://github.com/NixOS/nixpkgs.git")

    def test_generic_nix_edges_are_graph_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / "deps.nix"
            manifest.write_text(
                """src = fetchFromGitHub {
  owner = "alpha"; repo = "lib"; rev = "v1.2.3";
  hash = "sha256-example";
};
other = "github:beta/tool/v0.5.0";
""",
                encoding="utf-8",
            )
            edges = scan_nix_manifest(manifest, root, "consumer/app")
            self.assertEqual(len(edges), 2)
            self.assertTrue(all(not edge.mutable for edge in edges))

    def test_graph_dot_escapes_labels(self) -> None:
        edge = DependencyRef(
            repository='owner/repo"x',
            kind="zpkg",
            key="zpkg:a/b",
            name="a/b",
            source_url="https://github.com/a/b.git",
            manifest_path=".zpkg.toml",
            current_version=SemVer(1, 2, 3),
        )
        dot = graph_to_dot([edge])
        self.assertIn('owner/repo\\"x', dot)
        self.assertIn('label="zpkg 1.2.3"', dot)


class RemediationBoundaryTests(unittest.TestCase):
    def test_rejects_workflow_and_parent_paths(self) -> None:
        with self.assertRaises(StewardError):
            validate_patch("--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n")
        with self.assertRaises(StewardError):
            validate_patch("--- a/../secret\n+++ b/../secret\n")

    def test_accepts_normal_source_patch(self) -> None:
        validate_patch("--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-a\n+b\n")


class CredentialBoundaryTests(unittest.TestCase):
    def test_child_environment_strips_provider_and_runner_secrets(self) -> None:
        previous = dict(os.environ)
        try:
            os.environ["DEPENDENCY_STEWARD_GITHUB_TOKEN"] = "secret"
            os.environ["LINEAR_API_KEY"] = "secret"
            os.environ["ACTIONS_RUNTIME_TOKEN"] = "secret"
            os.environ["SAFE_VALUE"] = "kept"
            child = sanitized_environment({"CI": "1"})
            self.assertNotIn("DEPENDENCY_STEWARD_GITHUB_TOKEN", child)
            self.assertNotIn("LINEAR_API_KEY", child)
            self.assertNotIn("ACTIONS_RUNTIME_TOKEN", child)
            self.assertEqual(child["SAFE_VALUE"], "kept")
            self.assertEqual(child["CI"], "1")
        finally:
            os.environ.clear()
            os.environ.update(previous)

    def test_git_extraheader_is_redacted_from_errors(self) -> None:
        rendered = display_command(
            [
                "git",
                "-c",
                "http.https://github.com/.extraheader=AUTHORIZATION: basic c2VjcmV0",
                "fetch",
            ]
        )
        self.assertIn("[REDACTED]", rendered)
        self.assertNotIn("c2VjcmV0", rendered)


class PublishPlanTests(unittest.TestCase):
    def test_plan_contract_and_patch_digest_are_verified(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            patches = root / "patches"
            patches.mkdir()
            patch = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-a\n+b\n"
            digest = hashlib.sha256(patch.encode()).hexdigest()
            (patches / f"{digest}.patch").write_text(patch, encoding="utf-8")
            plan_path = root / "publish-plan.json"
            plan_path.write_text(
                json.dumps(
                    {
                        "contract": "dependency-steward:v1",
                        "phase": "analyze",
                        "tickets": [],
                        "pull_requests": [],
                    }
                ),
                encoding="utf-8",
            )
            self.assertEqual(load_plan(plan_path)["phase"], "analyze")
            self.assertEqual(
                resolve_patch(plan_path, f"patches/{digest}.patch", digest), patch
            )
            with self.assertRaises(StewardError):
                resolve_patch(plan_path, f"patches/{digest}.patch", "0" * 64)

    def test_push_branch_preserves_verified_worktree_changes(self) -> None:
        if not shutil.which("git"):
            self.skipTest("git is unavailable")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            remote = root / "remote.git"
            repo = root / "repo"
            subprocess.run(["git", "init", "--bare", str(remote)], check=True, capture_output=True)
            subprocess.run(["git", "init", "-b", "main", str(repo)], check=True, capture_output=True)
            subprocess.run(["git", "config", "user.name", "Test"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.email", "test@example.com"], cwd=repo, check=True)
            (repo / "value.txt").write_text("old\n", encoding="utf-8")
            subprocess.run(["git", "add", "value.txt"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-m", "initial"], cwd=repo, check=True, capture_output=True)
            base = subprocess.run(
                ["git", "rev-parse", "HEAD"], cwd=repo, check=True, capture_output=True, text=True
            ).stdout.strip()
            subprocess.run(["git", "remote", "add", "origin", str(remote)], cwd=repo, check=True)
            subprocess.run(["git", "push", "origin", "main"], cwd=repo, check=True, capture_output=True)
            (repo / "value.txt").write_text("new\n", encoding="utf-8")
            push_branch(
                root=repo,
                branch="automation/dependency-minor/example/1.2",
                base_sha=base,
                token="not-a-real-token",
                message="chore: update",
            )
            observed = subprocess.run(
                [
                    "git",
                    f"--git-dir={remote}",
                    "show",
                    "automation/dependency-minor/example/1.2:value.txt",
                ],
                check=True,
                capture_output=True,
                text=True,
            ).stdout
            self.assertEqual(observed, "new\n")


if __name__ == "__main__":
    unittest.main()
