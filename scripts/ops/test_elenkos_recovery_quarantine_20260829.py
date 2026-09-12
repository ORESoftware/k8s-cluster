#!/usr/bin/env python3
"""Hermetic negative tests: no GitHub connection or credentials required."""
from __future__ import annotations

import ast
import importlib.util
from pathlib import Path
import re
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

MODULE_PATH = Path(__file__).with_name("patch_elenkos_managed_tree_reconcile_20260820.py")
spec = importlib.util.spec_from_file_location("quarantine", MODULE_PATH)
assert spec is not None and spec.loader is not None
quarantine = importlib.util.module_from_spec(spec)
spec.loader.exec_module(quarantine)

SOURCE = '''# unrelated module content must be preserved
SENTINEL = "preserved"
def create_full_commit(api, expected, parent_sha):
    return api.post("/git/commits", {})

def ensure_initial_tag(api, expected, main_sha, main_commit, tag_sha):
    return api.patch("/git/refs/tags/v0.1.0", {"force": True})

def recover_existing_repository(api, expected, main_sha):
    return create_full_commit(api, expected, main_sha)
'''
SHA = "a" * 40
OTHER = "b" * 40
FILES = {".elenkos-bootstrap.json": ("c" * 40, "100644"),
         "src/lib.rs": ("d" * 40, "100644")}


class RewriteTests(unittest.TestCase):
    def test_preserves_unrelated_source_and_is_idempotent(self):
        rewritten, result = quarantine.rewrite_source(SOURCE)
        self.assertEqual(result, "applied")
        self.assertIn('SENTINEL = "preserved"', rewritten)
        self.assertEqual(quarantine.rewrite_source(rewritten), (rewritten, "already-applied"))

    def test_generated_functions_have_no_mutating_api_calls(self):
        rewritten, _ = quarantine.rewrite_source(SOURCE)
        attrs = {node.attr for node in ast.walk(ast.parse(rewritten)) if isinstance(node, ast.Attribute)}
        self.assertTrue(attrs.isdisjoint({"post", "put", "patch", "delete", "request"}))

    def test_rejects_each_legacy_widening_helper(self):
        for helper in quarantine.LEGACY_HELPERS:
            with self.subTest(helper=helper), self.assertRaises(ValueError):
                quarantine.rewrite_source(SOURCE + f"\ndef {helper}():\n    pass\n")

    def test_rejects_duplicate_missing_nested_and_changed_signatures(self):
        mutations = [
            SOURCE + "\ndef create_full_commit(api, expected, parent_sha):\n    pass\n",
            SOURCE.replace("def create_full_commit", "def removed"),
            SOURCE.replace("parent_sha):", "parent_sha, force=True):"),
            SOURCE.replace("def create_full_commit", "async def create_full_commit"),
            SOURCE.replace("def create_full_commit", "@decorator\ndef create_full_commit"),
            "def wrapper():\n" + "\n".join("    " + line for line in SOURCE.splitlines()),
        ]
        for source in mutations:
            with self.subTest(source=source[:60]), self.assertRaises(ValueError):
                quarantine.rewrite_source(source)

    def test_rejects_partial_quarantine(self):
        source = SOURCE.replace(
            'def create_full_commit(api, expected, parent_sha):\n    return api.post("/git/commits", {})\n',
            quarantine.REPLACEMENTS["create_full_commit"],
        )
        with self.assertRaisesRegex(ValueError, "partial"):
            quarantine.rewrite_source(source)

    def test_syntax_failure_does_not_modify_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, "recovery.py")
            path.write_text("def invalid(:\n", encoding="utf-8")
            before = path.read_bytes()
            with self.assertRaises(SyntaxError):
                quarantine.apply(path)
            self.assertEqual(before, path.read_bytes())

    def test_apply_is_idempotent_and_rejects_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, "recovery.py")
            path.write_text(SOURCE, encoding="utf-8")
            self.assertEqual(quarantine.apply(path), "applied")
            before = path.stat().st_mtime_ns
            self.assertEqual(quarantine.apply(path), "already-applied")
            self.assertEqual(before, path.stat().st_mtime_ns)
            link = Path(directory, "link.py")
            link.symlink_to(path)
            with self.assertRaises(ValueError):
                quarantine.apply(link)


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.api = Mock()
        self.expected = SimpleNamespace(full_name="example/component", git_files=FILES.copy(),
                                        initial_message="feat: initialize component")
        self.commit = {"sha": SHA, "message": self.expected.initial_message}
        self.commit_tree = Mock(return_value=(self.commit, FILES.copy()))
        self.read_ref = Mock(side_effect=[SHA, SHA, SHA])
        self.verify_marker = Mock()
        namespace = {"SHA_RE": re.compile(r"^[0-9a-f]{40}$"), "TAG": "v0.1.0",
                     "commit_tree": self.commit_tree, "read_ref": self.read_ref,
                     "verify_marker_blob": self.verify_marker}
        rewritten, _ = quarantine.rewrite_source(SOURCE)
        exec(compile(rewritten, "<fixture>", "exec"), namespace)
        self.recover = namespace["recover_existing_repository"]
        self.create = namespace["create_full_commit"]
        self.tag = namespace["ensure_initial_tag"]

    def tearDown(self):
        self.assertEqual(self.api.mock_calls, [], "no GitHub mutation may occur")

    def test_exact_tree_and_existing_tag_are_read_only_noop(self):
        self.assertEqual(self.recover(self.api, self.expected, SHA), ("full-live-tree:ready", SHA))
        self.assertEqual(self.read_ref.call_count, 3)
        self.verify_marker.assert_called_once()

    def test_self_certified_tree_and_recomputed_marker_are_rejected(self):
        forged = FILES | {"src/lib.rs": ("e" * 40, "100644"),
                          ".elenkos-bootstrap.json": ("f" * 40, "100644")}
        self.commit_tree.return_value = (self.commit, forged)
        with self.assertRaisesRegex(RuntimeError, "unapproved tree"):
            self.recover(self.api, self.expected, SHA)
        self.read_ref.assert_not_called()

    def test_marker_only_and_file_mode_drift_are_rejected(self):
        for files in [{".elenkos-bootstrap.json": FILES[".elenkos-bootstrap.json"]},
                      FILES | {"src/lib.rs": ("d" * 40, "100755")}, FILES | {"extra": (OTHER, "100644")}, {}]:
            self.commit_tree.return_value = (self.commit, files)
            with self.subTest(files=files), self.assertRaisesRegex(RuntimeError, "unapproved tree"):
                self.recover(self.api, self.expected, SHA)

    def test_missing_or_moved_tag_is_not_created_or_repaired(self):
        for tag_sha in [None, OTHER]:
            self.read_ref.side_effect = [tag_sha]
            with self.subTest(tag=tag_sha), self.assertRaisesRegex(RuntimeError, "tag creation"):
                self.recover(self.api, self.expected, SHA)

    def test_head_and_tag_races_are_rejected(self):
        for refs in [[SHA, OTHER], [SHA, SHA, OTHER], [SHA, None]]:
            self.read_ref.side_effect = refs
            with self.subTest(refs=refs), self.assertRaisesRegex(RuntimeError, "changed during"):
                self.recover(self.api, self.expected, SHA)

    def test_bad_sha_and_commit_identity_are_rejected(self):
        for value in [None, "", "main", "A" * 40, "a" * 39]:
            with self.subTest(value=value), self.assertRaisesRegex(RuntimeError, "invalid observed"):
                self.recover(self.api, self.expected, value)
        self.commit["sha"] = OTHER
        with self.assertRaisesRegex(RuntimeError, "identity changed"):
            self.recover(self.api, self.expected, SHA)

    def test_commit_message_and_marker_failure_are_rejected(self):
        self.commit["message"] = "unapproved work"
        with self.assertRaisesRegex(RuntimeError, "commit message"):
            self.recover(self.api, self.expected, SHA)
        self.commit["message"] = self.expected.initial_message
        self.verify_marker.side_effect = RuntimeError("marker invalid")
        with self.assertRaisesRegex(RuntimeError, "marker invalid"):
            self.recover(self.api, self.expected, SHA)

    def test_direct_mutator_entrypoints_are_quarantined(self):
        with self.assertRaisesRegex(RuntimeError, "approved predecessor"):
            self.create(self.api, self.expected, SHA)
        for tag_sha in [None, OTHER]:
            with self.assertRaisesRegex(RuntimeError, "prohibited"):
                self.tag(self.api, self.expected, SHA, self.commit, tag_sha)
        self.assertEqual(self.tag(self.api, self.expected, SHA, self.commit, SHA), "ready")


if __name__ == "__main__":
    unittest.main()
