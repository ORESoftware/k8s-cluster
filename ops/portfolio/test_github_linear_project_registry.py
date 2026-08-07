#!/usr/bin/env python3
"""Mutation tests for the fleet GitHub/Linear/Project registry."""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("validate_github_linear_project_registry.py")
SPEC = importlib.util.spec_from_file_location("fleet_registry_validator", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
validator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validator)

REGISTRY = Path(__file__).with_name("github-linear-project-registry.tsv")


class RegistryTests(unittest.TestCase):
    def rows(self) -> list[list[str]]:
        return [line.split("\t") for line in REGISTRY.read_text(encoding="utf-8").splitlines()]

    def write(self, rows: list[list[str]], *, newline: bool = True) -> Path:
        directory = tempfile.TemporaryDirectory(prefix="fleet-registry-test-")
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "registry.tsv"
        content = "\n".join("\t".join(row) for row in rows)
        path.write_text(content + ("\n" if newline else ""), encoding="utf-8")
        return path

    def assert_rejected(self, rows: list[list[str]], message: str) -> None:
        with self.assertRaisesRegex(validator.RegistryError, message):
            validator.validate(self.write(rows))

    def test_current_registry_is_complete_and_unique(self) -> None:
        self.assertEqual(validator.validate(REGISTRY), (64, 64, 64))

    def test_rejects_project_title_drift(self) -> None:
        rows = self.rows()
        rows[1][1] = "wrong-project"
        self.assert_rejected(rows, "project title must be")

    def test_rejects_wrong_project_number_exception(self) -> None:
        rows = self.rows()
        index = next(i for i, row in enumerate(rows) if row[0] == "dancing-dragons")
        rows[index][2] = "https://github.com/orgs/dancing-dragons/projects/1"
        self.assert_rejected(rows, "GitHub Project URL must be exactly")

    def test_rejects_duplicate_linear_identity(self) -> None:
        rows = self.rows()
        rows[2][3] = rows[1][3]
        self.assert_rejected(rows, "duplicates Linear URL")

    def test_rejects_case_insensitive_organization_collision(self) -> None:
        rows = self.rows()
        rows[2][0] = "3fa-APP"
        rows[2][1] = "3fa-APP-project"
        rows[2][2] = "https://github.com/orgs/3fa-APP/projects/1"
        self.assert_rejected(rows, "duplicates organization")

    def test_rejects_project_url_query_and_credentials(self) -> None:
        for value in (
            "https://github.com/orgs/3FA-app/projects/1?view=1",
            "https://user@github.com/orgs/3FA-app/projects/1",
        ):
            with self.subTest(value=value):
                rows = self.rows()
                rows[1][2] = value
                self.assert_rejected(rows, "GitHub Project URL must be exactly")

    def test_rejects_schema_count_and_newline_drift(self) -> None:
        rows = self.rows()
        rows[0] = ["organization", "linear_url"]
        with self.assertRaisesRegex(validator.RegistryError, "header must be"):
            validator.validate(self.write(rows))

        rows = self.rows()[:-1]
        with self.assertRaisesRegex(validator.RegistryError, "expected 64 organizations"):
            validator.validate(self.write(rows))

        with self.assertRaisesRegex(validator.RegistryError, "must end with one newline"):
            validator.validate(self.write(self.rows(), newline=False))


if __name__ == "__main__":
    unittest.main()
