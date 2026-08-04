#!/usr/bin/env python3
"""Verify the content-free Google Chat reconciliation ledger."""

from __future__ import annotations

import argparse
import base64
import binascii
import gzip
import hashlib
import json
import re
import sys
from collections import Counter
from datetime import datetime
from pathlib import Path, PurePosixPath
from typing import Any

ISSUE_RE = re.compile(r"^DEN-[1-9][0-9]*$")
ALLOWED_RECORD_KEYS = {"id", "t", "d", "c", "i", "dup"}
REQUIRED_RECORD_KEYS = {"id", "t", "d", "c"}
ALLOWED_ENVELOPE_KEYS = {"schemaVersion", "routingVersion", "sourceKeyPrefix", "records"}
FORBIDDEN_SERIALIZED_KEYS = {
    "text",
    "body",
    "message",
    "formattedText",
    "argumentText",
    "fallbackText",
    "attachments",
    "contact",
    "phone",
    "email",
    "address",
    "credential",
    "secret",
    "token",
}


class LedgerError(RuntimeError):
    """Fail-closed ledger validation error."""


def parse_time(value: str) -> datetime:
    if not isinstance(value, str) or not value:
        raise LedgerError("record timestamp must be a non-empty string")
    candidate = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        return datetime.fromisoformat(candidate)
    except ValueError as error:
        raise LedgerError(f"invalid record timestamp: {value!r}") from error


def require_plain_string(value: Any, label: str, *, maximum: int = 512) -> str:
    if not isinstance(value, str) or not value or len(value) > maximum:
        raise LedgerError(f"{label} must be a non-empty bounded string")
    if any(character.isspace() for character in value):
        raise LedgerError(f"{label} contains whitespace")
    if any(ord(character) < 32 for character in value):
        raise LedgerError(f"{label} contains control characters")
    return value


def safe_part_name(value: Any) -> str:
    name = require_plain_string(value, "ledger filename", maximum=128)
    pure = PurePosixPath(name)
    if pure.name != name or pure.is_absolute() or ".." in pure.parts:
        raise LedgerError(f"unsafe ledger filename: {name!r}")
    if not re.fullmatch(r"ledger\.json\.gz\.base64\.part-[0-9]{4}", name):
        raise LedgerError(f"unexpected ledger filename: {name!r}")
    return name


def sum_counter(values: dict[str, Any], label: str) -> int:
    if not isinstance(values, dict) or not values:
        raise LedgerError(f"{label} counts must be a non-empty object")
    total = 0
    for key, value in values.items():
        require_plain_string(key, f"{label} key", maximum=128)
        if not isinstance(value, int) or value < 0:
            raise LedgerError(f"{label} count for {key!r} must be a non-negative integer")
        total += value
    return total


def validate_index(index: dict[str, Any]) -> tuple[list[str], int]:
    if index.get("schemaVersion") != 1:
        raise LedgerError("unsupported index schemaVersion")
    source = index.get("source")
    if not isinstance(source, dict):
        raise LedgerError("index source must be an object")
    if source.get("spaceName") != "spaces/AAQAoHKdzvI":
        raise LedgerError("index targets the wrong Google Chat space")
    if source.get("displayName") != "alex-alex-me":
        raise LedgerError("index targets the wrong Google Chat display name")
    window_messages = source.get("windowMessages")
    if not isinstance(window_messages, int) or window_messages <= 0:
        raise LedgerError("source.windowMessages must be a positive integer")

    privacy = index.get("privacy")
    expected_privacy = {
        "containsMessageBodies": False,
        "containsMatchedCredentialValues": False,
        "containsContactValues": False,
        "rawExportCommitted": False,
    }
    if privacy != expected_privacy:
        raise LedgerError("privacy flags must exactly declare a content-free ledger")

    counts = index.get("counts")
    if not isinstance(counts, dict):
        raise LedgerError("index counts must be an object")
    dispositions_total = sum_counter(counts.get("dispositions"), "disposition")
    categories_total = sum_counter(counts.get("categories"), "category")
    if dispositions_total != window_messages or categories_total != window_messages:
        raise LedgerError("index disposition/category totals do not match windowMessages")
    duplicates = counts.get("duplicates")
    if not isinstance(duplicates, int) or duplicates < 0:
        raise LedgerError("counts.duplicates must be a non-negative integer")
    linear_counts = counts.get("linearIssues")
    if not isinstance(linear_counts, dict):
        raise LedgerError("counts.linearIssues must be an object")
    for issue, count in linear_counts.items():
        if ISSUE_RE.fullmatch(issue) is None or not isinstance(count, int) or count <= 0:
            raise LedgerError(f"invalid Linear count entry: {issue!r}")

    ledger = index.get("ledger")
    if not isinstance(ledger, dict) or ledger.get("format") != "gzip+base64-parts":
        raise LedgerError("unsupported ledger format")
    files = ledger.get("files")
    if not isinstance(files, list):
        raise LedgerError("ledger.files must be an array")
    safe_files = [safe_part_name(value) for value in files]
    if safe_files != sorted(safe_files) or len(set(safe_files)) != len(safe_files):
        raise LedgerError("ledger files must be unique and lexically ordered")
    if ledger.get("parts") != len(safe_files):
        raise LedgerError("ledger part count does not match files")
    if ledger.get("records") != window_messages:
        raise LedgerError("ledger record count does not match source window")
    for key in ("compressedBytes", "base64Characters", "uncompressedJsonBytes"):
        value = ledger.get(key)
        if not isinstance(value, int) or value <= 0:
            raise LedgerError(f"ledger.{key} must be a positive integer")
    digest = ledger.get("sha256")
    if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
        raise LedgerError("ledger.sha256 must be lowercase hexadecimal")
    return safe_files, window_messages


def extract_records(decoded: Any, index: dict[str, Any]) -> list[dict[str, Any]]:
    if isinstance(decoded, list):
        return decoded
    if not isinstance(decoded, dict):
        raise LedgerError("ledger root must be an array or bounded envelope")
    unexpected = set(decoded) - ALLOWED_ENVELOPE_KEYS
    if unexpected:
        raise LedgerError(f"ledger envelope has unsupported keys: {sorted(unexpected)}")
    records = decoded.get("records")
    if not isinstance(records, list):
        raise LedgerError("ledger envelope records must be an array")
    schema_version = decoded.get("schemaVersion")
    if schema_version is not None and schema_version != index["schemaVersion"]:
        raise LedgerError("ledger envelope schemaVersion differs from index")
    routing_version = decoded.get("routingVersion")
    if routing_version is not None and routing_version != index.get("routingVersion"):
        raise LedgerError("ledger envelope routingVersion differs from index")
    source_prefix = decoded.get("sourceKeyPrefix")
    if source_prefix is not None and source_prefix != index.get("sourceKeyPrefix"):
        raise LedgerError("ledger envelope sourceKeyPrefix differs from index")
    return records


def load_ledger(root: Path, index: dict[str, Any], files: list[str]) -> tuple[list[dict[str, Any]], bytes]:
    encoded_parts: list[str] = []
    for filename in files:
        part = root / filename
        if not part.is_file():
            raise LedgerError(f"missing ledger part: {filename}")
        encoded_parts.append(part.read_text(encoding="utf-8"))
    compact = "".join("".join(encoded_parts).split())
    if len(compact) != index["ledger"]["base64Characters"]:
        raise LedgerError("base64 character count does not match index")
    try:
        compressed = base64.b64decode(compact, validate=True)
    except (ValueError, binascii.Error) as error:
        raise LedgerError("ledger parts are not valid base64") from error
    if len(compressed) != index["ledger"]["compressedBytes"]:
        raise LedgerError("compressed byte count does not match index")
    digest = hashlib.sha256(compressed).hexdigest()
    if digest != index["ledger"]["sha256"]:
        raise LedgerError("compressed ledger SHA-256 does not match index")
    try:
        raw = gzip.decompress(compressed)
    except OSError as error:
        raise LedgerError("ledger is not valid gzip") from error
    if len(raw) != index["ledger"]["uncompressedJsonBytes"]:
        raise LedgerError("uncompressed JSON byte count does not match index")
    try:
        decoded = json.loads(raw)
    except json.JSONDecodeError as error:
        raise LedgerError("ledger JSON is malformed") from error
    return extract_records(decoded, index), raw


def validate_records(index: dict[str, Any], records: list[dict[str, Any]], raw: bytes) -> dict[str, Any]:
    expected_records = index["ledger"]["records"]
    if len(records) != expected_records:
        raise LedgerError("decoded record count does not match index")
    source_prefix = require_plain_string(index.get("sourceKeyPrefix"), "sourceKeyPrefix", maximum=512)

    dispositions: Counter[str] = Counter()
    categories: Counter[str] = Counter()
    linear_issues: Counter[str] = Counter()
    seen_ids: set[str] = set()
    duplicate_count = 0
    previous_time: datetime | None = None

    for position, record in enumerate(records):
        if not isinstance(record, dict):
            raise LedgerError(f"record {position} is not an object")
        keys = set(record)
        if not REQUIRED_RECORD_KEYS <= keys or not keys <= ALLOWED_RECORD_KEYS:
            raise LedgerError(f"record {position} has unsupported keys: {sorted(keys)}")
        identifier = require_plain_string(record["id"], f"record {position} id", maximum=256)
        if identifier in seen_ids:
            raise LedgerError(f"duplicate record id: {identifier}")
        if any(character in identifier for character in ("/", "\\", "?", "#")):
            raise LedgerError(f"record {position} id is not a message-id suffix")
        if len(source_prefix + identifier) > 1024:
            raise LedgerError(f"record {position} source key is too long")

        created_at = parse_time(record["t"])
        if previous_time is not None and created_at < previous_time:
            raise LedgerError("ledger records are not chronological")
        previous_time = created_at

        disposition = require_plain_string(record["d"], f"record {position} disposition", maximum=128)
        category = require_plain_string(record["c"], f"record {position} category", maximum=128)
        dispositions[disposition] += 1
        categories[category] += 1

        issues = record.get("i", [])
        if not isinstance(issues, list) or len(issues) != len(set(issues)):
            raise LedgerError(f"record {position} Linear issue list is malformed")
        for issue in issues:
            if not isinstance(issue, str) or ISSUE_RE.fullmatch(issue) is None:
                raise LedgerError(f"record {position} has invalid Linear issue identifier")
            linear_issues[issue] += 1

        duplicate_of = record.get("dup")
        if duplicate_of is not None:
            duplicate_of = require_plain_string(duplicate_of, f"record {position} duplicate target", maximum=256)
            if duplicate_of not in seen_ids:
                raise LedgerError(f"record {position} duplicate target is not earlier in the ledger")
            duplicate_count += 1
        seen_ids.add(identifier)

    if dict(dispositions) != index["counts"]["dispositions"]:
        raise LedgerError("decoded disposition counts do not match index")
    if dict(categories) != index["counts"]["categories"]:
        raise LedgerError("decoded category counts do not match index")
    if dict(sorted(linear_issues.items())) != index["counts"]["linearIssues"]:
        raise LedgerError("decoded Linear issue counts do not match index")
    if duplicate_count != index["counts"]["duplicates"]:
        raise LedgerError("decoded duplicate count does not match index")

    lowered = raw.decode("utf-8").lower()
    for forbidden in FORBIDDEN_SERIALIZED_KEYS:
        if f'"{forbidden.lower()}"' in lowered:
            raise LedgerError(f"ledger contains forbidden content field: {forbidden}")

    return {
        "schemaVersion": index["schemaVersion"],
        "source": index["source"],
        "records": len(records),
        "duplicates": duplicate_count,
        "dispositions": dict(dispositions),
        "categories": dict(categories),
        "linearIssues": dict(sorted(linear_issues.items())),
        "sha256": index["ledger"]["sha256"],
        "privacy": index["privacy"],
    }


def verify(root: Path) -> dict[str, Any]:
    index_path = root / "index.json"
    if not index_path.is_file():
        raise LedgerError(f"missing index: {index_path}")
    try:
        index = json.loads(index_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise LedgerError("index.json is malformed") from error
    if not isinstance(index, dict):
        raise LedgerError("index root must be an object")
    files, _ = validate_index(index)
    records, raw = load_ledger(root, index, files)
    return validate_records(index, records, raw)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path("docs/audits/google-chat/alex-alex-me-since-2026-06-05"),
    )
    parser.add_argument("--json-output", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        report = verify(args.root)
    except (LedgerError, OSError) as error:
        print(f"Google Chat ledger verification failed: {error}", file=sys.stderr)
        return 1
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.json_output:
        args.json_output.parent.mkdir(parents=True, exist_ok=True)
        args.json_output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
