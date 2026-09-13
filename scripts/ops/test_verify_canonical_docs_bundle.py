#!/usr/bin/env python3
"""Fail-closed tests for the immutable Canonical Docs source verifier."""

from __future__ import annotations

import importlib.util
import shutil
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("verify_canonical_docs_bundle.py")
SPEC = importlib.util.spec_from_file_location("verify_canonical_docs_bundle", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

SOURCE_ASSETS = Path(__file__).resolve().parents[1] / "critical-org-fleet" / "assets"


class CanonicalDocsBundleTests(unittest.TestCase):
    def copy_assets(self, destination: Path) -> Path:
        target = destination / "assets"
        shutil.copytree(SOURCE_ASSETS, target)
        return target

    def test_reviewed_bundle_passes_exact_contract(self) -> None:
        report = MODULE.verify(SOURCE_ASSETS)
        self.assertEqual(report["bundle_sha256"], MODULE.BUNDLE_SHA256)
        self.assertEqual(report["heads"], MODULE.EXPECTED_HEADS)
        self.assertEqual(report["main_tree"], MODULE.MAIN_TREE_SHA)
        self.assertEqual(report["feature_tree"], MODULE.FEATURE_TREE_SHA)
        self.assertEqual(report["feature_parent"], MODULE.MAIN_SHA)
        self.assertEqual(report["business_plan_sha256"], MODULE.BUSINESS_PLAN_SHA256)
        self.assertEqual(report["asset_count"], 4)
        self.assertTrue(report["documentation_contract"].startswith("documentation contract: PASS"))

    def test_missing_and_unexpected_assets_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="canonical-docs-assets-") as temporary:
            assets = self.copy_assets(Path(temporary))
            (assets / MODULE.EXPECTED_ASSETS[0]).unlink()
            with self.assertRaisesRegex(MODULE.VerificationError, "asset inventory mismatch"):
                MODULE.read_assets(assets)

        with tempfile.TemporaryDirectory(prefix="canonical-docs-assets-") as temporary:
            assets = self.copy_assets(Path(temporary))
            (assets / "canonical-docs.part999").write_text("QQ==\n", encoding="ascii")
            with self.assertRaisesRegex(MODULE.VerificationError, "asset inventory mismatch"):
                MODULE.read_assets(assets)

    def test_non_regular_asset_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="canonical-docs-assets-") as temporary:
            assets = self.copy_assets(Path(temporary))
            victim = assets / MODULE.EXPECTED_ASSETS[0]
            content = victim.read_text(encoding="ascii")
            victim.unlink()
            target = assets / "outside"
            target.write_text(content, encoding="ascii")
            victim.symlink_to(target)
            with self.assertRaisesRegex(MODULE.VerificationError, "not a regular file"):
                MODULE.read_assets(assets)

    def test_invalid_base64_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="canonical-docs-assets-") as temporary:
            assets = self.copy_assets(Path(temporary))
            first = assets / MODULE.EXPECTED_ASSETS[0]
            text = first.read_text(encoding="ascii")
            first.write_text("!" + text[1:], encoding="ascii")
            with self.assertRaisesRegex(MODULE.VerificationError, "not canonical base64"):
                MODULE.read_assets(assets)

    def test_valid_base64_with_changed_bytes_fails_digest(self) -> None:
        with tempfile.TemporaryDirectory(prefix="canonical-docs-assets-") as temporary:
            assets = self.copy_assets(Path(temporary))
            first = assets / MODULE.EXPECTED_ASSETS[0]
            text = first.read_text(encoding="ascii")
            replacement = "A" if text[0] != "A" else "B"
            first.write_text(replacement + text[1:], encoding="ascii")
            with self.assertRaisesRegex(MODULE.VerificationError, "bundle digest mismatch"):
                MODULE.read_assets(assets)

    def test_head_parser_rejects_duplicates_and_malformed_rows(self) -> None:
        with self.assertRaisesRegex(MODULE.VerificationError, "duplicate bundle head"):
            MODULE.parse_heads(
                f"{MODULE.MAIN_SHA} refs/heads/main\n"
                f"{MODULE.MAIN_SHA} refs/heads/main\n"
            )
        with self.assertRaisesRegex(MODULE.VerificationError, "malformed bundle head"):
            MODULE.parse_heads("not-a-sha refs/heads/main\n")


if __name__ == "__main__":
    unittest.main()
