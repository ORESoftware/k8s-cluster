#!/usr/bin/env python3
from __future__ import annotations

from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest

from record_repository_fleet_publication_evidence import (
    PublicationEvidenceError,
    collect_publication_evidence,
    parse_publisher_log,
    write_evidence,
)


ALIASES = [
    (
        "streempilot/streempilot-flutter-app",
        "StreemPilot/sp-web-leptos",
        1318677943,
    ),
    ("streempilot/streempilot-infra", "StreemPilot/sp-infra", 1318678140),
    (
        "streempilot/streempilot-interfaces",
        "StreemPilot/sp-interfaces",
        1318677845,
    ),
    (
        "streempilot/streempilot-mcp-server.rs",
        "StreemPilot/sp-web-dioxus",
        1318678104,
    ),
    ("streempilot/streempilot-sync", "StreemPilot/sp-sync", 1318678045),
    (
        "streempilot/streempilot-web-server.rs",
        "StreemPilot/sp-web-mash",
        1318677908,
    ),
]
CREATED = [
    "hypesiege/hypesiege-analytics.rs",
    "hypesiege/hypesiege-cli.rs",
    "hypesiege/hypesiege-connectors",
    "hypesiege/hypesiege-publishing-worker.rs",
    "hypesiege/hypesiege-scheduler.rs",
    "streempilot/streempilot-chat.rs",
    "streempilot/streempilot-cli.rs",
    "streempilot/streempilot-compositor.rs",
    "streempilot/streempilot-destinations",
    "streempilot/streempilot-media-router.rs",
    "streempilot/streempilot-recording.rs",
    "streempilot/streempilot-webrtc-adapter.rs",
]
EXACT_PRESERVED = [f"hypesiege/reviewed-existing-{index}" for index in range(14)]
REQUIRED = [
    "streempilot/streempilot-media-router.rs",
    "hypesiege/hypesiege-scheduler.rs",
    "hypesiege/hypesiege-publishing-worker.rs",
    "hypesiege/hypesiege-analytics.rs",
]


class FakeRemote:
    def __init__(self) -> None:
        self.repositories: dict[str, dict[str, object]] = {}
        self.heads: dict[str, str] = {}

    def add(
        self,
        requested: str,
        remote: str,
        *,
        repository_id: int,
        head: str,
    ) -> None:
        payload = {
            "id": repository_id,
            "full_name": remote,
            "private": True,
            "visibility": "private",
            "default_branch": "main",
            "archived": False,
            "disabled": False,
            "html_url": f"https://github.com/{remote}",
        }
        self.repositories[requested.casefold()] = payload
        self.repositories.setdefault(remote.casefold(), payload)
        self.heads[remote.casefold()] = head

    def lookup(self, full_name: str):
        payload = self.repositories.get(full_name.casefold())
        return (200, payload) if payload is not None else (404, None)

    def main_ref(self, full_name: str):
        return self.heads.get(full_name.casefold())


def alias_payload() -> dict[str, object]:
    return {
        "schema_version": 1,
        "source_repository": "ORESoftware/ai-agent-coordinator.rs",
        "source_sha": "5d9a0c2cb44dff607bc3953954ce4b9af08e5789",
        "aliases": [
            {
                "sealed_full_name": sealed,
                "remote_full_name": remote,
                "repository_id": repository_id,
            }
            for sealed, remote, repository_id in ALIASES
        ],
    }


def fixture() -> tuple[str, dict[str, str], dict[str, str], FakeRemote]:
    created_heads = {
        full_name: f"{index + 1:040x}"[-40:]
        for index, full_name in enumerate(CREATED)
    }
    preserved_heads: dict[str, str] = {}
    remote = FakeRemote()
    lines: list[str] = []

    for index, full_name in enumerate(CREATED, start=1):
        head = created_heads[full_name]
        remote.add(full_name, full_name, repository_id=1000 + index, head=head)
        lines.append(f"VERIFIED_CREATED_PRIVATE {full_name} {head}")

    for index, full_name in enumerate(EXACT_PRESERVED, start=1):
        head = f"{100 + index:040x}"[-40:]
        preserved_heads[full_name] = head
        remote.add(full_name, full_name, repository_id=2000 + index, head=head)
        lines.append(f"VERIFIED_PRESERVED_PRIVATE {full_name} {head}")

    for index, (sealed, target, repository_id) in enumerate(ALIASES, start=1):
        head = f"{200 + index:040x}"[-40:]
        preserved_heads[sealed] = head
        remote.add(sealed, target, repository_id=repository_id, head=head)
        lines.append(
            f"VERIFIED_PRESERVED_RENAMED {sealed} {target} {repository_id} {head}"
        )
        lines.append(f"VERIFIED_PRESERVED_PRIVATE {sealed} {head}")

    lines.append(
        "VERIFIED private canonical fleet remote state "
        "created=12 preserved=20 total=32"
    )
    return "\n".join(lines) + "\n", created_heads, preserved_heads, remote


class RepositoryPublicationEvidenceTests(unittest.TestCase):
    def write_alias_ledger(self, root: Path) -> Path:
        path = root / "aliases.json"
        path.write_text(json.dumps(alias_payload()), encoding="utf-8")
        return path

    def test_parses_and_verifies_all_created_alias_and_required_evidence(self) -> None:
        log, _, _, remote = fixture()
        parsed = parse_publisher_log(log)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            evidence = collect_publication_evidence(
                parsed,
                alias_ledger_path=self.write_alias_ledger(root),
                repository_lookup=remote.lookup,
                main_ref_lookup=remote.main_ref,
                required_repositories=REQUIRED,
            )
            self.assertEqual(evidence["summary"], {
                "created": 12,
                "preserved": 20,
                "renamed": 6,
                "total": 32,
            })
            self.assertEqual(len(evidence["created_repositories"]), 12)
            self.assertEqual(len(evidence["preserved_aliases"]), 6)
            self.assertEqual(len(evidence["required_repositories"]), 4)
            write_evidence(root / "out", evidence)
            self.assertEqual(
                json.loads((root / "out/summary.json").read_text()),
                evidence["summary"],
            )

    def test_parser_rejects_missing_duplicate_and_mismatched_summary_evidence(self) -> None:
        log, _, _, _ = fixture()
        with self.assertRaisesRegex(PublicationEvidenceError, "lacks the final"):
            parse_publisher_log(log.rsplit("VERIFIED private", 1)[0])

        first_created = log.splitlines()[0]
        with self.assertRaisesRegex(PublicationEvidenceError, "duplicate created"):
            parse_publisher_log(f"{first_created}\n{log}")

        with self.assertRaisesRegex(PublicationEvidenceError, "count mismatch"):
            parse_publisher_log(log.replace("created=12", "created=11"))

        with self.assertRaisesRegex(PublicationEvidenceError, "publisher total changed"):
            parse_publisher_log(log.replace("total=32", "total=31"))

    def test_created_redirect_alias_swap_and_head_drift_fail_closed(self) -> None:
        log, created_heads, preserved_heads, remote = fixture()
        parsed = parse_publisher_log(log)
        with tempfile.TemporaryDirectory() as temporary:
            ledger = self.write_alias_ledger(Path(temporary))

            first_created = CREATED[0]
            remote.repositories[first_created.casefold()]["full_name"] = "hypesiege/unreviewed"
            with self.assertRaisesRegex(PublicationEvidenceError, "unexpected redirect"):
                collect_publication_evidence(
                    parsed,
                    alias_ledger_path=ledger,
                    repository_lookup=remote.lookup,
                    main_ref_lookup=remote.main_ref,
                    required_repositories=REQUIRED,
                )

            remote.repositories[first_created.casefold()]["full_name"] = first_created
            sealed, target, _ = ALIASES[0]
            remote.repositories[sealed.casefold()]["full_name"] = "StreemPilot/sp-web-dioxus"
            with self.assertRaisesRegex(PublicationEvidenceError, "alias target changed"):
                collect_publication_evidence(
                    parsed,
                    alias_ledger_path=ledger,
                    repository_lookup=remote.lookup,
                    main_ref_lookup=remote.main_ref,
                    required_repositories=REQUIRED,
                )

            remote.repositories[sealed.casefold()]["full_name"] = target
            remote.heads[target.casefold()] = "f" * 40
            with self.assertRaisesRegex(PublicationEvidenceError, "alias head changed"):
                collect_publication_evidence(
                    parsed,
                    alias_ledger_path=ledger,
                    repository_lookup=remote.lookup,
                    main_ref_lookup=remote.main_ref,
                    required_repositories=REQUIRED,
                )

            remote.heads[target.casefold()] = preserved_heads[sealed]
            remote.heads[first_created.casefold()] = "f" * 40
            with self.assertRaisesRegex(PublicationEvidenceError, "head mismatch"):
                collect_publication_evidence(
                    parsed,
                    alias_ledger_path=ledger,
                    repository_lookup=remote.lookup,
                    main_ref_lookup=remote.main_ref,
                    required_repositories=REQUIRED,
                )
            remote.heads[first_created.casefold()] = created_heads[first_created]

    def test_missing_required_repository_and_alias_log_drift_fail_closed(self) -> None:
        log, _, _, remote = fixture()
        parsed = parse_publisher_log(log)
        with tempfile.TemporaryDirectory() as temporary:
            ledger = self.write_alias_ledger(Path(temporary))
            required = REQUIRED[0]
            remote.repositories.pop(required.casefold())
            with self.assertRaisesRegex(PublicationEvidenceError, "absent after publication"):
                collect_publication_evidence(
                    parsed,
                    alias_ledger_path=ledger,
                    repository_lookup=remote.lookup,
                    main_ref_lookup=remote.main_ref,
                    required_repositories=REQUIRED,
                )

            drifted = log.replace(
                "VERIFIED_PRESERVED_RENAMED "
                "streempilot/streempilot-flutter-app StreemPilot/sp-web-leptos",
                "VERIFIED_PRESERVED_RENAMED "
                "streempilot/streempilot-flutter-app StreemPilot/sp-web-dioxus",
            )
            with self.assertRaisesRegex(PublicationEvidenceError, "does not exactly match"):
                collect_publication_evidence(
                    parse_publisher_log(drifted),
                    alias_ledger_path=ledger,
                    repository_lookup=remote.lookup,
                    main_ref_lookup=remote.main_ref,
                    required_repositories=[],
                )


if __name__ == "__main__":
    unittest.main()
