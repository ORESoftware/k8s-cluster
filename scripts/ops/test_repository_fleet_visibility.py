#!/usr/bin/env python3
from __future__ import annotations

from copy import deepcopy
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
            "organizations": {"example": 2},
            "repositories": [
                {
                    "full_name": "example/api.rs",
                    "commit": "a" * 40,
                    "files": 12,
                    "gitlinks": 0,
                    "description": "API",
                    "visibility": "public",
                    "metadata": {
                        "owners": ["api-team"],
                        "nested": {"enabled": True},
                    },
                },
                {
                    "full_name": "example/monorepo",
                    "commit": "b" * 40,
                    "files": 20,
                    "gitlinks": 1,
                    "description": "Monorepo",
                    "visibility": "private",
                    "metadata": {
                        "owners": ["platform-team"],
                        "nested": {"enabled": False},
                    },
                },
            ],
        }

    def repositories(self, manifest: dict[str, object]) -> list[dict[str, object]]:
        repositories = manifest["repositories"]
        assert isinstance(repositories, list)
        assert all(isinstance(repository, dict) for repository in repositories)
        return repositories

    def test_projection_is_private_and_does_not_mutate_reviewed_ledger(self) -> None:
        reviewed = self.manifest()
        original = deepcopy(reviewed)

        projected = project_private_execution_manifest(reviewed)

        self.assertEqual(reviewed, original)
        repositories = self.repositories(projected)
        self.assertEqual(
            [repository["visibility"] for repository in repositories],
            ["private", "private"],
        )
        self.assertEqual(
            [repository["commit"] for repository in repositories],
            ["a" * 40, "b" * 40],
        )
        self.assertEqual(
            [repository["full_name"] for repository in repositories],
            ["example/api.rs", "example/monorepo"],
        )

    def test_projection_is_a_deep_copy_including_nested_metadata(self) -> None:
        reviewed = self.manifest()
        projected = project_private_execution_manifest(reviewed)
        projected_repositories = self.repositories(projected)
        projected_metadata = projected_repositories[0]["metadata"]
        assert isinstance(projected_metadata, dict)
        projected_owners = projected_metadata["owners"]
        assert isinstance(projected_owners, list)
        projected_owners.append("new-owner")
        projected_nested = projected_metadata["nested"]
        assert isinstance(projected_nested, dict)
        projected_nested["enabled"] = False

        reviewed_repositories = self.repositories(reviewed)
        reviewed_metadata = reviewed_repositories[0]["metadata"]
        assert isinstance(reviewed_metadata, dict)
        self.assertEqual(reviewed_metadata["owners"], ["api-team"])
        self.assertEqual(reviewed_metadata["nested"], {"enabled": True})

    def test_projection_is_idempotent(self) -> None:
        projected = project_private_execution_manifest(self.manifest())
        self.assertEqual(project_private_execution_manifest(projected), projected)

    def test_invalid_visibility_fails_closed(self) -> None:
        for invalid_visibility in (None, "", "internal", "PUBLIC", True):
            with self.subTest(visibility=invalid_visibility):
                reviewed = self.manifest()
                self.repositories(reviewed)[0]["visibility"] = invalid_visibility
                with self.assertRaisesRegex(
                    VisibilityProjectionError, "invalid visibility"
                ):
                    project_private_execution_manifest(reviewed)

    def test_missing_repository_ledger_fails_closed(self) -> None:
        for manifest in ({}, {"repository_count": 0, "repositories": []}):
            with self.subTest(manifest=manifest):
                with self.assertRaisesRegex(
                    VisibilityProjectionError, "no repository ledger"
                ):
                    project_private_execution_manifest(manifest)

    def test_non_object_manifest_and_repository_fail_closed(self) -> None:
        with self.assertRaisesRegex(
            VisibilityProjectionError, "manifest is not an object"
        ):
            project_private_execution_manifest([])  # type: ignore[arg-type]

        with self.assertRaisesRegex(
            VisibilityProjectionError, "is not an object"
        ):
            project_private_execution_manifest(
                {"repository_count": 1, "repositories": ["not-an-object"]}
            )

    def test_repository_count_must_be_an_exact_integer_match(self) -> None:
        for invalid_count in (None, True, 1, 3, "2"):
            with self.subTest(repository_count=invalid_count):
                reviewed = self.manifest()
                reviewed["repository_count"] = invalid_count
                with self.assertRaisesRegex(
                    VisibilityProjectionError, "repository_count"
                ):
                    project_private_execution_manifest(reviewed)

    def test_duplicate_full_names_are_rejected_case_insensitively(self) -> None:
        reviewed = self.manifest()
        self.repositories(reviewed)[1]["full_name"] = "EXAMPLE/API.RS"
        with self.assertRaisesRegex(
            VisibilityProjectionError, "duplicates full_name"
        ):
            project_private_execution_manifest(reviewed)

    def test_full_name_must_be_one_owner_and_repository_pair(self) -> None:
        for invalid_name in (
            None,
            "",
            "example",
            "example/",
            "/api.rs",
            "example/team/api.rs",
            "example/api rs",
        ):
            with self.subTest(full_name=invalid_name):
                reviewed = self.manifest()
                self.repositories(reviewed)[0]["full_name"] = invalid_name
                with self.assertRaisesRegex(
                    VisibilityProjectionError, "invalid full_name"
                ):
                    project_private_execution_manifest(reviewed)

    def test_commit_must_be_lowercase_40_character_hex(self) -> None:
        for invalid_commit in (
            None,
            "",
            "a" * 39,
            "a" * 41,
            "A" * 40,
            "g" * 40,
        ):
            with self.subTest(commit=invalid_commit):
                reviewed = self.manifest()
                self.repositories(reviewed)[0]["commit"] = invalid_commit
                with self.assertRaisesRegex(
                    VisibilityProjectionError, "invalid commit"
                ):
                    project_private_execution_manifest(reviewed)


if __name__ == "__main__":
    unittest.main()
