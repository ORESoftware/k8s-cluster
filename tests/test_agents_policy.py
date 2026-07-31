from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

TOOLS = Path(__file__).resolve().parents[1] / "tools"
sys.path.insert(0, str(TOOLS))

import agents_policy  # noqa: E402


class ResolverTests(unittest.TestCase):
    def test_root_to_leaf_order_excludes_siblings(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            child = root / "child"
            deep = child / "deep"
            sibling = root / "sibling"
            deep.mkdir(parents=True)
            sibling.mkdir()
            (root / "agents.md").write_text("root\n", encoding="utf-8")
            (child / "agents.md").write_text("child\n", encoding="utf-8")
            (sibling / "agents.md").write_text("sibling\n", encoding="utf-8")

            result = agents_policy.resolve_chain(deep)

            self.assertEqual(
                result.chain,
                (
                    (root / "agents.md").resolve(),
                    (child / "agents.md").resolve(),
                ),
            )
            self.assertEqual(result.diagnostics, ())

    @unittest.skipIf(os.name == "nt", "symlink fixture requires Unix CI")
    def test_resolved_identity_is_deduplicated(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            child = root / "child"
            child.mkdir()
            (root / "agents.md").write_text("root\n", encoding="utf-8")
            (child / "agents.md").symlink_to("../agents.md")

            result = agents_policy.resolve_chain(child)

            self.assertEqual(result.chain, ((root / "agents.md").resolve(),))
            self.assertEqual(result.diagnostics, ())

    @unittest.skipIf(os.name == "nt", "symlink fixture requires Unix CI")
    def test_symlink_cycle_is_reported_without_looping(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            child = root / "child"
            child.mkdir()
            (root / "agents.md").write_text("root\n", encoding="utf-8")
            (child / "agents.md").symlink_to("agents.md")

            result = agents_policy.resolve_chain(child)

            self.assertEqual(result.chain, ((root / "agents.md").resolve(),))
            self.assertTrue(result.diagnostics)
            self.assertIn("cycle", result.diagnostics[0])

    def test_unreadable_file_is_reported_and_skipped(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            child = root / "child"
            child.mkdir()
            root_agents = root / "agents.md"
            child_agents = child / "agents.md"
            root_agents.write_text("root\n", encoding="utf-8")
            child_agents.write_text("child\n", encoding="utf-8")
            real_access = agents_policy.os.access

            def access(path: os.PathLike[str] | str, mode: int) -> bool:
                if Path(path) == child_agents.resolve():
                    return False
                return real_access(path, mode)

            with mock.patch.object(agents_policy.os, "access", side_effect=access):
                result = agents_policy.resolve_chain(child)

            self.assertEqual(result.chain, (root_agents.resolve(),))
            self.assertEqual(len(result.diagnostics), 1)
            self.assertIn("unreadable", result.diagnostics[0])


class ValidationTests(unittest.TestCase):
    def make_valid_repo(self, root: Path) -> None:
        (root / "agents.md").write_text("# Agent instructions\n", encoding="utf-8")
        for relative in agents_policy.POINTERS:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(agents_policy.POINTER_TEXT, encoding="utf-8")

    def test_minimal_pointer_layout_is_valid(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.make_valid_repo(root)

            result = agents_policy.validate_repo(root)

            self.assertEqual(result.issues, ())
            self.assertTrue(result.chains)
            self.assertEqual(
                result.chains[-1],
                ((root / "agents.md").resolve(),),
            )

    def test_duplicated_tool_guidance_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.make_valid_repo(root)
            (root / ".claude/CLAUDE.md").write_text(
                "# Duplicate instructions\nDo unrelated work.\n",
                encoding="utf-8",
            )

            result = agents_policy.validate_repo(root)

            self.assertTrue(
                any("one-line pointer" in issue for issue in result.issues),
                result.issues,
            )

    def test_uppercase_root_duplicate_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.make_valid_repo(root)
            (root / "AGENTS.md").write_text("duplicate\n", encoding="utf-8")

            result = agents_policy.validate_repo(root)

            self.assertTrue(
                any("duplicate root" in issue for issue in result.issues),
                result.issues,
            )


if __name__ == "__main__":
    unittest.main()
