from __future__ import annotations

import importlib.util
import os
import sys
import unittest
from pathlib import Path
from unittest import mock

os.environ.setdefault("GH_TOKEN", "unit-test-token")
MODULE_PATH = Path(__file__).resolve().parents[1] / "bootstrap_missing_canonical_repositories.py"
SPEC = importlib.util.spec_from_file_location("gap_bootstrap", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {MODULE_PATH}")
bootstrap = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = bootstrap
SPEC.loader.exec_module(bootstrap)


class GapBootstrapContractTests(unittest.TestCase):
    def test_missing_product_repository_is_created_public(self) -> None:
        calls: list[tuple[str, str, object | None]] = []

        def fake_api(method: str, path: str, body: object | None = None):
            calls.append((method, path, body))
            if method == "GET":
                return 404, None
            self.assertEqual(method, "POST")
            assert isinstance(body, dict)
            return 201, {
                "full_name": "hypesiege/example",
                "visibility": "public",
                "private": False,
            }

        repository = bootstrap.ensure_repository(
            "hypesiege", "example", "example", "public", api=fake_api
        )
        self.assertEqual(repository["visibility"], "public")
        create = next(call for call in calls if call[0] == "POST")
        payload = create[2]
        assert isinstance(payload, dict)
        self.assertIs(payload["private"], False)
        self.assertEqual(payload["visibility"], "public")
        self.assertIs(payload["auto_init"], False)

    def test_existing_private_product_repository_is_reconciled_without_git_mutation(self) -> None:
        calls: list[tuple[str, str, object | None]] = []

        def fake_api(method: str, path: str, body: object | None = None):
            calls.append((method, path, body))
            if method == "GET":
                return 200, {
                    "full_name": "StreemPilot/example",
                    "visibility": "private",
                    "private": True,
                }
            self.assertEqual(method, "PATCH")
            self.assertEqual(path, "/repos/StreemPilot/example")
            self.assertEqual(body, {"private": False, "visibility": "public"})
            return 200, {
                "full_name": "StreemPilot/example",
                "visibility": "public",
                "private": False,
            }

        repository = bootstrap.ensure_repository(
            "StreemPilot", "example", "example", "public", api=fake_api
        )
        self.assertEqual(repository["visibility"], "public")
        self.assertEqual([call[0] for call in calls], ["GET", "PATCH"])

    def test_existing_public_repository_requires_no_metadata_write(self) -> None:
        calls: list[tuple[str, str, object | None]] = []

        def fake_api(method: str, path: str, body: object | None = None):
            calls.append((method, path, body))
            return 200, {
                "full_name": "hypesiege/example",
                "visibility": "public",
                "private": False,
            }

        bootstrap.ensure_repository(
            "hypesiege", "example", "example", "public", api=fake_api
        )
        self.assertEqual([call[0] for call in calls], ["GET"])

    def test_meta_agents_is_not_initialized_by_gap_bootstrap(self) -> None:
        def fake_api(method: str, path: str, body: object | None = None):
            self.assertEqual(method, "GET")
            self.assertEqual(path, f"/repos/{bootstrap.META_AGENT}")
            return 404, None

        with (
            mock.patch.object(bootstrap.CORE, "api", side_effect=fake_api),
            mock.patch.object(
                bootstrap,
                "ensure_repository",
                return_value={"visibility": "private", "private": True},
            ) as ensure,
            mock.patch.object(bootstrap.CORE, "main_ref", return_value="a" * 40),
            mock.patch.object(bootstrap.CORE, "publish_file_tunnel_mcp") as publish,
        ):
            result = bootstrap.bootstrap_extracted(Path("/tmp/not-used"))

        self.assertEqual(
            result[bootstrap.META_AGENT], "managed_by_exact_public_publisher"
        )
        ensure.assert_called_once()
        args = ensure.call_args.args
        self.assertEqual(args[0:2], ("file-tunnel", "ftnl-mcp-server.rs"))
        self.assertEqual(args[3], "private")
        publish.assert_not_called()

    def test_fleet_source_archive_is_commit_pinned(self) -> None:
        self.assertEqual(len(bootstrap.FLEET_SOURCE_SHA), 40)
        self.assertIn(
            f"/archive/{bootstrap.FLEET_SOURCE_SHA}.tar.gz",
            bootstrap.FLEET_SOURCE_ARCHIVE_URL,
        )
        source = MODULE_PATH.read_text(encoding="utf-8")
        self.assertNotIn('"git",\n            "clone"', source)
        self.assertNotIn('"git", "fetch"', source)
        self.assertIn("FLEET_SOURCE_ARCHIVE_URL", source)

    def test_source_forbids_force_push_and_orders_children_before_monorepos(self) -> None:
        source = MODULE_PATH.read_text(encoding="utf-8")
        self.assertNotIn("--force", source)
        self.assertIn(
            "key=lambda item: (item.get(\"kind\") == \"monorepo\"",
            source,
        )
        self.assertIn("PRESERVED_DIVERGENT", source)
        self.assertIn("visibility_reconciled_to_public", source)


if __name__ == "__main__":
    unittest.main()
