#!/usr/bin/env python3
"""Offline regression tests for the protected submodule App selector."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import bootstrap_k8s_submodule_app_secrets as subject


class MultiRepositoryValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.app_id = subject.protected.AppIdCandidate(
            value="12345",
            sources={"secret-manager:k8s-submodule-app"},
        )
        self.private_key = subject.protected.KeyCandidate(
            value="private-key-test-value",
            fingerprint="f" * 64,
            sources={"secret-manager:k8s-submodule-app"},
        )
        self.targets = subject.parse_target_repositories(
            [
                "ORESoftware/k8s-libs-and-shared-defs",
                "scintilla-run/gleam-lambda-runner",
                "scintilla-run/scintilla-run-monorepo",
            ]
        )
        self.installations = {
            "ORESoftware/k8s-libs-and-shared-defs": 101,
            "scintilla-run/gleam-lambda-runner": 202,
            "scintilla-run/scintilla-run-monorepo": 202,
        }
        self.token_repositories = {
            "token-101-k8s-libs-and-shared-defs": (
                "ORESoftware/k8s-libs-and-shared-defs"
            ),
            "token-202-gleam-lambda-runner": (
                "scintilla-run/gleam-lambda-runner"
            ),
            "token-202-scintilla-run-monorepo": (
                "scintilla-run/scintilla-run-monorepo"
            ),
        }
        self.revoked: list[str] = []

    def fake_request(
        self,
        method: str,
        path: str,
        token: str,
        payload: dict[str, object] | None = None,
    ) -> tuple[int, object]:
        if method == "GET" and path == "/app":
            self.assertEqual(token, "app-jwt")
            return 200, {
                "slug": "k8s-submodule-reader",
                "permissions": {"contents": "read", "metadata": "read"},
            }

        if method == "GET" and path.endswith("/installation"):
            full_name = path.removeprefix("/repos/").removesuffix("/installation")
            installation_id = self.installations.get(full_name)
            if installation_id is None:
                return 404, {"message": "not installed"}
            owner = full_name.split("/", 1)[0]
            return 200, {"id": installation_id, "account": {"login": owner}}

        if method == "POST" and "/access_tokens" in path:
            self.assertIsNotNone(payload)
            assert payload is not None
            repositories = payload.get("repositories")
            self.assertIsInstance(repositories, list)
            assert isinstance(repositories, list)
            self.assertEqual(len(repositories), 1)
            repository = str(repositories[0])
            installation_id = int(path.split("/")[3])
            issued = f"token-{installation_id}-{repository}"
            self.assertIn(issued, self.token_repositories)
            self.assertEqual(payload.get("permissions"), {"contents": "read"})
            return 201, {
                "token": issued,
                "expires_at": "2030-01-01T00:00:00Z",
                "permissions": {"contents": "read", "metadata": "read"},
            }

        if method == "GET" and path.startswith("/repos/"):
            expected = self.token_repositories.get(token)
            full_name = path.removeprefix("/repos/")
            if expected != full_name:
                return 404, {"message": "wrong repository token"}
            return 200, {"full_name": full_name, "private": True}

        if method == "GET" and path == "/installation/repositories?per_page=100":
            expected = self.token_repositories.get(token)
            if expected is None:
                return 401, {"message": "invalid token"}
            return 200, {
                "total_count": 1,
                "repositories": [{"full_name": expected}],
            }

        if method == "DELETE" and path == "/installation/token":
            self.revoked.append(token)
            return 204, None

        raise AssertionError(f"unexpected request: {method} {path} token={token}")

    def validate(self) -> subject.ValidatedPair | None:
        with tempfile.TemporaryDirectory() as temporary:
            with (
                patch.object(
                    subject.protected,
                    "mint_app_jwt",
                    return_value="app-jwt",
                ),
                patch.object(
                    subject.protected,
                    "request_json",
                    side_effect=self.fake_request,
                ),
            ):
                return subject.validate_pair(
                    self.app_id,
                    self.private_key,
                    self.targets,
                    Path(temporary),
                )

    def test_one_pair_must_validate_every_repository_and_revoke_every_token(self) -> None:
        validated = self.validate()
        self.assertIsNotNone(validated)
        assert validated is not None
        self.assertEqual(validated.app_slug, "k8s-submodule-reader")
        self.assertEqual(
            tuple(repository.full_name for repository in validated.repositories),
            tuple(target.full_name for target in self.targets),
        )
        self.assertCountEqual(self.revoked, self.token_repositories)
        self.assertEqual(len(self.revoked), len(self.targets))

    def test_missing_cross_org_installation_fails_closed_after_revocation(self) -> None:
        self.installations.pop("scintilla-run/gleam-lambda-runner")
        validated = self.validate()
        self.assertIsNone(validated)
        self.assertEqual(
            self.revoked,
            ["token-101-k8s-libs-and-shared-defs"],
        )

    def test_duplicate_repository_names_are_rejected_case_insensitively(self) -> None:
        with self.assertRaisesRegex(ValueError, "duplicate target repository"):
            subject.parse_target_repositories(
                ["scintilla-run/gleam-lambda-runner", "SCINTILLA-RUN/GLEAM-LAMBDA-RUNNER"]
            )


if __name__ == "__main__":
    unittest.main()
