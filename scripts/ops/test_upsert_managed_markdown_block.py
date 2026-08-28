#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("upsert_managed_markdown_block.py")
SPEC = importlib.util.spec_from_file_location("managed_markdown", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ManagedBlockTests(unittest.TestCase):
    def test_empty_file_has_no_leading_blank_lines_and_is_idempotent(self):
        original = ""
        first = MODULE.render_managed_block(original, "routing", "# Routing\n")
        self.assertTrue(first.startswith("<!-- routing:start -->"))
        self.assertFalse(first.startswith("\n"))
        self.assertEqual(
            MODULE.render_managed_block(first, "routing", "# Routing\n"),
            first,
        )

    def test_existing_leading_blank_lines_are_normalized_once(self):
        original = (
            "\n\n<!-- routing:start -->\nold\n<!-- routing:end -->\n"
        )
        expected = (
            "<!-- routing:start -->\nnew\n<!-- routing:end -->\n"
        )
        first = MODULE.render_managed_block(original, "routing", "new")
        self.assertEqual(first, expected)
        self.assertEqual(
            MODULE.render_managed_block(first, "routing", "new"),
            expected,
        )

    def test_unrelated_prose_is_preserved(self):
        original = (
            "# Header\n\n"
            "<!-- routing:start -->\nold\n<!-- routing:end -->\n\n"
            "Footer with  two  spaces.\n"
        )
        expected = (
            "# Header\n\n"
            "<!-- routing:start -->\nnew\n<!-- routing:end -->\n\n"
            "Footer with  two  spaces.\n"
        )
        self.assertEqual(
            MODULE.render_managed_block(original, "routing", "new"),
            expected,
        )

    def test_new_block_is_appended_after_existing_prose(self):
        self.assertEqual(
            MODULE.render_managed_block("# Header\n", "routing", "new"),
            "# Header\n\n<!-- routing:start -->\nnew\n<!-- routing:end -->\n",
        )

    def test_malformed_or_duplicate_markers_are_rejected(self):
        with self.assertRaises(MODULE.ManagedBlockError):
            MODULE.render_managed_block(
                "<!-- routing:start -->\nmissing end\n",
                "routing",
                "new",
            )
        with self.assertRaises(MODULE.ManagedBlockError):
            MODULE.render_managed_block(
                "<!-- routing:start -->\na\n<!-- routing:end -->\n"
                "<!-- routing:start -->\nb\n<!-- routing:end -->\n",
                "routing",
                "new",
            )

    def test_file_upsert_returns_change_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "docs/PROJECTS.md"
            block = root / "block.md"
            block.write_text("# Routing\n", encoding="utf-8")
            self.assertTrue(
                MODULE.upsert_managed_block(target, "routing", block)
            )
            self.assertFalse(
                MODULE.upsert_managed_block(target, "routing", block)
            )


if __name__ == "__main__":
    unittest.main()
