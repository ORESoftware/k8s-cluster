#!/usr/bin/env python3
from __future__ import annotations

import unittest

from repository_fleet_visibility import (
    VisibilityProjectionError,
    project_private_execution_manifest,
)


class FleetVisibilityProjectionTests(unittest.TestCase):
    def manifest(self) -> dict[str, object]:
        return {
            "schema_version": 2,
            "repository_count": 2,
            "repositories": [
                {
                    "full_name": "example/api.rs",
                    "commit": "a" * 40,
                    "files": 12,
                    "gitlinks": 0,
                    "visibility": "public",
                },
                {
                    "full_name": "example/monorepo",
                    "commit": "b" * 40,
                    "files": 20,
                    "gitlinks": 1,
                    "visibility": "private",
                },
            ],
        }

    def test_projection_is_private_and_does_not_mutate_reviewed_ledger(self) -> None:
        reviewed = self.manifest()
        original = self.manifest()

        projected = project_private_execution_manifest(reviewed)

        self.assertEqual(reviewed, original)
        repositories = projected["repositories"]
        assert isinstance(repositories, list)
        self.assertEqual(
            [repository["visibility"] for repository in repositories],
            ["private", "private"],
        )
        self.assertEqual(
            [repository["commit"] for repository in repositories],
            ["a" * 40, "b" * 40],
        )

    def test_projection_is_deep_copy(self) -> None:
        reviewed = self.manifest()
        projected = project_private_execution_manifest(reviewed)
        repositories = projected["repositories"]
        assert isinstance(repositories, list)
        repositories[0]["commit"] = "c" * 40

        reviewed_repositories = reviewed["repositories"]
        assert isinstance(reviewed_repositories, list)
        self.assertEqual(reviewed_repositories[0]["commit"], "a" * 40)

    def test_invalid_visibility_fails_closed(self) -> None:
        reviewed = self.manifest()
        repositories = reviewed["repositories"]
        assert isinstance(repositories, list)
        repositories[0]["visibility"] = None

        with self.assertRaisesRegex(
            VisibilityProjectionError, "invalid visibility"
        ):
            project_private_execution_manifest(reviewed)

    def test_missing_repository_ledger_fails_closed(self) -> None:
        with self.assertRaisesRegex(
            VisibilityProjectionError, "no repository ledger"
        ):
            project_private_execution_manifest({"repositories": []})


if __name__ == "__main__":
    unittest.main()
