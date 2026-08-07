#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import importlib.util
from pathlib import Path
import unittest
from unittest import mock
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SELECTOR_PATH = ROOT / "scripts" / "ops" / "select_org_dotgithub_owner_token.py"
SPEC = importlib.util.spec_from_file_location("org_dotgithub_owner_token_selector", SELECTOR_PATH)
assert SPEC is not None and SPEC.loader is not None
selector = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(selector)


class OrgDotgithubOwnerTokenSelectorTests(unittest.TestCase):
    def test_fleet_is_exactly_36_unique_organizations(self) -> None:
        self.assertEqual(36, len(selector.ORGANIZATIONS))
        self.assertEqual(36, len({name.lower() for name in selector.ORGANIZATIONS}))
        self.assertIn("fiducia-cloud", selector.ORGANIZATIONS)
        self.assertIn("sonus-auris", selector.ORGANIZATIONS)
        self.assertIn("shared-auth", selector.ORGANIZATIONS)
        self.assertIn("StreemPilot", selector.ORGANIZATIONS)

    def test_recursively_selects_nested_owner_token(self) -> None:
        token = "github_pat_nested_owner_token_1234567890"
        payload = {"service": {"credentials": [{"github": {"GH_PAT": token}}]}}
        calls: list[tuple[str, str]] = []

        def requester(candidate: str, path: str) -> tuple[int, Any]:
            calls.append((candidate, path))
            if path == "/user":
                return 200, {"login": "ORESoftware"}
            return 200, {"role": "admin", "state": "active"}

        field, selected = selector.select_owner_admin_token(payload, requester=requester)
        self.assertEqual("service.credentials.0.github.GH_PAT", field)
        self.assertEqual(token, selected)
        self.assertEqual(37, len(calls))
        self.assertTrue(all(candidate == token for candidate, _path in calls))

    def test_prefers_gh_pat_over_generic_nested_token(self) -> None:
        payload = {
            "backup": {"github_token": "github_pat_backup_1234567890"},
            "primary": {"GH_PAT": "github_pat_primary_1234567890"},
        }

        def requester(candidate: str, path: str) -> tuple[int, Any]:
            if path == "/user":
                return 200, {"login": "ORESoftware"}
            return 200, {"role": "admin", "state": "active"}

        field, selected = selector.select_owner_admin_token(payload, requester=requester)
        self.assertEqual("primary.GH_PAT", field)
        self.assertEqual("github_pat_primary_1234567890", selected)

    def test_rejects_wrong_identity_without_leaking_token(self) -> None:
        token = "github_pat_wrong_identity_secret_1234567890"

        def requester(_candidate: str, _path: str) -> tuple[int, Any]:
            return 200, {"login": "someone-else"}

        with self.assertRaises(selector.CredentialSelectionError) as context:
            selector.select_owner_admin_token({"GH_PAT": token}, requester=requester)
        self.assertNotIn(token, str(context.exception))

    def test_rejects_candidate_missing_one_admin_membership(self) -> None:
        token = "github_pat_partial_owner_secret_1234567890"
        denied_org = selector.ORGANIZATIONS[-1]

        def requester(_candidate: str, path: str) -> tuple[int, Any]:
            if path == "/user":
                return 200, {"login": "ORESoftware"}
            if path.endswith("/" + denied_org):
                return 200, {"role": "member", "state": "active"}
            return 200, {"role": "admin", "state": "active"}

        with self.assertRaises(selector.CredentialSelectionError):
            selector.select_owner_admin_token({"GH_PAT": token}, requester=requester)

    def test_ignores_whitespace_and_unrelated_strings(self) -> None:
        payload = {
            "password": "not-a-github-token",
            "github": {"token": "contains whitespace"},
            "notes": ["github_pat_not_named_as_a_field_1234567890"],
        }
        self.assertEqual([], list(selector.iter_candidates(payload)))

    def test_rejects_explicitly_revoked_credential_fingerprint(self) -> None:
        token = "github_pat_fixture_revoked_1234567890"
        fingerprint = hashlib.sha256(token.encode("utf-8")).hexdigest()
        with mock.patch.object(selector, "REJECTED_TOKEN_SHA256", frozenset({fingerprint})):
            self.assertFalse(selector._valid_token(token))
            self.assertEqual([], list(selector.iter_candidates({"GH_PAT": token})))

    def test_duplicate_token_is_validated_only_once(self) -> None:
        token = "github_pat_duplicate_secret_1234567890"
        user_calls = 0

        def requester(_candidate: str, path: str) -> tuple[int, Any]:
            nonlocal user_calls
            if path == "/user":
                user_calls += 1
                return 200, {"login": "wrong"}
            raise AssertionError("membership must not be queried for the wrong identity")

        with self.assertRaises(selector.CredentialSelectionError):
            selector.select_owner_admin_token(
                {"GH_PAT": token, "nested": {"github_token": token}},
                requester=requester,
            )
        self.assertEqual(1, user_calls)


if __name__ == "__main__":
    unittest.main()
