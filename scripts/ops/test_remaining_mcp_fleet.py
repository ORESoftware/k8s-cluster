#!/usr/bin/env python3
from __future__ import annotations

import copy
import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent
MODULE_PATH = ROOT / "remaining_mcp_fleet.py"
SPEC = importlib.util.spec_from_file_location("remaining_mcp_fleet", MODULE_PATH)
assert SPEC and SPEC.loader
fleet = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = fleet
SPEC.loader.exec_module(fleet)


class RemainingMcpFleetTests(unittest.TestCase):
    def test_exact_server_and_monorepo_allowlists(self) -> None:
        self.assertEqual(
            [spec.full_name for spec in fleet.SERVER_SPECS],
            [
                "cliptown/cliptown-mcp-server.rs",
                "opto-sync/opto-sync-mcp-server.rs",
                "voxletra/vxl-mcp-server.rs",
                "zed-pkg/zed-mcp-server.rs",
                "zed-pkg-test/zed-pkg-test-mcp-server.rs",
            ],
        )
        self.assertEqual(
            [spec.full_name for spec in fleet.MONOREPO_SPECS],
            [
                "cliptown/cliptown-monorepo",
                "opto-sync/opto-sync-monorepo",
                "voxletra/vxl-monorepo",
                "zed-pkg/zed-monorepo",
                "zed-pkg-test/zed-pkg-test-monorepo",
            ],
        )

    def test_visibility_follows_reviewed_sibling_repository_policy(self) -> None:
        visibility = {spec.full_name: spec.visibility for spec in fleet.SERVER_SPECS}
        self.assertEqual(visibility["voxletra/vxl-mcp-server.rs"], "private")
        self.assertTrue(
            all(value == "public" for key, value in visibility.items() if not key.startswith("voxletra/"))
        )
        monorepo = {spec.full_name: spec.visibility for spec in fleet.MONOREPO_SPECS}
        self.assertEqual(monorepo["voxletra/vxl-monorepo"], "private")

    def test_generated_servers_use_exact_sdk_and_immutable_shared_revision(self) -> None:
        for spec in fleet.SERVER_SPECS:
            files = fleet.render_server_files(spec)
            manifest = files["Cargo.toml"]
            self.assertIn('rmcp = { version = "=3.1.0"', manifest)
            self.assertEqual(manifest.count(f'rev = "{fleet.SHARED_REVISION}"'), 2)
            self.assertIn('rust-version = "1.88.0"', manifest)
            self.assertIn("Cargo.lock", files[".github/workflows/ci.yml"])
            self.assertNotIn("Cargo.lock", files[".gitignore"])
            self.assertIn("MIT License", files["LICENSE"])
            self.assertIn("GitHub Security Advisories", files["SECURITY.md"])

    def test_generated_tool_surface_is_closed_read_only_and_offline(self) -> None:
        for spec in fleet.SERVER_SPECS:
            files = fleet.render_server_files(spec)
            server = files["src/server.rs"]
            self.assertIn("ToolAnnotations::new()", server)
            self.assertIn(".read_only(true)", server)
            self.assertIn(".destructive(false)", server)
            self.assertIn(".idempotent(true)", server)
            self.assertIn(".open_world(false)", server)
            self.assertIn("#[serde(deny_unknown_fields)]", server)
            self.assertIn("ore_mcp_safety::Bounds::new", server)
            self.assertEqual(server.count("#[tool(description"), 5)
            for forbidden in ["reqwest", "tokio::process", "std::fs::", "Command::new"]:
                self.assertNotIn(forbidden, server)
            safety = files["src/domain.rs"]
            self.assertIn('"credentials_accepted": false', safety)
            self.assertIn('"filesystem_writes": false', safety)
            self.assertIn('"network_access": false', safety)

    def test_real_binary_matrix_covers_ids_notifications_errors_recovery_and_eof(self) -> None:
        for spec in fleet.SERVER_SPECS:
            test = fleet.render_server_files(spec)["tests/stdio_protocol.rs"]
            for snippet in [
                '"id":"init-string"',
                '"id":2',
                "notifications/initialized",
                "RecvTimeoutError::Timeout",
                spec.forbidden_argument[0],
                '"id":5',
                "server did not exit after EOF",
                "audit_stdio_stdout",
                'annotations"]["readOnlyHint',
            ]:
                self.assertIn(snippet, test)

    def test_domain_specific_privacy_and_mutation_denials_are_present(self) -> None:
        rendered = {spec.full_name: fleet.render_server_files(spec) for spec in fleet.SERVER_SPECS}
        self.assertIn("content_received", rendered["cliptown/cliptown-mcp-server.rs"]["src/server.rs"])
        self.assertIn("database_url", rendered["opto-sync/opto-sync-mcp-server.rs"]["tests/stdio_protocol.rs"])
        self.assertIn("transcript_received", rendered["voxletra/vxl-mcp-server.rs"]["src/server.rs"])
        self.assertIn("plan_only must be true", rendered["zed-pkg/zed-mcp-server.rs"]["src/server.rs"])
        self.assertIn("live_registry must be false", rendered["zed-pkg-test/zed-pkg-test-mcp-server.rs"]["src/server.rs"])

    def test_request_manifest_is_exact_and_fails_on_drift(self) -> None:
        manifest = fleet.request_manifest()
        fleet.validate_request_manifest(manifest)
        for mutation in (
            lambda value: value.__setitem__("execute", False),
            lambda value: value["servers"].pop(),
            lambda value: value["monorepos"][2].__setitem__("visibility", "public"),
            lambda value: value.__setitem__("template_digest", "0" * 64),
        ):
            candidate = copy.deepcopy(manifest)
            mutation(candidate)
            with self.assertRaises(ValueError):
                fleet.validate_request_manifest(candidate)

    def test_generated_tree_is_deterministic(self) -> None:
        for spec in fleet.SERVER_SPECS:
            first = fleet.render_server_files(spec)
            second = fleet.render_server_files(spec)
            self.assertEqual(first, second)
            with tempfile.TemporaryDirectory() as directory:
                fleet.write_server_tree(spec, Path(directory))
                self.assertEqual(
                    sorted(
                        path.relative_to(directory).as_posix()
                        for path in Path(directory).rglob("*")
                        if path.is_file()
                    ),
                    sorted(first),
                )

    def test_publisher_is_pr_only_and_ci_gated(self) -> None:
        publisher = (ROOT / "publish_remaining_mcp_fleet.py").read_text(encoding="utf-8")
        self.assertIn("wait_for_workflow", publisher)
        self.assertIn("merge_pull_request", publisher)
        self.assertLess(
            publisher.index("wait_for_workflow(spec.full_name"),
            publisher.index("merge_pull_request(spec.full_name"),
        )
        self.assertIn('"merge_method": "squash"', publisher)
        self.assertNotIn("--force", publisher)
        self.assertNotIn("HEAD:refs/heads/main", publisher)
        self.assertIn("auto_init", publisher)
        self.assertIn("GIT_ASKPASS", publisher)
        self.assertNotIn("Authorization: Bearer", publisher)

    def test_monorepo_publication_uses_exact_mode_160000_contracts(self) -> None:
        publisher = (ROOT / "publish_remaining_mcp_fleet.py").read_text(encoding="utf-8")
        self.assertIn("gitlink", publisher)
        self.assertIn("parts[0] != '160000'", publisher)
        self.assertIn("MCP submodule contract", publisher)
        self.assertIn("submodule", publisher)
        self.assertIn(
            'wait_for_workflow(spec.full_name, head, "MCP submodule contract")', publisher
        )


if __name__ == "__main__":
    unittest.main()
