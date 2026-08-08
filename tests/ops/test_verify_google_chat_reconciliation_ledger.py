#!/usr/bin/env python3
from __future__ import annotations

import base64
import gzip
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts" / "ops"))

import verify_google_chat_reconciliation_ledger as verifier  # noqa: E402


def write_fixture(root: Path, records: list[dict], *, mutate_index=None, envelope=False) -> None:
    index_routing_version = "test"
    decoded: object = records
    if envelope:
        decoded = {
            "schemaVersion": 1,
            "routingVersion": index_routing_version,
            "sourceKeyPrefix": "google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/messages/",
            "records": records,
        }
    raw = json.dumps(decoded, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    compressed = gzip.compress(raw, mtime=0)
    encoded = base64.b64encode(compressed).decode("ascii")
    midpoint = len(encoded) // 2
    parts = [encoded[:midpoint], encoded[midpoint:]]
    files = ["ledger.json.gz.base64.part-0001", "ledger.json.gz.base64.part-0002"]
    for filename, content in zip(files, parts, strict=True):
        (root / filename).write_text(content, encoding="utf-8")

    dispositions: dict[str, int] = {}
    categories: dict[str, int] = {}
    issues: dict[str, int] = {}
    duplicates = 0
    for record in records:
        dispositions[record["d"]] = dispositions.get(record["d"], 0) + 1
        categories[record["c"]] = categories.get(record["c"], 0) + 1
        for issue in record.get("i", []):
            issues[issue] = issues.get(issue, 0) + 1
        if "dup" in record:
            duplicates += 1

    index = {
        "schemaVersion": 1,
        "routingVersion": index_routing_version,
        "sourceKeyPrefix": "google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/messages/",
        "source": {
            "spaceName": "spaces/AAQAoHKdzvI",
            "displayName": "alex-alex-me",
            "bridgeVersion": "test",
            "exportedAt": "2026-08-01T00:00:00Z",
            "windowStartInclusive": "2026-06-05T00:00:00Z",
            "windowEndInclusive": "2026-08-01T00:00:00Z",
            "exportPages": 1,
            "exportMessages": len(records),
            "windowMessages": len(records),
        },
        "privacy": {
            "containsMessageBodies": False,
            "containsMatchedCredentialValues": False,
            "containsContactValues": False,
            "rawExportCommitted": False,
        },
        "counts": {
            "dispositions": dict(sorted(dispositions.items())),
            "categories": dict(sorted(categories.items())),
            "linearIssues": dict(sorted(issues.items())),
            "duplicates": duplicates,
        },
        "ledger": {
            "files": files,
            "format": "gzip+base64-parts",
            "records": len(records),
            "compressedBytes": len(compressed),
            "base64Characters": len(encoded),
            "parts": len(files),
            "sha256": hashlib.sha256(compressed).hexdigest(),
            "uncompressedJsonBytes": len(raw),
        },
    }
    if mutate_index:
        mutate_index(index)
    (root / "index.json").write_text(json.dumps(index, indent=2) + "\n", encoding="utf-8")


class LedgerVerifierTests(unittest.TestCase):
    def records(self) -> list[dict]:
        return [
            {
                "id": "first-message",
                "t": "2026-06-05T04:00:00Z",
                "d": "mapped-existing-work",
                "c": "general_intake",
                "i": ["DEN-822"],
            },
            {
                "id": "duplicate-message",
                "t": "2026-06-05T04:01:00Z",
                "d": "mapped-existing-work",
                "c": "general_intake",
                "i": ["DEN-822"],
                "dup": "first-message",
            },
        ]

    def test_valid_content_free_array_ledger_round_trips(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root, self.records())
            report = verifier.verify(root)
            self.assertEqual(report["records"], 2)
            self.assertEqual(report["duplicates"], 1)
            self.assertEqual(report["linearIssues"], {"DEN-822": 2})
            self.assertFalse(report["privacy"]["containsMessageBodies"])

    def test_valid_bounded_envelope_round_trips(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root, self.records(), envelope=True)
            self.assertEqual(verifier.verify(root)["records"], 2)

    def test_rejects_arbitrary_envelope_metadata(self) -> None:
        index = {
            "schemaVersion": 1,
            "routingVersion": "test",
            "sourceKeyPrefix": "google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/messages/",
        }
        with self.assertRaisesRegex(verifier.LedgerError, "unsupported keys"):
            verifier.extract_records(
                {"records": self.records(), "messageBodies": ["forbidden"]},
                index,
            )

    def test_rejects_digest_tampering(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root, self.records())
            part = root / "ledger.json.gz.base64.part-0002"
            part.write_text(part.read_text(encoding="utf-8")[:-1] + "A", encoding="utf-8")
            with self.assertRaises(verifier.LedgerError):
                verifier.verify(root)

    def test_rejects_message_content_fields_even_when_counts_match(self) -> None:
        records = self.records()
        records[0]["text"] = "should never be committed"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root, records)
            with self.assertRaisesRegex(verifier.LedgerError, "unsupported keys"):
                verifier.verify(root)

    def test_rejects_forward_duplicate_provenance(self) -> None:
        records = self.records()
        records[0]["dup"] = "duplicate-message"
        records[1].pop("dup")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root, records)
            with self.assertRaisesRegex(verifier.LedgerError, "not earlier"):
                verifier.verify(root)

    def test_rejects_wrong_privacy_declaration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(
                root,
                self.records(),
                mutate_index=lambda index: index["privacy"].update(
                    {"containsContactValues": True}
                ),
            )
            with self.assertRaisesRegex(verifier.LedgerError, "privacy flags"):
                verifier.verify(root)

    def test_rejects_unsafe_or_unordered_part_paths(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(
                root,
                self.records(),
                mutate_index=lambda index: index["ledger"].update(
                    {"files": ["../ledger.json.gz.base64.part-0001", "ledger.json.gz.base64.part-0002"]}
                ),
            )
            with self.assertRaisesRegex(verifier.LedgerError, "unsafe ledger filename"):
                verifier.verify(root)

    def test_rejects_non_chronological_records(self) -> None:
        records = self.records()
        records.reverse()
        records[0].pop("dup", None)
        records[1]["dup"] = records[0]["id"]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root, records)
            with self.assertRaisesRegex(verifier.LedgerError, "not chronological"):
                verifier.verify(root)


if __name__ == "__main__":
    unittest.main()
