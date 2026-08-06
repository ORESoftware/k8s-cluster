from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
MODULE_PATH = ROOT / "scripts/ops/publish_requested_mcp_servers.py"
sys.path.insert(0, str(MODULE_PATH.parent))
import requested_mcp_publisher as MODULE


class ManifestTests(unittest.TestCase):
    def test_exact_allowlist_and_visibility(self) -> None:
        specs = MODULE.validate_specs()
        self.assertEqual(
            [(spec.full_name, spec.visibility) for spec in specs],
            [
                ("cliptown/cliptown-mcp-server.rs", "public"),
                ("opto-sync/opto-sync-mcp-server.rs", "public"),
                ("voxletra/vxl-mcp-server.rs", "private"),
                ("zed-pkg/zed-mcp-server.rs", "public"),
                ("zed-pkg-test/zed-pkg-test-mcp-server.rs", "public"),
            ],
        )

    def test_allowlist_rejects_count_and_identity_drift(self) -> None:
        with self.assertRaises(MODULE.PublisherError):
            MODULE.validate_specs(MODULE.REPOSITORIES[:-1])
        replacement = MODULE.RepositorySpec(
            owner="cliptown",
            name="wrong-name.rs",
            description="wrong",
            private=False,
            product="wrong",
        )
        with self.assertRaises(MODULE.PublisherError):
            MODULE.validate_specs((replacement, *MODULE.REPOSITORIES[1:]))

    def test_bootstrap_contract_is_safe_and_product_specific(self) -> None:
        for spec in MODULE.validate_specs():
            files = MODULE.bootstrap_files(spec)
            self.assertEqual(set(files), {".gitignore", "LICENSE", "README.md", "SECURITY.md"})
            joined = "\n".join(files.values())
            self.assertIn(spec.full_name, joined)
            self.assertIn(spec.product, joined)
            self.assertIn("read-only", joined.lower())
            self.assertNotRegex(joined, r"ghp_[A-Za-z0-9]+")
            self.assertNotIn("Authorization: Bearer", joined)
            self.assertNotIn("GITHUB_TOKEN", joined)

    def test_bootstrap_commit_is_deterministic(self) -> None:
        spec = MODULE.REPOSITORIES[0]
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            first_sha = MODULE.materialize_bootstrap(spec, Path(first) / "repo")
            second_sha = MODULE.materialize_bootstrap(spec, Path(second) / "repo")
        self.assertRegex(first_sha, r"^[0-9a-f]{40}$")
        self.assertEqual(first_sha, second_sha)

    def test_check_mode_reconstructs_all_five_roots(self) -> None:
        report = MODULE.check(MODULE.validate_specs())
        self.assertEqual(report["schema_version"], 1)
        self.assertEqual(len(report["repositories"]), 5)
        self.assertEqual(len({row["bootstrap_sha"] for row in report["repositories"]}), 5)
        for row in report["repositories"]:
            self.assertRegex(row["bootstrap_sha"], r"^[0-9a-f]{40}$")
            self.assertEqual(row["files"], [".gitignore", "LICENSE", "README.md", "SECURITY.md"])


class SourceSafetyTests(unittest.TestCase):
    @staticmethod
    def publisher_source() -> str:
        return "\n".join(
            path.read_text(encoding="utf-8")
            for path in sorted((MODULE_PATH.parent / "requested_mcp_publisher").glob("*.py"))
        )

    def test_source_has_no_force_push_or_token_bearing_remote(self) -> None:
        source = self.publisher_source()
        self.assertNotIn("--force", source)
        self.assertNotRegex(source, r"https://[^\s\"']*@github\.com")
        self.assertIn("GIT_ASKPASS_REQUIRE", source)
        self.assertIn("GIT_TERMINAL_PROMPT", source)
        self.assertIn("bootstrap_is_ancestor", source)

    def test_api_paths_are_bounded_and_redirect_free_by_construction(self) -> None:
        source = self.publisher_source()
        self.assertIn("MAX_API_RESPONSE_BYTES", source)
        self.assertIn("timeout=30", source)
        self.assertIn("X-GitHub-Api-Version", source)
        self.assertIn("_NoRedirect", source)

    def test_script_compiles_and_check_mode_runs(self) -> None:
        subprocess.run([sys.executable, "-m", "py_compile", str(MODULE_PATH)], check=True)
        completed = subprocess.run(
            [sys.executable, str(MODULE_PATH), "--check"],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        payload = json.loads(completed.stdout)
        self.assertEqual(len(payload["repositories"]), 5)


if __name__ == "__main__":
    unittest.main(verbosity=2)
