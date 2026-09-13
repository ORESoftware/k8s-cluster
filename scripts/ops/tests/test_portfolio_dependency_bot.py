from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve()
OPS = HERE.parents[1]
if str(OPS) not in sys.path:
    sys.path.insert(0, str(OPS))

from portfolio_dependency_bot import (  # noqa: E402
    Candidate,
    DependencyEdge,
    Policy,
    Repository,
    SemVer,
    detect_profile,
    discover_edges,
    find_highest_passing,
    load_policy,
    managed_marker,
    normalize_github_repo_url,
    pr_is_managed,
    replace_zpkg_git_pin,
    replace_zpkg_version,
    select_semver_candidates,
    validate_policy,
    worker_repository_url,
)


class SemVerPolicyTests(unittest.TestCase):
    def test_stable_semver_parser_rejects_prerelease_and_partial_versions(self) -> None:
        self.assertEqual(SemVer.parse("v1.2.3"), SemVer(1, 2, 3))
        self.assertEqual(SemVer.parse("0.9.0"), SemVer(0, 9, 0))
        self.assertIsNone(SemVer.parse("1.2"))
        self.assertIsNone(SemVer.parse("1.2.3-rc.1"))

    def test_minor_candidates_keep_highest_patch_per_new_minor(self) -> None:
        minor, patch, major = select_semver_candidates(
            SemVer(1, 2, 3),
            [
                (SemVer(1, 2, 4), "v1.2.4", "a" * 40),
                (SemVer(1, 3, 0), "v1.3.0", "b" * 40),
                (SemVer(1, 3, 7), "v1.3.7", "c" * 40),
                (SemVer(1, 4, 1), "v1.4.1", "d" * 40),
                (SemVer(2, 0, 0), "v2.0.0", "e" * 40),
            ],
        )
        self.assertEqual([str(item.version) for item in minor], ["1.3.7", "1.4.1"])
        self.assertEqual([str(item.version) for item in patch], ["1.2.4"])
        self.assertEqual([str(item.version) for item in major], ["2.0.0"])
        self.assertTrue(all(item.change_class == "minor" for item in minor))
        self.assertTrue(all(item.change_class == "patch" for item in patch))
        self.assertTrue(all(item.change_class == "major" for item in major))

    def test_binary_search_then_descending_verification_finds_later_recovery(self) -> None:
        candidates = [Candidate(str(value)) for value in range(8)]
        passing = {0, 1, 2, 6}
        best, outcomes = find_highest_passing(
            candidates, lambda candidate: int(candidate.display) in passing
        )
        self.assertEqual(best, 6)
        self.assertTrue(outcomes[6])
        self.assertIn(7, outcomes)

    def test_binary_search_reports_no_candidate_when_all_fail(self) -> None:
        candidates = [Candidate("a"), Candidate("b"), Candidate("c")]
        best, outcomes = find_highest_passing(candidates, lambda _: False)
        self.assertIsNone(best)
        self.assertEqual(set(outcomes), {0, 1, 2})


class PolicyValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy_path = HERE.parents[3] / "ops" / "registries" / "portfolio-dependency-policy.json"

    def test_checked_in_policy_is_minor_only_and_timezone_aware(self) -> None:
        policy = load_policy(self.policy_path)
        self.assertTrue(policy.branch_is_minor_lane("main"))
        self.assertTrue(policy.branch_is_minor_lane("master"))
        self.assertTrue(policy.branch_is_minor_lane("release/2026.08"))
        self.assertFalse(policy.branch_is_minor_lane("develop"))
        self.assertFalse(policy.raw["updates"]["allowPatchOnly"])
        self.assertFalse(policy.raw["updates"]["allowMajor"])
        self.assertEqual(policy.raw["schedule"]["timezone"], "America/Chicago")

    def test_policy_rejects_patch_or_major_enablement(self) -> None:
        raw = json.loads(self.policy_path.read_text(encoding="utf-8"))
        raw["updates"]["allowPatchOnly"] = True
        raw["updates"]["allowMajor"] = True
        errors = validate_policy(raw)
        self.assertTrue(any("patch-only" in error for error in errors))
        self.assertTrue(any("major updates" in error for error in errors))


class DiscoveryTests(unittest.TestCase):
    def repository(self) -> Repository:
        return Repository(
            full_name="example/app",
            default_branch="main",
            clone_url="https://github.com/example/app.git",
        )

    def init_repo(self, root: Path) -> None:
        import subprocess

        subprocess.run(["git", "init", "-q", str(root)], check=True)
        subprocess.run(["git", "-C", str(root), "config", "user.name", "test"], check=True)
        subprocess.run(["git", "-C", str(root), "config", "user.email", "test@example.invalid"], check=True)

    def test_discovers_gitmodules_zpkg_and_nix_edges(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.init_repo(root)
            (root / ".gitmodules").write_text(
                """
[submodule "lib"]
  path = vendor/lib
  url = https://github.com/example/lib.git
  branch = main
""".strip()
                + "\n",
                encoding="utf-8",
            )
            (root / ".zpkg.toml").write_text(
                """
[package]
name = "app"
version = "0.1.0"

[dependencies]
"example/interfaces" = "^0.2.4"
""".strip()
                + "\n",
                encoding="utf-8",
            )
            (root / "flake.lock").write_text(
                json.dumps(
                    {
                        "version": 7,
                        "root": "root",
                        "nodes": {
                            "root": {"inputs": {"nixpkgs": "nixpkgs"}},
                            "nixpkgs": {
                                "inputs": {"lib": "lib"},
                                "locked": {
                                    "type": "github",
                                    "owner": "NixOS",
                                    "repo": "nixpkgs",
                                    "rev": "a" * 40,
                                    "narHash": "sha256-test",
                                },
                                "original": {
                                    "type": "github",
                                    "owner": "NixOS",
                                    "repo": "nixpkgs",
                                    "ref": "release/26.05",
                                },
                            },
                            "lib": {
                                "locked": {
                                    "type": "github",
                                    "owner": "example",
                                    "repo": "transitive-lib",
                                    "rev": "e" * 40,
                                },
                                "original": {
                                    "type": "github",
                                    "owner": "example",
                                    "repo": "transitive-lib",
                                    "ref": "main",
                                },
                            },
                        },
                    }
                )
                + "\n",
                encoding="utf-8",
            )
            edges = discover_edges(self.repository(), root)
            by_kind = {edge.kind: edge for edge in edges}
            self.assertEqual(by_kind["git-submodule"].target_repo, "example/lib")
            self.assertEqual(by_kind["git-submodule"].tracked_branch, "main")
            self.assertEqual(by_kind["zed-package"].current_version, "0.2.4")
            self.assertEqual(by_kind["zed-package"].target_repo, "example/interfaces")
            self.assertEqual(by_kind["nix-flake"].target_repo, "NixOS/nixpkgs")
            self.assertEqual(by_kind["nix-flake"].input_name, "nixpkgs")
            self.assertEqual(by_kind["nix-flake"].tracked_branch, "release/26.05")
            self.assertEqual(by_kind["nix-flake"].input_name, "nixpkgs")
            self.assertTrue(by_kind["nix-flake"].metadata["directInput"])
            self.assertEqual(
                by_kind["nix-flake-transitive"].target_repo,
                "example/transitive-lib",
            )
            self.assertTrue(by_kind["nix-flake-transitive"].metadata["graphOnly"])

    def test_relative_submodule_url_resolves_inside_source_org(self) -> None:
        self.assertEqual(
            normalize_github_repo_url("../interfaces.git", "example"),
            "example/interfaces",
        )

    def test_worker_repository_url_normalizes_case_for_prefix_policy(self) -> None:
        repository = Repository(
            full_name="3FA-app/ThreeFA-Interfaces",
            default_branch="main",
            clone_url="https://github.com/3FA-app/ThreeFA-Interfaces.git",
        )
        self.assertEqual(
            worker_repository_url(repository),
            "https://github.com/3fa-app/threefa-interfaces.git",
        )

    def test_replaces_only_scalar_zpkg_dependency_and_preserves_operator(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / ".zpkg.toml"
            path.write_text(
                '[dependencies]\n"example/interfaces" = "^0.2.4" # keep\n',
                encoding="utf-8",
            )
            replace_zpkg_version(path, "example/interfaces", SemVer(0, 3, 9))
            self.assertIn('"example/interfaces" = "^0.3.9" # keep', path.read_text())

    def test_replaces_inline_structured_zpkg_git_pin_only(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / ".zpkg.toml"
            path.write_text(
                '[dependencies]\n"example/interfaces" = { git = "https://github.com/example/interfaces.git", branch = "main", rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" } # keep\n',
                encoding="utf-8",
            )
            replacement = "b" * 40
            replace_zpkg_git_pin(path, "example/interfaces", replacement)
            content = path.read_text(encoding="utf-8")
            self.assertIn(f'rev = "{replacement}"', content)
            self.assertIn('branch = "main"', content)
            self.assertTrue(content.rstrip().endswith("# keep"))

    def test_replaces_table_structured_zpkg_git_pin_only(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / ".zpkg.toml"
            path.write_text(
                '[dependencies."example/interfaces"]\ngit = "https://github.com/example/interfaces.git"\nbranch = "release/1.x"\nsha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" # keep\n\n[package]\nname = "app"\n',
                encoding="utf-8",
            )
            replacement = "c" * 40
            replace_zpkg_git_pin(path, "example/interfaces", replacement)
            content = path.read_text(encoding="utf-8")
            self.assertIn(f'sha = "{replacement}" # keep', content)
            self.assertIn('[package]\nname = "app"', content)

    def test_enriches_branch_pinned_zpkg_dependency_from_lock(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.init_repo(root)
            (root / ".zpkg.toml").write_text(
                '[dependencies]\n"example/interfaces" = { git = "https://github.com/example/interfaces.git", branch = "main" }\n',
                encoding="utf-8",
            )
            (root / ".zpkg.lock").write_text(
                json.dumps(
                    {
                        "dependencies": [
                            {
                                "name": "example/interfaces",
                                "git": "https://github.com/example/interfaces.git",
                                "rev": "d" * 40,
                            }
                        ]
                    }
                )
                + "\n",
                encoding="utf-8",
            )
            edges = discover_edges(self.repository(), root)
            direct = next(edge for edge in edges if edge.kind == "zed-package")
            self.assertEqual(direct.current_sha, "d" * 40)
            self.assertEqual(direct.tracked_branch, "main")
            self.assertFalse(any(edge.kind == "zed-package-lock-only" for edge in edges))

    def test_detects_fixed_profiles_without_running_repository_commands(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text("[package]\nname='x'\nversion='0.1.0'\n")
            self.assertEqual(detect_profile(root), "rust-verify")
            (root / "flake.nix").write_text("{}\n")
            self.assertEqual(detect_profile(root), "nix-verify")


class ManagedPullRequestTests(unittest.TestCase):
    def test_only_matching_marker_and_bot_branch_are_managed(self) -> None:
        edge = DependencyEdge(
            source_repo="example/app",
            source_path=".zpkg.toml",
            kind="zed-package",
            dependency_key="example/lib",
            target_repo="example/lib",
            target_url=None,
            current_version="1.2.3",
        )
        body = managed_marker(edge) + "\nmanaged body"
        self.assertTrue(pr_is_managed(body, "bot/portfolio-deps/lib-123", edge, "bot/portfolio-deps/"))
        self.assertFalse(pr_is_managed(body, "human/dependency", edge, "bot/portfolio-deps/"))
        other = DependencyEdge(
            source_repo="example/app",
            source_path=".zpkg.toml",
            kind="zed-package",
            dependency_key="example/other",
            target_repo="example/other",
            target_url=None,
        )
        self.assertFalse(pr_is_managed(body, "bot/portfolio-deps/lib-123", other, "bot/portfolio-deps/"))


if __name__ == "__main__":
    unittest.main()
