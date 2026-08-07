#!/usr/bin/env python3
from __future__ import annotations

from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest

from repository_fleet_aliases import (
    RepositoryAliasError,
    load_repository_aliases,
    validate_repository_alias_payload,
)


SOURCE_REPOSITORY = "ORESoftware/ai-agent-coordinator.rs"
SOURCE_SHA = "5d9a0c2cb44dff607bc3953954ce4b9af08e5789"
SEALED = [
    "streempilot/streempilot-flutter-app",
    "streempilot/streempilot-infra",
    "hypesiege/hypesiege-analytics.rs",
]


def valid_payload() -> dict[str, object]:
    return {
        "schema_version": 1,
        "source_repository": SOURCE_REPOSITORY,
        "source_sha": SOURCE_SHA,
        "aliases": [
            {
                "sealed_full_name": "streempilot/streempilot-flutter-app",
                "remote_full_name": "StreemPilot/sp-web-leptos",
                "repository_id": 1318677943,
            },
            {
                "sealed_full_name": "streempilot/streempilot-infra",
                "remote_full_name": "StreemPilot/sp-infra",
                "repository_id": 1318678140,
            },
        ],
    }


class RepositoryFleetAliasTests(unittest.TestCase):
    def validate(self, payload: object):
        return validate_repository_alias_payload(
            payload,
            sealed_full_names=SEALED,
            expected_source_repository=SOURCE_REPOSITORY,
            expected_source_sha=SOURCE_SHA,
        )

    def test_valid_aliases_are_keyed_by_sealed_casefold_identity(self) -> None:
        aliases = self.validate(valid_payload())
        self.assertEqual(
            aliases["streempilot/streempilot-flutter-app"].remote_full_name,
            "StreemPilot/sp-web-leptos",
        )
        self.assertEqual(
            aliases["streempilot/streempilot-infra"].repository_id,
            1318678140,
        )

    def test_source_repository_and_commit_are_immutable(self) -> None:
        for field, bad_value, expected in (
            ("source_repository", "ORESoftware/other", "source repository changed"),
            ("source_sha", "f" * 40, "source commit changed"),
            ("schema_version", 2, "schema version 1"),
        ):
            with self.subTest(field=field):
                payload = valid_payload()
                payload[field] = bad_value
                with self.assertRaisesRegex(RepositoryAliasError, expected):
                    self.validate(payload)

    def test_alias_source_must_exist_with_exact_sealed_casing(self) -> None:
        payload = valid_payload()
        aliases = payload["aliases"]
        assert isinstance(aliases, list)
        first = aliases[0]
        assert isinstance(first, dict)
        first["sealed_full_name"] = "streempilot/not-sealed"
        with self.assertRaisesRegex(RepositoryAliasError, "not in the sealed fleet"):
            self.validate(payload)

        payload = valid_payload()
        aliases = payload["aliases"]
        assert isinstance(aliases, list)
        first = aliases[0]
        assert isinstance(first, dict)
        first["sealed_full_name"] = "StreemPilot/streempilot-flutter-app"
        with self.assertRaisesRegex(RepositoryAliasError, "casing differs"):
            self.validate(payload)

    def test_alias_target_cannot_collide_with_sealed_identity(self) -> None:
        payload = valid_payload()
        aliases = payload["aliases"]
        assert isinstance(aliases, list)
        first = aliases[0]
        assert isinstance(first, dict)
        first["remote_full_name"] = "hypesiege/hypesiege-analytics.rs"
        with self.assertRaisesRegex(RepositoryAliasError, "collides with a sealed"):
            self.validate(payload)

    def test_duplicate_source_target_and_repository_id_fail_closed(self) -> None:
        payload = valid_payload()
        aliases = payload["aliases"]
        assert isinstance(aliases, list)
        aliases.append(deepcopy(aliases[0]))
        with self.assertRaisesRegex(RepositoryAliasError, "duplicate alias source"):
            self.validate(payload)

        payload = valid_payload()
        aliases = payload["aliases"]
        assert isinstance(aliases, list)
        second = aliases[1]
        first = aliases[0]
        assert isinstance(second, dict) and isinstance(first, dict)
        second["remote_full_name"] = first["remote_full_name"]
        with self.assertRaisesRegex(RepositoryAliasError, "duplicate alias target"):
            self.validate(payload)

        payload = valid_payload()
        aliases = payload["aliases"]
        assert isinstance(aliases, list)
        second = aliases[1]
        first = aliases[0]
        assert isinstance(second, dict) and isinstance(first, dict)
        second["repository_id"] = first["repository_id"]
        with self.assertRaisesRegex(RepositoryAliasError, "duplicate alias repository ID"):
            self.validate(payload)

    def test_alias_objects_are_exact_and_repository_ids_are_positive_integers(self) -> None:
        payload = valid_payload()
        aliases = payload["aliases"]
        assert isinstance(aliases, list)
        first = aliases[0]
        assert isinstance(first, dict)
        first["reason"] = "unreviewed extra field"
        with self.assertRaisesRegex(RepositoryAliasError, "contain exactly"):
            self.validate(payload)

        for repository_id in (0, -1, True, "1318677943"):
            with self.subTest(repository_id=repository_id):
                payload = valid_payload()
                aliases = payload["aliases"]
                assert isinstance(aliases, list)
                first = aliases[0]
                assert isinstance(first, dict)
                first["repository_id"] = repository_id
                with self.assertRaisesRegex(RepositoryAliasError, "invalid repository_id"):
                    self.validate(payload)

    def test_loader_rejects_missing_and_malformed_json(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            missing = root / "missing.json"
            with self.assertRaisesRegex(RepositoryAliasError, "ledger is missing"):
                load_repository_aliases(
                    missing,
                    sealed_full_names=SEALED,
                    expected_source_repository=SOURCE_REPOSITORY,
                    expected_source_sha=SOURCE_SHA,
                )

            malformed = root / "malformed.json"
            malformed.write_text("{not-json", encoding="utf-8")
            with self.assertRaisesRegex(RepositoryAliasError, "not valid JSON"):
                load_repository_aliases(
                    malformed,
                    sealed_full_names=SEALED,
                    expected_source_repository=SOURCE_REPOSITORY,
                    expected_source_sha=SOURCE_SHA,
                )

            valid = root / "valid.json"
            valid.write_text(json.dumps(valid_payload()), encoding="utf-8")
            aliases = load_repository_aliases(
                valid,
                sealed_full_names=SEALED,
                expected_source_repository=SOURCE_REPOSITORY,
                expected_source_sha=SOURCE_SHA,
            )
            self.assertEqual(len(aliases), 2)


if __name__ == "__main__":
    unittest.main()
