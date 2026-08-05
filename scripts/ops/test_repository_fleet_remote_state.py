#!/usr/bin/env python3
from __future__ import annotations

from copy import deepcopy
import unittest

from repository_fleet_aliases import RepositoryAlias
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
        lookup_name: str | None = None,
    ) -> None:
        request_name = lookup_name or full_name
        payload = {
            "id": repository_id,
            "full_name": full_name,
            "private": visibility == "private",
            "visibility": visibility,
            "default_branch": default_branch,
            "archived": archived,
            "disabled": False,
        }
        self.repositories[request_name.casefold()] = payload
        self.repositories.setdefault(full_name.casefold(), payload)
        self.heads[full_name.casefold()] = head

    def add_redirect(
        self,
        sealed_full_name: str,
        remote_full_name: str,
        *,
        head: str,
        repository_id: int,
    ) -> None:
        self.add(
            remote_full_name,
            lookup_name=sealed_full_name,
            head=head,
            repository_id=repository_id,
        )

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

    def alias_record(self) -> dict[str, object]:
        return {
            "full_name": "streempilot/streempilot-flutter-app",
            "commit": "d" * 40,
            "kind": "flutter",
        }

    def aliases(self) -> dict[str, RepositoryAlias]:
        alias = RepositoryAlias(
            sealed_full_name="streempilot/streempilot-flutter-app",
            remote_full_name="StreemPilot/sp-web-leptos",
            repository_id=1318677943,
        )
        return {alias.sealed_full_name.casefold(): alias}

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
        state = existing["hypesiege/hypesiege-api-server.rs"]
        self.assertEqual(state["head"], "d" * 40)
        self.assertEqual(
            state["remote_full_name"], "hypesiege/hypesiege-api-server.rs"
        )
        self.assertFalse(state["renamed"])
        self.assertFalse(state["matches_sealed_commit"])
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

    def test_reviewed_redirect_is_preserved_by_target_id_and_head(self) -> None:
        remote = FakeRemote()
        remote.add_redirect(
            "streempilot/streempilot-flutter-app",
            "StreemPilot/sp-web-leptos",
            head="e" * 40,
            repository_id=1318677943,
        )
        missing, existing = classify_remote_fleet(
            [self.alias_record()],
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
            repository_aliases=self.aliases(),
        )
        self.assertEqual(missing, [])
        state = existing["streempilot/streempilot-flutter-app"]
        self.assertTrue(state["renamed"])
        self.assertEqual(state["remote_full_name"], "StreemPilot/sp-web-leptos")
        self.assertEqual(state["repository_id"], 1318677943)
        self.assertEqual(state["head"], "e" * 40)
        verify_preserved_existing(
            existing,
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
            repository_aliases=self.aliases(),
        )

    def test_unlisted_redirect_and_missing_reviewed_alias_fail_closed(self) -> None:
        remote = FakeRemote()
        remote.add_redirect(
            "streempilot/streempilot-flutter-app",
            "StreemPilot/sp-web-leptos",
            head="e" * 40,
            repository_id=1318677943,
        )
        with self.assertRaisesRegex(RemoteFleetStateError, "unexpected repository"):
            classify_remote_fleet(
                [self.alias_record()],
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
            )

        missing_remote = FakeRemote()
        with self.assertRaisesRegex(RemoteFleetStateError, "alias source no longer resolves"):
            classify_remote_fleet(
                [self.alias_record()],
                repository_lookup=missing_remote.lookup,
                main_ref_lookup=missing_remote.main_ref,
                repository_aliases=self.aliases(),
            )

    def test_alias_target_and_pinned_repository_id_must_match(self) -> None:
        remote = FakeRemote()
        remote.add_redirect(
            "streempilot/streempilot-flutter-app",
            "StreemPilot/sp-web-dioxus",
            head="e" * 40,
            repository_id=1318677943,
        )
        with self.assertRaisesRegex(RemoteFleetStateError, "unreviewed rename"):
            classify_remote_fleet(
                [self.alias_record()],
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
                repository_aliases=self.aliases(),
            )

        remote = FakeRemote()
        remote.add_redirect(
            "streempilot/streempilot-flutter-app",
            "StreemPilot/sp-web-leptos",
            head="e" * 40,
            repository_id=99,
        )
        with self.assertRaisesRegex(RemoteFleetStateError, "repository ID changed"):
            classify_remote_fleet(
                [self.alias_record()],
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
                repository_aliases=self.aliases(),
            )

    def test_alias_target_id_and_head_changes_after_snapshot_are_detected(self) -> None:
        remote = FakeRemote()
        remote.add_redirect(
            "streempilot/streempilot-flutter-app",
            "StreemPilot/sp-web-leptos",
            head="e" * 40,
            repository_id=1318677943,
        )
        _, existing = classify_remote_fleet(
            [self.alias_record()],
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
            repository_aliases=self.aliases(),
        )

        redirected = remote.repositories[
            "streempilot/streempilot-flutter-app"
        ]
        redirected["full_name"] = "StreemPilot/sp-web-dioxus"
        with self.assertRaisesRegex(RemoteFleetStateError, "unreviewed rename"):
            verify_preserved_existing(
                existing,
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
                repository_aliases=self.aliases(),
            )

        redirected["full_name"] = "StreemPilot/sp-web-leptos"
        redirected["id"] = 99
        with self.assertRaisesRegex(RemoteFleetStateError, "repository ID changed"):
            verify_preserved_existing(
                existing,
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
                repository_aliases=self.aliases(),
            )

        redirected["id"] = 1318677943
        remote.heads["streempilot/sp-web-leptos"] = "f" * 40
        with self.assertRaisesRegex(RemoteFleetStateError, "changed during"):
            verify_preserved_existing(
                existing,
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
                repository_aliases=self.aliases(),
            )

    def test_newly_created_repository_cannot_be_satisfied_by_an_alias(self) -> None:
        remote = FakeRemote()
        remote.add_redirect(
            "streempilot/streempilot-flutter-app",
            "StreemPilot/sp-web-leptos",
            head="d" * 40,
            repository_id=1318677943,
        )
        with self.assertRaisesRegex(RemoteFleetStateError, "unexpected repository"):
            verify_created_repositories(
                [self.alias_record()],
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
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
