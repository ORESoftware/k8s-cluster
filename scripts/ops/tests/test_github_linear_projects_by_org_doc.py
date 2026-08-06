from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
MODULE_PATH = REPO_ROOT / "scripts" / "ops" / "validate_github_linear_projects_by_org_doc.py"
SPEC = importlib.util.spec_from_file_location("github_linear_org_directory", MODULE_PATH)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = module
SPEC.loader.exec_module(module)


class GitHubLinearProjectsByOrgDocumentTests(unittest.TestCase):
    def test_checked_in_directory_matches_canonical_registry(self) -> None:
        module.validate_document(module.DEFAULT_REGISTRY, module.DEFAULT_DOCUMENT)
        rows = module.load_registry(module.DEFAULT_REGISTRY)
        self.assertEqual(64, len(rows))
        dancing = next(row for row in rows if row.organization == "dancing-dragons")
        self.assertEqual(4, dancing.project_number)
        self.assertTrue(
            all(row.project_number == 1 for row in rows if row != dancing)
        )

    def test_write_repairs_generated_block_without_touching_surrounding_prose(self) -> None:
        rows = module.load_registry(module.DEFAULT_REGISTRY)
        with tempfile.TemporaryDirectory() as directory:
            document = Path(directory) / "directory.md"
            document.write_text(
                "before\n\n"
                + module.BEGIN_MARKER
                + "\n\ninvalid\n\n"
                + module.END_MARKER
                + "\n\nafter\n",
                encoding="utf-8",
            )
            updated = module.replace_generated_block(
                document.read_text(encoding="utf-8"),
                module.render_directory(rows),
            )
            document.write_text(updated, encoding="utf-8")
            module.validate_document(module.DEFAULT_REGISTRY, document)
            content = document.read_text(encoding="utf-8")
            self.assertTrue(content.startswith("before\n\n"))
            self.assertTrue(content.endswith("\n\nafter\n"))

    def test_document_drift_fails_closed(self) -> None:
        rows = module.load_registry(module.DEFAULT_REGISTRY)
        rendered = module.render_directory(rows).replace("projects/4", "projects/1", 1)
        with tempfile.TemporaryDirectory() as directory:
            document = Path(directory) / "directory.md"
            document.write_text(rendered + "\n", encoding="utf-8")
            with self.assertRaisesRegex(module.DirectoryError, "drifted"):
                module.validate_document(module.DEFAULT_REGISTRY, document)

    def test_registry_rejects_credential_query_and_duplicate_identity(self) -> None:
        invalid_registries = {
            "credential": (
                "organization\tlinear_url\n"
                "example\thttps://linear.app/denman/project/token=secret-value\n"
            ),
            "query": (
                "organization\tlinear_url\n"
                "example\thttps://linear.app/denman/project/example?redirect=1\n"
            ),
            "duplicate": (
                "organization\tlinear_url\n"
                "Example\thttps://linear.app/denman/project/example-one\n"
                "example\thttps://linear.app/denman/project/example-two\n"
            ),
        }
        with tempfile.TemporaryDirectory() as directory:
            for name, content in invalid_registries.items():
                with self.subTest(name=name):
                    path = Path(directory) / f"{name}.tsv"
                    path.write_text(content, encoding="utf-8")
                    with self.assertRaises(module.DirectoryError):
                        module.load_registry(path)

    def test_duplicate_or_missing_markers_fail_closed(self) -> None:
        rows = module.load_registry(module.DEFAULT_REGISTRY)
        rendered = module.render_directory(rows)
        with self.assertRaisesRegex(module.DirectoryError, "exactly one"):
            module.replace_generated_block(rendered + rendered, rendered)
        with self.assertRaisesRegex(module.DirectoryError, "exactly one"):
            module.replace_generated_block("no markers", rendered)


if __name__ == "__main__":
    unittest.main()
