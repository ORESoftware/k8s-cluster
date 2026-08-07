#!/usr/bin/env python3
"""Unit tests for the bounded Canonical Docs repository publisher."""

from __future__ import annotations

import importlib.util
import os
import unittest
from pathlib import Path
from unittest.mock import patch

MODULE_PATH = Path(__file__).with_name("publish_canonical_docs.py")
SPEC = importlib.util.spec_from_file_location("publish_canonical_docs", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {MODULE_PATH}")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class CanonicalDocsPublisherTests(unittest.TestCase):
    def repository(self, **overrides: object) -> dict[str, object]:
        value: dict[str, object] = {
            "full_name": MODULE.TARGET_REPOSITORY,
            "owner": {"login": MODULE.TARGET_OWNER},
            "visibility": "public",
            "private": False,
            "archived": False,
            "disabled": False,
            "default_branch": MODULE.MAIN_REF,
            "allow_rebase_merge": False,
            "delete_branch_on_merge": True,
        }
        value.update(overrides)
        return value

    def test_repository_payload_is_public_and_review_oriented(self) -> None:
        payload = MODULE.repository_payload()
        self.assertEqual(payload["name"], "canonical-docs")
        self.assertIs(payload["private"], False)
        self.assertIs(payload["auto_init"], False)
        self.assertIs(payload["has_issues"], True)
        self.assertIs(payload["has_projects"], False)
        self.assertIs(payload["has_wiki"], False)
        self.assertIs(payload["allow_squash_merge"], True)
        self.assertIs(payload["allow_rebase_merge"], False)
        self.assertIs(payload["delete_branch_on_merge"], True)

    def test_token_shape_fails_closed(self) -> None:
        for value in (None, "", "token", "ghp_has whitespace"):
            environment = {} if value is None else {"GH_TOKEN": value}
            with self.subTest(value=value), patch.dict(os.environ, environment, clear=True):
                with self.assertRaises(MODULE.PublishError):
                    MODULE.token_from_environment()

        with patch.dict(os.environ, {"GH_TOKEN": "ghp_" + "x" * 36}, clear=True):
            self.assertTrue(MODULE.token_from_environment().startswith("ghp_"))
        with patch.dict(
            os.environ,
            {"GITHUB_REPOSITORY_ADMIN_TOKEN": "github_pat_" + "x" * 40},
            clear=True,
        ):
            self.assertTrue(MODULE.token_from_environment().startswith("github_pat_"))

    def test_repository_validation_rejects_identity_visibility_and_lifecycle_drift(self) -> None:
        MODULE.validate_repository(self.repository())
        bad_values = (
            self.repository(full_name="other/repository"),
            self.repository(owner={"login": "other"}),
            self.repository(visibility="private", private=True),
            self.repository(archived=True),
            self.repository(disabled=True),
        )
        for value in bad_values:
            with self.subTest(value=value), self.assertRaises(MODULE.PublishError):
                MODULE.validate_repository(value)

    def test_existing_conflicting_ref_is_never_overwritten(self) -> None:
        with patch.object(MODULE, "read_ref", return_value="0" * 40):
            with self.assertRaisesRegex(MODULE.PublishError, "refusing to overwrite"):
                MODULE.ensure_ref(
                    "token", Path("source.git"), {}, MODULE.MAIN_REF, MODULE.MAIN_SHA
                )

    def test_pull_request_creation_is_exact_and_evidence_bounded(self) -> None:
        calls: list[tuple[str, str, object]] = []

        def fake_api(
            token: str,
            method: str,
            path: str,
            body: object = None,
            **_: object,
        ):
            calls.append((method, path, body))
            if method == "GET":
                return 200, []
            return 201, {
                "number": 1,
                "html_url": "https://github.com/canonical-cloud/canonical-docs/pull/1",
                "state": "open",
                "merged_at": None,
                "head": {"ref": MODULE.FEATURE_REF},
                "base": {"ref": MODULE.MAIN_REF},
            }

        with patch.object(MODULE, "api", side_effect=fake_api):
            pull = MODULE.ensure_pull_request("token")
        self.assertEqual(pull["number"], 1)
        create_body = calls[-1][2]
        self.assertIsInstance(create_body, dict)
        assert isinstance(create_body, dict)
        self.assertEqual(create_body["title"], MODULE.PR_TITLE)
        self.assertEqual(create_body["head"], MODULE.FEATURE_REF)
        self.assertEqual(create_body["base"], MODULE.MAIN_REF)
        self.assertIs(create_body["draft"], False)
        body = str(create_body["body"])
        self.assertIn("do not claim", body)
        self.assertIn("DEN-1049", body)
        self.assertIn("scripts/check_docs.py", body)

    def test_ref_path_preserves_nested_feature_branch(self) -> None:
        self.assertTrue(MODULE.ref_path(MODULE.FEATURE_REF).endswith(MODULE.FEATURE_REF))


if __name__ == "__main__":
    unittest.main()
