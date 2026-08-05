#!/usr/bin/env python3
from __future__ import annotations

from collections.abc import Iterable
import unittest

from private_repository_creation import ensure_private_repository
from repository_fleet_remote_state import (
    RemoteFleetStateError,
    classify_remote_fleet,
    verify_preserved_existing,
)

ApiResponse = tuple[int, object | None]
ApiOutcome = ApiResponse | Exception
ApiCall = tuple[str, str, dict[str, object] | None]


class ScriptedApi:
    def __init__(self, script: Iterable[tuple[ApiCall, ApiOutcome]]) -> None:
        self.script = list(script)

    def __call__(self, method: str, path: str, body: dict[str, object] | None) -> ApiResponse:
        if not self.script:
            raise AssertionError(f"unexpected API call: {(method, path, body)!r}")
        expected, outcome = self.script.pop(0)
        self.assert_call(expected, (method, path, body))
        if isinstance(outcome, Exception):
            raise outcome
        return outcome

    @staticmethod
    def assert_call(expected: ApiCall, actual: ApiCall) -> None:
        if expected != actual:
            raise AssertionError(f"expected API call {expected!r}, got {actual!r}")

    def assert_exhausted(self) -> None:
        if self.script:
            raise AssertionError(f"unconsumed API script: {self.script!r}")


class RedirectRemote:
    def __init__(self) -> None:
        self.repositories: dict[str, dict[str, object]] = {}
        self.heads: dict[str, str] = {}

    def add_redirect(
        self,
        requested: str,
        actual: str,
        *,
        head: str,
        repository_id: int = 88,
        visibility: str = "private",
    ) -> None:
        payload = {
            "id": repository_id,
            "full_name": actual,
            "private": visibility == "private",
            "visibility": visibility,
            "default_branch": "main",
            "archived": False,
            "disabled": False,
        }
        self.repositories[requested.casefold()] = payload
        self.repositories[actual.casefold()] = payload
        self.heads[actual.casefold()] = head

    def lookup(self, full_name: str) -> tuple[int, dict[str, object] | None]:
        payload = self.repositories.get(full_name.casefold())
        return (200, payload) if payload is not None else (404, None)

    def main_ref(self, full_name: str) -> str | None:
        return self.heads.get(full_name.casefold())


def record(full_name: str, commit: str = "a" * 40) -> dict[str, object]:
    return {"full_name": full_name, "commit": commit, "kind": "application"}


def private_repository(full_name: str, *, visibility: str = "private") -> dict[str, object]:
    return {
        "full_name": full_name,
        "private": visibility == "private",
        "visibility": visibility,
    }


def create_payload() -> dict[str, object]:
    return {
        "name": "streempilot-flutter-app",
        "description": "Canonical application",
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


class RepositoryRedirectAliasTests(unittest.TestCase):
    requested = "streempilot/streempilot-flutter-app"
    actual = "StreemPilot/sp-web-leptos"

    def classify(self, remote: RedirectRemote, records: list[dict[str, object]]):
        return classify_remote_fleet(
            records,
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
        )

    def test_same_owner_redirect_is_preserved_and_canonical_identity_stays_missing(self) -> None:
        remote = RedirectRemote()
        remote.add_redirect(self.requested, self.actual, head="d" * 40)
        missing, existing = self.classify(remote, [record(self.requested)])
        self.assertEqual([item["full_name"] for item in missing], [self.requested])
        self.assertEqual(set(existing), {self.actual})
        self.assertEqual(existing[self.actual]["repository_id"], 88)
        self.assertEqual(existing[self.actual]["head"], "d" * 40)
        self.assertEqual(existing[self.actual]["redirect_alias_for"], self.requested)
        verify_preserved_existing(
            existing,
            repository_lookup=remote.lookup,
            main_ref_lookup=remote.main_ref,
        )

    def test_redirect_target_head_or_id_change_is_detected(self) -> None:
        remote = RedirectRemote()
        remote.add_redirect(self.requested, self.actual, head="d" * 40)
        _, existing = self.classify(remote, [record(self.requested)])
        remote.heads[self.actual.casefold()] = "e" * 40
        with self.assertRaisesRegex(RemoteFleetStateError, "changed during"):
            verify_preserved_existing(
                existing,
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
            )
        remote.heads[self.actual.casefold()] = "d" * 40
        remote.repositories[self.actual.casefold()]["id"] = 99
        with self.assertRaisesRegex(RemoteFleetStateError, "identity changed"):
            verify_preserved_existing(
                existing,
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
            )

    def test_cross_owner_redirect_fails_closed(self) -> None:
        remote = RedirectRemote()
        remote.add_redirect(self.requested, "other-owner/sp-web-leptos", head="d" * 40)
        with self.assertRaisesRegex(RemoteFleetStateError, "unexpected repository"):
            self.classify(remote, [record(self.requested)])

    def test_redirect_to_another_canonical_identity_fails_closed(self) -> None:
        remote = RedirectRemote()
        remote.add_redirect(self.requested, self.actual, head="d" * 40)
        with self.assertRaisesRegex(RemoteFleetStateError, "another canonical identity"):
            self.classify(
                remote,
                [record(self.requested), record(self.actual, "b" * 40)],
            )

    def test_multiple_canonical_names_cannot_share_one_redirect_target(self) -> None:
        remote = RedirectRemote()
        remote.add_redirect(self.requested, self.actual, head="d" * 40)
        second = "streempilot/streempilot-desktop-app"
        remote.repositories[second.casefold()] = remote.repositories[self.actual.casefold()]
        with self.assertRaisesRegex(RemoteFleetStateError, "multiple canonical identities"):
            self.classify(remote, [record(self.requested), record(second, "b" * 40)])

    def test_create_helper_preserves_alias_then_creates_canonical_repository(self) -> None:
        repository_path = f"/repos/{self.requested}"
        api = ScriptedApi(
            [
                (("GET", repository_path, None), (200, private_repository(self.actual))),
                (
                    ("POST", "/orgs/streempilot/repos", create_payload()),
                    (201, private_repository("StreemPilot/streempilot-flutter-app")),
                ),
            ]
        )
        emitted: list[str] = []
        ensure_private_repository(
            api,
            "streempilot",
            "streempilot-flutter-app",
            "Canonical application",
            emit=emitted.append,
        )
        api.assert_exhausted()
        self.assertEqual(
            emitted,
            [
                f"PRESERVED_REDIRECT_ALIAS {self.requested} -> {self.actual}",
                f"CREATED_PRIVATE {self.requested}",
            ],
        )

    def test_create_helper_rejects_cross_owner_and_public_redirects(self) -> None:
        repository_path = f"/repos/{self.requested}"
        cases = (
            (private_repository("other-owner/sp-web-leptos"), "identity mismatch"),
            (private_repository(self.actual, visibility="public"), "visibility mismatch"),
        )
        for payload, error in cases:
            with self.subTest(error=error):
                api = ScriptedApi([(("GET", repository_path, None), (200, payload))])
                with self.assertRaisesRegex(RuntimeError, error):
                    ensure_private_repository(
                        api,
                        "streempilot",
                        "streempilot-flutter-app",
                        "Canonical application",
                    )
                api.assert_exhausted()

    def test_create_conflict_does_not_reconcile_to_redirect_target(self) -> None:
        repository_path = f"/repos/{self.requested}"
        redirect = private_repository(self.actual)
        api = ScriptedApi(
            [
                (("GET", repository_path, None), (200, redirect)),
                (("POST", "/orgs/streempilot/repos", create_payload()), (422, {})),
                (("GET", repository_path, None), (200, redirect)),
            ]
        )
        with self.assertRaisesRegex(RuntimeError, "identity mismatch"):
            ensure_private_repository(
                api,
                "streempilot",
                "streempilot-flutter-app",
                "Canonical application",
            )
        api.assert_exhausted()


if __name__ == "__main__":
    unittest.main()
