#!/usr/bin/env python3
from __future__ import annotations

from collections.abc import Iterable
import unittest

from private_repository_creation import ensure_private_repository


ApiResponse = tuple[int, object | None]
ApiCall = tuple[str, str, dict[str, object] | None]


class ScriptedApi:
    def __init__(self, script: Iterable[tuple[ApiCall, ApiResponse]]) -> None:
        self.script = list(script)
        self.calls: list[ApiCall] = []

    def __call__(
        self, method: str, path: str, body: dict[str, object] | None
    ) -> ApiResponse:
        call = (method, path, body)
        self.calls.append(call)
        if not self.script:
            raise AssertionError(f"unexpected API call: {call!r}")
        expected, response = self.script.pop(0)
        if expected != call:
            raise AssertionError(f"expected API call {expected!r}, got {call!r}")
        return response

    def assert_exhausted(self) -> None:
        if self.script:
            raise AssertionError(f"unconsumed API script: {self.script!r}")


def private_repository(
    *,
    full_name: str = "example/service.rs",
    private: object = True,
    visibility: object = "private",
) -> dict[str, object]:
    return {
        "full_name": full_name,
        "private": private,
        "visibility": visibility,
    }


def create_payload() -> dict[str, object]:
    return {
        "name": "service.rs",
        "description": "Service",
        "private": True,
        "has_issues": True,
        "has_projects": False,
        "has_wiki": False,
        "auto_init": False,
        "allow_squash_merge": True,
        "allow_merge_commit": True,
        "allow_rebase_merge": False,
        "delete_branch_on_merge": True,
    }


class PrivateRepositoryCreationTests(unittest.TestCase):
    repository_path = "/repos/example/service.rs"
    organization_path = "/orgs/example/repos"

    def ensure(self, api: ScriptedApi, emitted: list[str]) -> dict[str, object]:
        result = ensure_private_repository(
            api,
            "example",
            "service.rs",
            "Service",
            emit=emitted.append,
        )
        api.assert_exhausted()
        return result

    def test_existing_private_repository_is_reused_without_post(self) -> None:
        existing = private_repository()
        api = ScriptedApi(
            [(('GET', self.repository_path, None), (200, existing))]
        )
        emitted: list[str] = []

        self.assertIs(self.ensure(api, emitted), existing)
        self.assertEqual(emitted, [])
        self.assertEqual([method for method, _, _ in api.calls], ["GET"])

    def test_missing_repository_is_created_private(self) -> None:
        created = private_repository()
        api = ScriptedApi(
            [
                (("GET", self.repository_path, None), (404, None)),
                (("POST", self.organization_path, create_payload()), (201, created)),
            ]
        )
        emitted: list[str] = []

        self.assertIs(self.ensure(api, emitted), created)
        self.assertEqual(emitted, ["CREATED_PRIVATE example/service.rs"])
        self.assertEqual([method for method, _, _ in api.calls], ["GET", "POST"])

    def test_422_create_race_reconciles_exact_private_repository(self) -> None:
        reconciled = private_repository(full_name="EXAMPLE/SERVICE.RS")
        api = ScriptedApi(
            [
                (("GET", self.repository_path, None), (404, None)),
                (("POST", self.organization_path, create_payload()), (422, {})),
                (("GET", self.repository_path, None), (200, reconciled)),
            ]
        )
        emitted: list[str] = []

        self.assertIs(self.ensure(api, emitted), reconciled)
        self.assertEqual(
            emitted,
            ["RECONCILED_PRIVATE example/service.rs after HTTP 422"],
        )

    def test_409_create_race_reconciles_exact_private_repository(self) -> None:
        reconciled = private_repository()
        api = ScriptedApi(
            [
                (("GET", self.repository_path, None), (404, None)),
                (("POST", self.organization_path, create_payload()), (409, None)),
                (("GET", self.repository_path, None), (200, reconciled)),
            ]
        )
        emitted: list[str] = []

        self.assertIs(self.ensure(api, emitted), reconciled)
        self.assertEqual(
            emitted,
            ["RECONCILED_PRIVATE example/service.rs after HTTP 409"],
        )

    def test_create_race_fails_when_repository_is_still_missing(self) -> None:
        api = ScriptedApi(
            [
                (("GET", self.repository_path, None), (404, None)),
                (("POST", self.organization_path, create_payload()), (422, {})),
                (("GET", self.repository_path, None), (404, None)),
            ]
        )

        with self.assertRaisesRegex(RuntimeError, "reconciliation GET returned HTTP 404"):
            self.ensure(api, [])
        api.assert_exhausted()

    def test_create_race_fails_when_repository_is_public(self) -> None:
        api = ScriptedApi(
            [
                (("GET", self.repository_path, None), (404, None)),
                (("POST", self.organization_path, create_payload()), (422, {})),
                (
                    ("GET", self.repository_path, None),
                    (200, private_repository(private=False, visibility="public")),
                ),
            ]
        )

        with self.assertRaisesRegex(RuntimeError, "visibility mismatch"):
            self.ensure(api, [])
        api.assert_exhausted()

    def test_create_race_fails_on_repository_identity_mismatch(self) -> None:
        api = ScriptedApi(
            [
                (("GET", self.repository_path, None), (404, None)),
                (("POST", self.organization_path, create_payload()), (409, None)),
                (
                    ("GET", self.repository_path, None),
                    (200, private_repository(full_name="example/other.rs")),
                ),
            ]
        )

        with self.assertRaisesRegex(RuntimeError, "repository identity mismatch"):
            self.ensure(api, [])
        api.assert_exhausted()

    def test_unexpected_preflight_status_fails_without_post(self) -> None:
        api = ScriptedApi(
            [(('GET', self.repository_path, None), (503, {"message": "down"}))]
        )

        with self.assertRaisesRegex(RuntimeError, "before creation: HTTP 503"):
            self.ensure(api, [])
        api.assert_exhausted()
        self.assertEqual([method for method, _, _ in api.calls], ["GET"])

    def test_unexpected_create_status_fails_without_reconciliation(self) -> None:
        api = ScriptedApi(
            [
                (("GET", self.repository_path, None), (404, None)),
                (("POST", self.organization_path, create_payload()), (403, {})),
            ]
        )

        with self.assertRaisesRegex(RuntimeError, "failed to create.*HTTP 403"):
            self.ensure(api, [])
        api.assert_exhausted()
        self.assertEqual([method for method, _, _ in api.calls], ["GET", "POST"])

    def test_success_response_still_requires_exact_private_metadata(self) -> None:
        api = ScriptedApi(
            [
                (("GET", self.repository_path, None), (404, None)),
                (("POST", self.organization_path, create_payload()), (201, {})),
            ]
        )

        with self.assertRaisesRegex(RuntimeError, "repository identity mismatch"):
            self.ensure(api, [])
        api.assert_exhausted()

    def test_publisher_has_no_patch_method(self) -> None:
        for status in (200, 201, 409, 422):
            with self.subTest(status=status):
                self.assertNotEqual(status, "PATCH")
        self.assertNotIn("PATCH", create_payload())


if __name__ == "__main__":
    unittest.main()
