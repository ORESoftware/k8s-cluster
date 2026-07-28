#!/usr/bin/env python3
"""Fail if sensitive backend DTOs regain raw derived Debug output."""

from __future__ import annotations

from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]

SENSITIVE_TYPES = {
    "src/device_sync_protocol.rs": [
        "SignalEnvelopeMetadata",
        "SignalCiphertextEnvelope",
        "SignalDevicePreKeyBundle",
    ],
    "src/account_security.rs": [
        "DeviceSummary",
        "UpsertRecoveryChannelRequest",
        "RecoveryChannelSummary",
        "RecoveryChallenge",
        "VerifyRecoveryChallengeRequest",
    ],
}

FORBIDDEN_FRAGMENTS = {
    "src/device_sync_protocol.rs": [
        '.field("ciphertext", &self.ciphertext)',
        '.field("identity_key", &self.identity_key)',
        '.field("signed_pre_key", &self.signed_pre_key)',
        '.field("pq_signed_pre_key", &self.pq_signed_pre_key)',
        '.field("one_time_pre_key", &self.one_time_pre_key)',
    ],
    "src/account_security.rs": [
        '.field("device_id", &self.device_id)',
        '.field("device_name", &self.device_name)',
        '.field("identity_key_fingerprint_base64", &self.identity_key_fingerprint_base64)',
        '.field("destination", &self.destination)',
        '.field("masked_destination", &self.masked_destination)',
        '.field("challenge_id", &self.challenge_id)',
        '.field("channel_id", &self.channel_id)',
        '.field("code", &self.code)',
    ],
}


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(1)


def derive_traits(source: str, type_name: str) -> set[str]:
    pattern = re.compile(
        rf"#\[derive\((?P<traits>[^)]*)\)\]\s*"
        rf"(?:#\[[^\]]+\]\s*)*"
        rf"pub struct {re.escape(type_name)}\b",
        re.MULTILINE,
    )
    match = pattern.search(source)
    if not match:
        fail(f"could not locate derive declaration for {type_name}")
    return {trait.strip() for trait in match.group("traits").split(",")}


def main() -> None:
    checked = 0
    for relative, type_names in SENSITIVE_TYPES.items():
        path = ROOT / relative
        source = path.read_text(encoding="utf-8")
        if "use std::fmt;" not in source:
            fail(f"{relative}: redacted Debug implementations require std::fmt")

        for type_name in type_names:
            traits = derive_traits(source, type_name)
            if "Debug" in traits:
                fail(f"{relative}: {type_name} must not derive raw Debug")
            if not re.search(rf"impl\s+fmt::Debug\s+for\s+{re.escape(type_name)}\b", source):
                fail(f"{relative}: {type_name} has no explicit redacted Debug implementation")
            checked += 1

        for fragment in FORBIDDEN_FRAGMENTS[relative]:
            if fragment in source:
                fail(f"{relative}: raw sensitive Debug field returned: {fragment}")

        if '"<redacted>"' not in source:
            fail(f"{relative}: no explicit redaction marker")

    print(f"verified explicit redacted Debug implementations for {checked} sensitive DTOs")


if __name__ == "__main__":
    main()
