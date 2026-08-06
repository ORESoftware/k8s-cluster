#!/usr/bin/env python3
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import harden_org_dotgithub_fleet as target


class RegistryTests(unittest.TestCase):
    def test_load_registry_accepts_canonical_header(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "registry.tsv"
            path.write_text(
                "organization\tlinear_url\n"
                "alpha\thttps://linear.app/example/project/alpha\n",
                encoding="utf-8",
            )
            self.assertEqual(
                [("alpha", "https://linear.app/example/project/alpha")],
                target.load_registry(str(path), 1),
            )

    def test_load_registry_rejects_duplicates(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "registry.tsv"
            path.write_text(
                "alpha\thttps://linear.app/example/project/alpha\n"
                "Alpha\thttps://linear.app/example/project/alpha-2\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(target.HardeningError, "duplicate"):
                target.load_registry(str(path), 2)

    def test_load_registry_requires_exact_count(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "registry.tsv"
            path.write_text("alpha\thttps://linear.app/example/project/alpha\n", encoding="utf-8")
            with self.assertRaisesRegex(target.HardeningError, "expected 2"):
                target.load_registry(str(path), 2)

    def test_static_baseline_is_safe_and_complete(self) -> None:
        target.validate_static()
        docs = target.documents("alpha")
        self.assertEqual(15, len(docs))
        self.assertIn("AGENTS.md", docs)
        self.assertIn("REPOSITORY_BOUNDARIES.md", docs)
        self.assertIn("*-monorepo/apps", docs["REPOSITORY_BOUNDARIES.md"])

    def test_branch_name_is_bounded(self) -> None:
        branch = target.unique_branch("Example-Organization")
        self.assertTrue(branch.startswith(target.BRANCH_PREFIX))
        self.assertLessEqual(len(branch), 240)


if __name__ == "__main__":
    unittest.main()
