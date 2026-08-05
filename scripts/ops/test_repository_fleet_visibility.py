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
            "total_tracked_files": 32,
            "total_gitlinks": 1,
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

    def test_schema_version_must_be_exact_v2(self) -> None:
        for invalid_version in (None, True, 1, 3, "2"):
            with self.subTest(schema_version=invalid_version):
                reviewed = self.manifest()
                reviewed["schema_version"] = invalid_version
                with self.assertRaisesRegex(
                    VisibilityProjectionError, "schema version 2"
                ):
                    project_private_execution_manifest(reviewed)

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
        for manifest in (
            {},
            {"schema_version": 2, "repository_count": 0, "repositories": []},
        ):
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
            reviewed = self.manifest()
            reviewed["repositories"] = ["not-an-object"]
            reviewed["repository_count"] = 1
            reviewed["organizations"] = {"example": 1}
            reviewed["total_tracked_files"] = 0
            reviewed["total_gitlinks"] = 0
            project_private_execution_manifest(reviewed)

    def test_repository_count_must_be_an_exact_integer_match(self) -> None:
        for invalid_count in (None, True, 1, 3, "2"):
            with self.subTest(repository_count=invalid_count):
                reviewed = self.manifest()
                reviewed["repository_count"] = invalid_count
                with self.assertRaisesRegex(
                    VisibilityProjectionError, "repository_count"
                ):
                    project_private_execution_manifest(reviewed)

    def test_organization_counts_must_match_repository_count(self) -> None:
        for invalid_organizations in (
            None,
            {},
            {"example": True},
            {"example": 0},
            {"example": 1},
            {"example": 3},
        ):
            with self.subTest(organizations=invalid_organizations):
                reviewed = self.manifest()
                reviewed["organizations"] = invalid_organizations
                with self.assertRaisesRegex(
                    VisibilityProjectionError, "organization"
                ):
                    project_private_execution_manifest(reviewed)

    def test_organization_counts_must_match_repository_identities(self) -> None:
        reviewed = self.manifest()
        reviewed["organizations"] = {"example": 1, "other": 1}
        with self.assertRaisesRegex(
            VisibilityProjectionError, "repository identities"
        ):
            project_private_execution_manifest(reviewed)

    def test_duplicate_organizations_are_rejected_case_insensitively(self) -> None:
        reviewed = self.manifest()
        reviewed["organizations"] = {"example": 1, "EXAMPLE": 1}
        with self.assertRaisesRegex(
            VisibilityProjectionError, "duplicates organization"
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

    def test_repository_file_and_gitlink_counts_are_exact_non_negative_ints(self) -> None:
        for field in ("files", "gitlinks"):
            for invalid_value in (None, True, -1, "0"):
                with self.subTest(field=field, value=invalid_value):
                    reviewed = self.manifest()
                    self.repositories(reviewed)[0][field] = invalid_value
                    with self.assertRaisesRegex(
                        VisibilityProjectionError, f"invalid {field}"
                    ):
                        project_private_execution_manifest(reviewed)

    def test_top_level_file_and_gitlink_totals_match_repository_records(self) -> None:
        for field, invalid_value in (
            ("total_tracked_files", None),
            ("total_tracked_files", True),
            ("total_tracked_files", 31),
            ("total_tracked_files", "32"),
            ("total_gitlinks", None),
            ("total_gitlinks", True),
            ("total_gitlinks", 2),
            ("total_gitlinks", "1"),
        ):
            with self.subTest(field=field, value=invalid_value):
                reviewed = self.manifest()
                reviewed[field] = invalid_value
                with self.assertRaisesRegex(
                    VisibilityProjectionError, field
                ):
                    project_private_execution_manifest(reviewed)


if __name__ == "__main__":
    unittest.main()
