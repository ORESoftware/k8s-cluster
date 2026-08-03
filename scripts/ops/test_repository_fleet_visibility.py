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
            "metadata": {
                "source": {"repository": "ORESoftware/ai-agent-coordinator.rs"},
                "tags": ["sealed", "reviewed"],
            },
            "repositories": [
                {
                    "full_name": "example/api.rs",
                    "commit": "a" * 40,
                    "files": 12,
                    "gitlinks": 0,
                    "description": "API",
                    "visibility": "public",
                    "metadata": {"language": "Rust", "labels": ["api"]},
                },
                {
                    "full_name": "example/monorepo",
                    "commit": "b" * 40,
                    "files": 20,
                    "gitlinks": 1,
                    "description": "Monorepo",
                    "visibility": "private",
                    "metadata": {"language": "TypeScript", "labels": ["web"]},
                },
            ],
        }

    def test_projection_is_private_and_does_not_mutate_reviewed_ledger(self) -> None:
        reviewed = self.manifest()
        original = deepcopy(reviewed)

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
        self.assertEqual(
            [repository["description"] for repository in repositories],
            ["API", "Monorepo"],
        )
        self.assertEqual(projected["repository_count"], 2)

    def test_projection_is_deep_copy_for_nested_manifest_and_repository_metadata(
        self,
    ) -> None:
        reviewed = self.manifest()
        projected = project_private_execution_manifest(reviewed)

        projected_metadata = projected["metadata"]
        assert isinstance(projected_metadata, dict)
        projected_source = projected_metadata["source"]
        assert isinstance(projected_source, dict)
        projected_source["repository"] = "changed/source"

        projected_repositories = projected["repositories"]
        assert isinstance(projected_repositories, list)
        projected_repository_metadata = projected_repositories[0]["metadata"]
        assert isinstance(projected_repository_metadata, dict)
        projected_labels = projected_repository_metadata["labels"]
        assert isinstance(projected_labels, list)
        projected_labels.append("changed")

        reviewed_metadata = reviewed["metadata"]
        assert isinstance(reviewed_metadata, dict)
        reviewed_source = reviewed_metadata["source"]
        assert isinstance(reviewed_source, dict)
        self.assertEqual(
            reviewed_source["repository"],
            "ORESoftware/ai-agent-coordinator.rs",
        )
        reviewed_repositories = reviewed["repositories"]
        assert isinstance(reviewed_repositories, list)
        reviewed_repository_metadata = reviewed_repositories[0]["metadata"]
        assert isinstance(reviewed_repository_metadata, dict)
        self.assertEqual(reviewed_repository_metadata["labels"], ["api"])

    def test_projection_accepts_manifest_without_optional_repository_count(self) -> None:
        reviewed = self.manifest()
        reviewed.pop("repository_count")

        projected = project_private_execution_manifest(reviewed)

        self.assertNotIn("repository_count", projected)

    def test_repository_count_mismatch_fails_without_mutation(self) -> None:
        reviewed = self.manifest()
        reviewed["repository_count"] = 3
        original = deepcopy(reviewed)

        with self.assertRaisesRegex(
            VisibilityProjectionError, "does not match repository ledger"
        ):
            project_private_execution_manifest(reviewed)

        self.assertEqual(reviewed, original)

    def test_non_integer_repository_count_fails_closed(self) -> None:
        for invalid_count in ("2", 2.0, True):
            with self.subTest(invalid_count=invalid_count):
                reviewed = self.manifest()
                reviewed["repository_count"] = invalid_count

                with self.assertRaisesRegex(
                    VisibilityProjectionError, "is not an integer"
                ):
                    project_private_execution_manifest(reviewed)

    def test_duplicate_full_name_fails_without_mutation(self) -> None:
        reviewed = self.manifest()
        repositories = reviewed["repositories"]
        assert isinstance(repositories, list)
        repositories[1]["full_name"] = repositories[0]["full_name"]
        original = deepcopy(reviewed)

        with self.assertRaisesRegex(
            VisibilityProjectionError, "duplicate full_name"
        ):
            project_private_execution_manifest(reviewed)

        self.assertEqual(reviewed, original)

    def test_invalid_full_name_fails_closed(self) -> None:
        invalid_names = (
            None,
            "",
            "owner",
            "/repo",
            "owner/",
            "owner/repo/extra",
            "owner /repo",
            "owner/repo name",
            "owner\n/repo",
        )
        for invalid_name in invalid_names:
            with self.subTest(invalid_name=invalid_name):
                reviewed = self.manifest()
                repositories = reviewed["repositories"]
                assert isinstance(repositories, list)
                repositories[0]["full_name"] = invalid_name

                with self.assertRaisesRegex(
                    VisibilityProjectionError, "invalid full_name"
                ):
                    project_private_execution_manifest(reviewed)

    def test_repository_identity_comparison_is_case_sensitive(self) -> None:
        reviewed = self.manifest()
        repositories = reviewed["repositories"]
        assert isinstance(repositories, list)
        repositories[1]["full_name"] = "Example/api.rs"

        projected = project_private_execution_manifest(reviewed)

        projected_repositories = projected["repositories"]
        assert isinstance(projected_repositories, list)
        self.assertEqual(
            [record["full_name"] for record in projected_repositories],
            ["example/api.rs", "Example/api.rs"],
        )

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
        invalid_manifests = (
            {},
            {"repositories": None},
            {"repositories": "not-a-list"},
            {"repositories": []},
        )
        for invalid_manifest in invalid_manifests:
            with self.subTest(invalid_manifest=invalid_manifest):
                with self.assertRaisesRegex(
                    VisibilityProjectionError, "no repository ledger"
                ):
                    project_private_execution_manifest(invalid_manifest)

    def test_non_object_manifest_fails_closed(self) -> None:
        with self.assertRaisesRegex(
            VisibilityProjectionError, "manifest is not an object"
        ):
            project_private_execution_manifest([])  # type: ignore[arg-type]

    def test_non_object_repository_fails_closed(self) -> None:
        with self.assertRaisesRegex(
            VisibilityProjectionError, "is not an object"
        ):
            project_private_execution_manifest(
                {"repository_count": 1, "repositories": ["not-an-object"]}
            )


if __name__ == "__main__":
    unittest.main()
