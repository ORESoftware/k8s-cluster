#!/usr/bin/env python3
from __future__ import annotations

from collections.abc import Iterable
import unittest

from repository_rename_alias_guard import (
    RedirectTargetSnapshot,
    RepositoryRenameAliasError,
    RepositoryRenameAliasGuard,
)


ApiResponse = tuple[int, object | None]
ApiOutcome = ApiResponse | Exception
ApiCall = tuple[str, str, dict[str, object] | None]
ProbeOutcome = tuple[int, str | None] | Exception
RefOutcome = str | None | Exception


class ScriptedApi:
    def __init__(self, script: Iterable[tuple[ApiCall, ApiOutcome]]) -> None:
        self.script = list(script)
        self.calls: list[ApiCall] = []

    def __call__(
        self, method: str, path: str, body: dict[str, object] | None
    ) -> ApiResponse:
        call = (method, path, body)
        self.calls.append(call)
        if not self.script:
            raise AssertionError(f"unexpected API call: {call!r}")
        expected, outcome = self.script.pop(0)
        if expected != call:
            raise AssertionError(f"expected API call {expected!r}, got {call!r}")
        if isinstance(outcome, Exception):
            raise outcome
        return outcome

    def assert_exhausted(self) -> None:
        if self.script:
            raise AssertionError(f"unconsumed API script: {self.script!r}")


class ScriptedProbe:
    def __init__(self, script: Iterable[tuple[str, ProbeOutcome]]) -> None:
        self.script = list(script)
        self.calls: list[str] = []

    def __call__(self, full_name: str) -> tuple[int, str | None]:
        self.calls.append(full_name)
        if not self.script:
            raise AssertionError(f"unexpected redirect probe: {full_name!r}")
        expected, outcome = self.script.pop(0)
        if expected.casefold() != full_name.casefold():
            raise AssertionError(
                f"expected redirect probe {expected!r}, got {full_name!r}"
            )
        if isinstance(outcome, Exception):
            raise outcome
        return outcome

    def assert_exhausted(self) -> None:
        if self.script:
            raise AssertionError(f"unconsumed redirect probes: {self.script!r}")


class ScriptedMainRefs:
    def __init__(self, script: Iterable[tuple[str, RefOutcome]]) -> None:
        self.script = list(script)
        self.calls: list[str] = []

    def __call__(self, full_name: str) -> str | None:
        self.calls.append(full_name)
        if not self.script:
            raise AssertionError(f"unexpected main-ref lookup: {full_name!r}")
        expected, outcome = self.script.pop(0)
        if expected.casefold() != full_name.casefold():
            raise AssertionError(
                f"expected main-ref lookup {expected!r}, got {full_name!r}"
            )
        if isinstance(outcome, Exception):
            raise outcome
        return outcome

    def assert_exhausted(self) -> None:
        if self.script:
            raise AssertionError(f"unconsumed main-ref lookups: {self.script!r}")


def private_repository(
    full_name: str,
    *,
    repository_id: int = 88,
    visibility: str = "private",
    default_branch: str = "main",
) -> dict[str, object]:
    return {
        "id": repository_id,
        "full_name": full_name,
        "private": visibility == "private",
        "visibility": visibility,
        "default_branch": default_branch,
        "archived": False,
        "disabled": False,
    }


class RepositoryRenameAliasGuardTests(unittest.TestCase):
    requested = "streempilot/streempilot-flutter-app"
    target = "StreemPilot/sp-web-leptos"
    stable_head = "d" * 40

    def make_guard(
        self,
        *,
        api: ScriptedApi,
        probe: ScriptedProbe,
        refs: ScriptedMainRefs,
        canonical: set[str] | None = None,
        emitted: list[str] | None = None,
    ) -> RepositoryRenameAliasGuard:
        canonical_names = canonical or {
            self.requested,
            "hypesiege/hypesiege-api-server.rs",
        }
        return RepositoryRenameAliasGuard(
            api_base="https://api.github.com",
            token="unit-test-token",
            api=api,
            main_ref_lookup=refs,
            canonical_full_names=canonical_names,
            redirect_probe=probe,
            emit=(emitted if emitted is not None else []).append,
        )

    def test_exact_private_repository_passes_through_without_redirect_probe(self) -> None:
        payload = private_repository(self.requested)
        api = ScriptedApi(
            [(("GET", f"/repos/{self.requested}", None), (200, payload))]
        )
        probe = ScriptedProbe([])
        refs = ScriptedMainRefs([])
        guard = self.make_guard(api=api, probe=probe, refs=refs)

        status, result = guard.repository_lookup(self.requested)

        self.assertEqual(status, 200)
        self.assertIs(result, payload)
        self.assertEqual(guard.snapshots, ())
        api.assert_exhausted()
        probe.assert_exhausted()
        refs.assert_exhausted()

    def test_same_owner_redirect_is_exposed_as_missing_and_target_is_preserved(self) -> None:
        payload = private_repository(self.target)
        api = ScriptedApi(
            [
                (("GET", f"/repos/{self.requested}", None), (200, payload)),
                (("GET", f"/repos/{self.target}", None), (200, payload)),
            ]
        )
        probe = ScriptedProbe(
            [(self.requested, (301, "/repositories/88"))]
        )
        refs = ScriptedMainRefs(
            [
                (self.target, self.stable_head),
                (self.target, self.stable_head),
            ]
        )
        emitted: list[str] = []
        guard = self.make_guard(
            api=api,
            probe=probe,
            refs=refs,
            emitted=emitted,
        )

        status, result = guard.repository_lookup(self.requested)
        self.assertEqual((status, result), (404, None))
        self.assertEqual(
            guard.snapshots,
            (
                RedirectTargetSnapshot(
                    requested_full_name=self.requested,
                    target_full_name=self.target,
                    target_repository_id=88,
                    target_head=self.stable_head,
                ),
            ),
        )

        guard.verify_preserved()

        self.assertEqual(
            emitted,
            [
                "PRESERVE_RENAMED_TARGET "
                f"{self.requested} -> {self.target} id=88 head={self.stable_head}",
                "VERIFIED_PRESERVED_RENAMED_TARGET "
                f"{self.target} {self.stable_head}",
            ],
        )
        api.assert_exhausted()
        probe.assert_exhausted()
        refs.assert_exhausted()

    def test_cross_owner_redirect_fails_closed(self) -> None:
        other_target = "other-owner/sp-web-leptos"
        api = ScriptedApi(
            [
                (
                    ("GET", f"/repos/{self.requested}", None),
                    (200, private_repository(other_target)),
                )
            ]
        )
        probe = ScriptedProbe([])
        refs = ScriptedMainRefs([])
        guard = self.make_guard(api=api, probe=probe, refs=refs)

        with self.assertRaisesRegex(
            RepositoryRenameAliasError, "redirect target owner mismatch"
        ):
            guard.repository_lookup(self.requested)

        api.assert_exhausted()
        probe.assert_exhausted()
        refs.assert_exhausted()

    def test_redirect_to_another_canonical_identity_fails_closed(self) -> None:
        api = ScriptedApi(
            [
                (
                    ("GET", f"/repos/{self.requested}", None),
                    (200, private_repository(self.target)),
                )
            ]
        )
        probe = ScriptedProbe([])
        refs = ScriptedMainRefs([])
        guard = self.make_guard(
            api=api,
            probe=probe,
            refs=refs,
            canonical={self.requested, self.target},
        )

        with self.assertRaisesRegex(
            RepositoryRenameAliasError, "also a canonical fleet identity"
        ):
            guard.repository_lookup(self.requested)

        api.assert_exhausted()
        probe.assert_exhausted()
        refs.assert_exhausted()

    def test_redirect_location_must_pin_the_followed_repository_id(self) -> None:
        api = ScriptedApi(
            [
                (
                    ("GET", f"/repos/{self.requested}", None),
                    (200, private_repository(self.target, repository_id=88)),
                )
            ]
        )
        probe = ScriptedProbe(
            [(self.requested, (301, "/repositories/99"))]
        )
        refs = ScriptedMainRefs([])
        guard = self.make_guard(api=api, probe=probe, refs=refs)

        with self.assertRaisesRegex(
            RepositoryRenameAliasError, "redirect repository id mismatch"
        ):
            guard.repository_lookup(self.requested)

        api.assert_exhausted()
        probe.assert_exhausted()
        refs.assert_exhausted()

    def test_repeat_lookup_rejects_redirect_target_head_drift(self) -> None:
        payload = private_repository(self.target)
        api = ScriptedApi(
            [
                (("GET", f"/repos/{self.requested}", None), (200, payload)),
                (("GET", f"/repos/{self.requested}", None), (200, payload)),
            ]
        )
        probe = ScriptedProbe(
            [
                (self.requested, (301, "/repositories/88")),
                (self.requested, (301, "/repositories/88")),
            ]
        )
        refs = ScriptedMainRefs(
            [
                (self.target, self.stable_head),
                (self.target, "e" * 40),
            ]
        )
        guard = self.make_guard(api=api, probe=probe, refs=refs)

        self.assertEqual(guard.repository_lookup(self.requested), (404, None))
        with self.assertRaisesRegex(
            RepositoryRenameAliasError, "redirect target changed during publication"
        ):
            guard.repository_lookup(self.requested)

        api.assert_exhausted()
        probe.assert_exhausted()
        refs.assert_exhausted()

    def test_final_verification_rejects_repository_id_change(self) -> None:
        initial = private_repository(self.target, repository_id=88)
        changed = private_repository(self.target, repository_id=99)
        api = ScriptedApi(
            [
                (("GET", f"/repos/{self.requested}", None), (200, initial)),
                (("GET", f"/repos/{self.target}", None), (200, changed)),
            ]
        )
        probe = ScriptedProbe(
            [(self.requested, (301, "/repositories/88"))]
        )
        refs = ScriptedMainRefs([(self.target, self.stable_head)])
        guard = self.make_guard(api=api, probe=probe, refs=refs)

        self.assertEqual(guard.repository_lookup(self.requested), (404, None))
        with self.assertRaisesRegex(
            RepositoryRenameAliasError, "redirect target identity changed"
        ):
            guard.verify_preserved()

        api.assert_exhausted()
        probe.assert_exhausted()
        refs.assert_exhausted()

    def test_public_or_non_main_redirect_target_fails_closed(self) -> None:
        for payload, message in (
            (
                private_repository(self.target, visibility="public"),
                "is not private",
            ),
            (
                private_repository(self.target, default_branch="dev"),
                "does not default to main",
            ),
        ):
            with self.subTest(message=message):
                api = ScriptedApi(
                    [
                        (
                            ("GET", f"/repos/{self.requested}", None),
                            (200, payload),
                        )
                    ]
                )
                probe = ScriptedProbe([])
                refs = ScriptedMainRefs([])
                guard = self.make_guard(api=api, probe=probe, refs=refs)

                with self.assertRaisesRegex(RepositoryRenameAliasError, message):
                    guard.repository_lookup(self.requested)

                api.assert_exhausted()
                probe.assert_exhausted()
                refs.assert_exhausted()


if __name__ == "__main__":
    unittest.main()
