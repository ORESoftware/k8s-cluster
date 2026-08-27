#!/usr/bin/env python3
from __future__ import annotations

import unittest

from repository_fleet_remote_state import (
    RemoteFleetStateError,
    classify_remote_fleet,
    verify_created_repositories,
    verify_preserved_existing,
)


class FakeRemote:
    def __init__(self) -> None:
        self.repositories: dict[str, dict[str, object]] = {}
        self.heads: dict[str, str] = {}

    def add(
        self,
        full_name: str,
        *,
        head: str,
        visibility: str = "private",
        default_branch: str = "main",
        repository_id: int | None = None,
        archived: bool = False,
    ) -> None:
        key = full_name.casefold()
        self.repositories[key] = {
            "id": repository_id,
            "full_name": full_name,
            "private": visibility == "private",
            "visibility": visibility,
            "default_branch": default_branch,
            "archived": archived,
            "disabled": False,
        }
        self.heads[key] = head

    def lookup(self, full_name: str) -> tuple[int, dict[str, object] | None]:
        value = self.repositories.get(full_name.casefold())
        return (200, value) if value is not None else (404, None)

    def main_ref(self, full_name: str) -> str | None:
        return self.heads.get(full_name.casefold())


class RepositoryFleetRemoteStateTests(unittest.TestCase):
    def records(self) -> list[dict[str, object]]:
        return [
            {
                "full_name": "hypesiege/hypesiege-analytics.rs",
                "commit": "a" * 40,
                "kind": "rust-worker",
            },
            {
                "full_name": "hypesiege/hypesiege-api-server.rs",
                "commit": "b" * 40,
                "kind": "rust-api",
            },
            {
                "full_name": "hypesiege/hypesiege-monorepo",
                "commit": "c" * 40,
                "kind": "monorepo",
            },
        ]

    def test_missing_leaves_are_ordered_before_missing_monorepo(self) -> None:
        remote = FakeRemote()
        missing, existing = classify_remote_fleet(
            self.records(),
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
        )
        self.assertEqual(existing, {})
        self.assertEqual(
            [record["full_name"] for record in missing],
            [
                "hypesiege/hypesiege-analytics.rs",
                "hypesiege/hypesiege-api-server.rs",
                "hypesiege/hypesiege-monorepo",
            ],
        )

    def test_divergent_existing_repository_is_preserved_not_republished(self) -> None:
        remote = FakeRemote()
        remote.add(
            "hypesiege/hypesiege-api-server.rs",
            head="d" * 40,
            repository_id=42,
        )
        missing, existing = classify_remote_fleet(
            self.records(),
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
        )
        self.assertEqual(
            [record["full_name"] for record in missing],
            [
                "hypesiege/hypesiege-analytics.rs",
                "hypesiege/hypesiege-monorepo",
            ],
        )
        self.assertEqual(existing["hypesiege/hypesiege-api-server.rs"]["head"], "d" * 40)
        self.assertFalse(
            existing["hypesiege/hypesiege-api-server.rs"]["matches_sealed_commit"]
        )
        verify_preserved_existing(
            existing,
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
        )

    def test_existing_sealed_repository_is_still_classified_as_preserved(self) -> None:
        remote = FakeRemote()
        remote.add(
            "hypesiege/hypesiege-api-server.rs",
            head="b" * 40,
            repository_id=7,
        )
        _, existing = classify_remote_fleet(
            self.records(),
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
        )
        self.assertTrue(
            existing["hypesiege/hypesiege-api-server.rs"]["matches_sealed_commit"]
        )

    def test_public_archived_non_main_or_headless_existing_repo_fails_closed(self) -> None:
        cases = (
            {"visibility": "public"},
            {"archived": True},
            {"default_branch": "master"},
        )
        for overrides in cases:
            with self.subTest(overrides=overrides):
                remote = FakeRemote()
                remote.add(
                    "hypesiege/hypesiege-api-server.rs",
                    head="b" * 40,
                    **overrides,
                )
                with self.assertRaises(RemoteFleetStateError):
                    classify_remote_fleet(
                        self.records(),
                        repository_lookup=remote.lookup,
                        main_ref_lookup=remote.main_ref,
                    )

        remote = FakeRemote()
        remote.add("hypesiege/hypesiege-api-server.rs", head="b" * 40)
        remote.heads.pop("hypesiege/hypesiege-api-server.rs")
        with self.assertRaisesRegex(RemoteFleetStateError, "valid main SHA"):
            classify_remote_fleet(
                self.records(),
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
            )

    def test_created_repository_must_match_sealed_commit(self) -> None:
        remote = FakeRemote()
        record = self.records()[0]
        full_name = str(record["full_name"])
        remote.add(full_name, head="a" * 40)
        verify_created_repositories(
            [record],
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
        )
        remote.heads[full_name] = "f" * 40
        with self.assertRaisesRegex(RemoteFleetStateError, "main drift"):
            verify_created_repositories(
                [record],
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
            )

    def test_preserved_repository_head_or_id_change_is_detected(self) -> None:
        remote = FakeRemote()
        remote.add(
            "hypesiege/hypesiege-api-server.rs",
            head="d" * 40,
            repository_id=42,
        )
        _, existing = classify_remote_fleet(
            self.records(),
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
        )

        remote.heads["hypesiege/hypesiege-api-server.rs"] = "e" * 40
        with self.assertRaisesRegex(RemoteFleetStateError, "changed during"):
            verify_preserved_existing(
                existing,
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
            )

        remote.heads["hypesiege/hypesiege-api-server.rs"] = "d" * 40
        remote.repositories["hypesiege/hypesiege-api-server.rs"]["id"] = 99
        with self.assertRaisesRegex(RemoteFleetStateError, "identity changed"):
            verify_preserved_existing(
                existing,
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
            )

    def test_duplicate_casefold_identity_and_malformed_records_fail_closed(self) -> None:
        remote = FakeRemote()
        records = self.records()
        records.append(
            {
                "full_name": "HYPESIEGE/HYPESIEGE-ANALYTICS.RS",
                "commit": "f" * 40,
                "kind": "rust-worker",
            }
        )
        with self.assertRaisesRegex(RemoteFleetStateError, "duplicate repository"):
            classify_remote_fleet(
                records,
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
            )

        for bad in (
            {"full_name": "not-a-repository", "commit": "a" * 40},
            {"full_name": "hypesiege/bad", "commit": "A" * 40},
            "not-an-object",
        ):
            with self.subTest(bad=bad):
                with self.assertRaises(RemoteFleetStateError):
                    classify_remote_fleet(
                        [bad],  # type: ignore[list-item]
                        repository_lookup=remote.lookup,
                        main_ref_lookup=remote.main_ref,
                    )


if __name__ == "__main__":
    unittest.main()
