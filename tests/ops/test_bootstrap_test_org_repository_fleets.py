from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "ops" / "bootstrap_test_org_repository_fleets.py"
CONFIG = ROOT / "config" / "test_org_repository_fleets" / "index.json"

spec = importlib.util.spec_from_file_location("test_fleet_publisher", SCRIPT)
assert spec and spec.loader
fleet = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fleet
spec.loader.exec_module(fleet)


class ConfigTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.payload = fleet.load_config(CONFIG)

    def test_fixed_org_and_repository_counts(self) -> None:
        self.assertEqual(set(self.payload["fleets"]), fleet.EXPECTED_TEST_ORGS)
        self.assertEqual(len(self.payload["fleets"]), 18)
        self.assertEqual(sum(len(v["repositories"]) for v in self.payload["fleets"].values()), 209)
        self.assertEqual(209 + len(self.payload["fleets"]), 227)

    def test_r2g_is_explicitly_excluded_and_never_referenced(self) -> None:
        excluded = {x.lower() for x in self.payload["excluded_organizations"]}
        self.assertTrue({"r2g", "r2g-test"}.issubset(excluded))
        serialized = json.dumps(self.payload["fleets"]).lower()
        self.assertNotIn('"r2g/', serialized)
        self.assertNotIn('"r2g-test/', serialized)

    def test_expected_per_org_counts(self) -> None:
        expected = {
            "3fa-app-test": 11,
            "claritas-viz-test": 9,
            "cliptown-test": 12,
            "declarative-migrations-test": 9,
            "embedded-alerts-test": 9,
            "evento-globolo-test": 11,
            "fiducia-cloud-test": 19,
            "file-tunnel-test": 10,
            "hypesiege-test": 11,
            "memebank-test": 11,
            "messaging-intel-test": 12,
            "opto-sync-test": 10,
            "quaestor-ledger-test": 12,
            "scintilla-run-test": 12,
            "shared-auth-test": 12,
            "sonus-auris-test": 13,
            "streempilot-test": 13,
            "zed-pkg-test": 13,
        }
        actual = {org: len(value["repositories"]) for org, value in self.payload["fleets"].items()}
        self.assertEqual(actual, expected)

    def test_sdk_consumers_declare_native_zed_and_git_modes(self) -> None:
        consumers = []
        for org, value in self.payload["fleets"].items():
            for repo in value["repositories"]:
                if repo["kind"] in {"client", "consumer-fixture"}:
                    consumers.append((org, repo))
        self.assertGreaterEqual(len(consumers), 45)
        for org, repo in consumers:
            managers = set(repo["package_managers"])
            self.assertIn("zed", managers, f"{org}/{repo['name']}")
            self.assertIn("git-submodule", managers, f"{org}/{repo['name']}")
            self.assertGreaterEqual(len(managers - {"zed", "git-submodule"}), 1, f"{org}/{repo['name']}")

    def test_database_and_device_requirements_are_present(self) -> None:
        migrations = self.payload["fleets"]["declarative-migrations-test"]["repositories"]
        names = {x["name"] for x in migrations}
        self.assertIn("postgres-forward-rollback", names)
        self.assertIn("cockroach-forward-rollback", names)
        threefa = {x["name"] for x in self.payload["fleets"]["3fa-app-test"]["repositories"]}
        self.assertTrue({"threefa-android-emulator", "threefa-ios-simulator", "threefa-desktop-linux", "threefa-desktop-macos", "threefa-desktop-windows"}.issubset(threefa))

    def test_fiducia_has_ten_native_language_consumers(self) -> None:
        repos = self.payload["fleets"]["fiducia-cloud-test"]["repositories"]
        clients = [x for x in repos if x["name"].startswith("fiducia-client-")]
        self.assertEqual(len(clients), 10)
        self.assertEqual(
            {x["languages"][0] for x in clients},
            {"rust", "typescript", "python", "go", "java", "kotlin", "csharp", "dart", "swift", "php"},
        )

    def test_existing_zed_baseline_is_preserved(self) -> None:
        baseline = set(self.payload["existing_baselines"]["zed-pkg-test"])
        self.assertEqual(len(baseline), 22)
        additions = {x["name"] for x in self.payload["fleets"]["zed-pkg-test"]["repositories"]}
        self.assertTrue(baseline.isdisjoint(additions))
        self.assertIn("submodule-consumer", baseline)
        self.assertIn("zed-api-contract", additions)


class RenderTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.payload = fleet.load_config(CONFIG)

    def sample(self):
        value = self.payload["fleets"]["fiducia-cloud-test"]
        repo = next(x for x in value["repositories"] if x["name"] == "fiducia-client-rust")
        pins = [
            fleet.SourcePin(
                repository="fiducia-cloud/fiducia-clients",
                alias="clients",
                role="clients",
                required=True,
                zed_dependency=True,
                exists=True,
                default_branch="main",
                commit_sha="1" * 40,
                private=True,
                html_url="https://github.com/fiducia-cloud/fiducia-clients",
            ),
            fleet.SourcePin(
                repository="fiducia-cloud/fiducia-interfaces",
                alias="interfaces",
                role="interfaces",
                required=True,
                zed_dependency=True,
                exists=False,
                error="planned fixture",
            ),
        ]
        return repo, pins

    def test_rendered_repo_has_source_pins_zed_submodule_and_workflows(self) -> None:
        repo, pins = self.sample()
        files, digest = fleet.render_repo_files("fiducia-cloud-test", "fiducia-cloud", repo, pins)
        self.assertEqual(len(digest), 64)
        self.assertIn(".gitmodules", files)
        self.assertIn("vendor/clients", files[".gitmodules"][0])
        zpkg = tomllib.loads(files[".zpkg.toml"][0])
        self.assertEqual(zpkg["install"]["dir"], ".vendor/.zed")
        self.assertEqual(zpkg["dependencies"]["fiducia-cloud/fiducia-clients"], "*")
        self.assertIn(".github/workflows/ci.yml", files)
        self.assertIn(".github/workflows/integration.yml", files)
        self.assertIn("source-pins.json", files)
        self.assertIn("test-plan.md", files)

    def test_rendered_contract_executes(self) -> None:
        repo, pins = self.sample()
        files, _ = fleet.render_repo_files("fiducia-cloud-test", "fiducia-cloud", repo, pins)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for relative, (content, mode) in files.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
                if mode == "100755":
                    path.chmod(0o755)
            subprocess.run([sys.executable, str(root / "scripts" / "verify_fleet.py")], cwd=root, check=True)
            subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"], cwd=root, check=True)

    def test_source_pin_changes_contract_digest(self) -> None:
        repo, pins = self.sample()
        _, first = fleet.render_repo_files("fiducia-cloud-test", "fiducia-cloud", repo, pins)
        changed = [pins[0].__class__(**{**pins[0].as_json(), "commit_sha": "2" * 40}), pins[1]]
        _, second = fleet.render_repo_files("fiducia-cloud-test", "fiducia-cloud", repo, changed)
        self.assertNotEqual(first, second)

    def test_dotgithub_catalog_matches_fleet(self) -> None:
        value = self.payload["fleets"]["cliptown-test"]
        files, digest = fleet.render_org_dotgithub_files("cliptown-test", "cliptown", value["repositories"])
        self.assertEqual(len(digest), 64)
        catalog = json.loads(files["test-fleet-catalog.json"][0])
        self.assertEqual(catalog["repository_count"], 12)
        self.assertEqual({x["name"] for x in catalog["repositories"]}, {x["name"] for x in value["repositories"]})
        self.assertIn("profile/README.md", files)
        self.assertIn(".github/workflows/reusable-test-fleet-ci.yml", files)


class TreeRequestTests(unittest.TestCase):
    class FakeClient:
        def __init__(self, reject_gitlink: bool = False):
            self.calls = []
            self.reject_gitlink = reject_gitlink
            self.rejected = False

        def get(self, path, **kwargs):
            self.calls.append(("GET", path, None))
            return {"tree": {"sha": "base-tree"}}

        def post(self, path, body, **kwargs):
            self.calls.append(("POST", path, body))
            if path.endswith("/git/trees"):
                if self.reject_gitlink and not self.rejected and any(x.get("mode") == "160000" for x in body["tree"]):
                    self.rejected = True
                    raise fleet.ApiError(422, "POST", path, {"message": "invalid gitlink"})
                return {"sha": "tree-sha"}
            if path.endswith("/git/commits"):
                return {"sha": "commit-sha"}
            raise AssertionError(path)

    def pin(self):
        return fleet.SourcePin("owner/source", "source", "clients", True, True, True, "main", "a" * 40, False, "https://github.com/owner/source")

    def test_files_use_one_inline_tree_request_not_per_file_blobs(self) -> None:
        client = self.FakeClient()
        commit, warnings = fleet.create_commit_tree(
            client,
            "org/repo",
            "b" * 40,
            {"README.md": ("hello", "100644"), "script.sh": ("#!/bin/sh\n", "100755")},
            [],
            "test commit",
        )
        self.assertEqual(commit, "commit-sha")
        self.assertEqual(warnings, [])
        paths = [path for method, path, _ in client.calls if method == "POST"]
        self.assertFalse(any(path.endswith("/git/blobs") for path in paths))
        tree_body = next(body for method, path, body in client.calls if path.endswith("/git/trees"))
        self.assertTrue(all("content" in item for item in tree_body["tree"]))

    def test_gitlink_rejection_falls_back_to_immutable_pin_files(self) -> None:
        client = self.FakeClient(reject_gitlink=True)
        _, warnings = fleet.create_commit_tree(
            client,
            "org/repo",
            "b" * 40,
            {"README.md": ("hello", "100644"), ".gitmodules": ("[submodule \"source\"]\n", "100644")},
            [self.pin()],
            "test commit",
        )
        self.assertEqual(len(warnings), 1)
        tree_calls = [body for method, path, body in client.calls if path.endswith("/git/trees")]
        self.assertEqual(len(tree_calls), 2)
        self.assertTrue(any(x.get("mode") == "160000" for x in tree_calls[0]["tree"]))
        self.assertFalse(any(x.get("mode") == "160000" for x in tree_calls[1]["tree"]))


class ExactHeadGateTests(unittest.TestCase):
    class Client:
        def __init__(self, statuses, checks):
            self.statuses = statuses
            self.checks = checks

        def get(self, path, **kwargs):
            if path.endswith("/status"):
                return {"statuses": self.statuses}
            if "/check-runs?" in path:
                return {"check_runs": self.checks}
            raise AssertionError(path)

    def test_success_requires_at_least_one_exact_head_gate(self) -> None:
        green, detail = fleet.commit_gate(self.Client([], []), "org/repo", "1" * 40)
        self.assertFalse(green)
        self.assertIn("no commit status", detail)
        green, detail = fleet.commit_gate(
            self.Client([{"context": "test-fleet/bootstrap-validation", "state": "success"}], []),
            "org/repo",
            "1" * 40,
        )
        self.assertTrue(green)
        self.assertIn("succeeded", detail)

    def test_pending_or_failed_observed_check_blocks_merge(self) -> None:
        client = self.Client(
            [{"context": "test-fleet/bootstrap-validation", "state": "success"}],
            [{"name": "CI", "status": "in_progress", "conclusion": None}],
        )
        green, detail = fleet.commit_gate(client, "org/repo", "2" * 40)
        self.assertFalse(green)
        self.assertIn("CI=in_progress/None", detail)

    def test_all_observed_gates_must_be_success(self) -> None:
        client = self.Client(
            [
                {"context": "test-fleet/bootstrap-validation", "state": "success"},
                {"context": "security", "state": "failure"},
            ],
            [{"name": "CI", "status": "completed", "conclusion": "success"}],
        )
        green, detail = fleet.commit_gate(client, "org/repo", "3" * 40)
        self.assertFalse(green)
        self.assertIn("security=failure", detail)


if __name__ == "__main__":
    unittest.main()
