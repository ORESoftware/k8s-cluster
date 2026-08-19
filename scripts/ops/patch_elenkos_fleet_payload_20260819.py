#!/usr/bin/env python3
from __future__ import annotations

import base64
import gzip
import hashlib
import json
import sys
from pathlib import Path

GENERATOR_ORIGINAL_SHA256 = "de511f13abd079437860a826c4e0dea50bfea90d15c76014432acb4b926e4016"
GENERATOR_PATCHED_SHA256 = "84957238e1131e5fff9171a4ffb27478c1430f4acec4e8e17af402fae1b8016c"
AUGMENT_ORIGINAL_SHA256 = "58ea2870c136160847e65e864388e4f85be92954fbdede356d90178eceb36c90"
AUGMENT_PATCHED_SHA256 = "95d8482f50b6e4ddc02ef2e4dfc0e264ff92365c72661bbfb06c5d10ae8cd563"
AUGMENT_PATCH_GZIP_B64 = "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"
GENERATOR_PATCH_GZIP_B64 = "H4sIAAAAAAAC/81V227bMAz9FdUPqw0Y6QXDLh76sHV9KLABQ4vuYXEgqBbtEZUlV5KbpkH+vZITN3ZTrxd0w/SSyCQPDw9JezwOyOrUBohmkidJpUHUHJJkfiw5XAM/cY9VGZNTy09ksfiUys2gUwDOzgU4B2cP4uAxlxflDSZxnzEIkBfKUIHnNFPaB6+h/flsDBayBGkPHRpyZiEmh0oakKY2P5TAbBaT7LdSBqiGK4QpaPeg9aCZBo6WsinT3MR9cA1GiSugxqfuGhebGvxrpo+Q6wrpDzMGtN0KB+DCPld/zmp0TZMwpVdvwyj+Gw6Ody3sA4ZqqUbfEPWvIzQUysrOwijqt6Nb7yb20xV4Ug2v6zSoyKAqDyhzT517Kg5PB4VLNyD9AQw1VEpbP27+10/km6ydXzeF76NRLaeaVa4ktwNYoHsDDPajyfBAS16ecxOsJTFQdq5VSeysQlkQLD0++c4qf204/8mcysbK6sJvMW03PhcAlu7v7r/b/bD3sY1qvXwNBq3SM5qj8PRvgNOKZRescMsr6qLTj8aDHJAvZ8ffvh6dnI5NBdnoAiWfhP5vTBwTQ5Qm80VMSsUhulO6jR1KvAJ4BvQ6Q8sPc7JiPirAhmmgdJEGEdk6IA2cuzKJN8yikh6q7yxZCV1vf0/WbdIM3Xv0JxM1HGmtdJinwbxxzGshqPdeJOQX8BaWIHeFop0RrjG3DnoJxiETzO22E6N0dHIwdsWAQwXu2yMzBJMGsSt1FQLXLo9tQuaZUpqjdLOWuAm8rFGDF5TkrqC1Le7ZUC5L6iZY3LXm1WS7PzrhnY7RfyjkmpEnsJZuZCqBNtze2Y7JXjTenSx25huVDbrvTaJFGvRa00n0zB4tI32nJpNb39A19S0JAAA="

MATERIALIZED_REPLACEMENTS: dict[str, tuple[tuple[str, str, int], ...]] = {
    "scripts/ops/publish_elenkos_fleet_20260819.py": (
        (
            'document.get("private") is not False or document.get("visibility") != "public"',
            'document.get("private") is not True or document.get("visibility") != "private"',
            1,
        ),
        ("repository must be public", "repository must be private", 1),
        ('"private": False', '"private": True', 1),
        ('"visibility": "public"', '"visibility": "private"', 2),
        ('visibility="public"', 'visibility="private"', 1),
    ),
    "scripts/ops/test_elenkos_fleet_20260819.py": (
        (
            "test_repository_creation_is_public_but_never_auto_initialized",
            "test_repository_creation_is_private_but_never_auto_initialized",
            1,
        ),
        (
            'self.assertIs(payload["private"], False)',
            'self.assertIs(payload["private"], True)',
            1,
        ),
    ),
    "scripts/ops/run_protected_elenkos_fleet_20260819.sh": (
        (
            '.publication.visibility == "public"',
            '.publication.visibility == "private"',
            1,
        ),
        ('.visibility == "public"', '.visibility == "private"', 1),
    ),
    "scripts/ops/validate_elenkos_fleet_payload_20260819.sh": (
        ('\'"private": False\'', '\'"private": True\'', 1),
    ),
    "docs/den-3786-elenkos-fleet.md": (
        ("public GitHub organizations", "private GitHub organizations", 1),
        ("Repository creation is public", "Repository creation is private", 1),
        ("public/no-auto-init creation", "private/no-auto-init creation", 1),
    ),
}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def replacements(payload: str) -> tuple[tuple[str, str], ...]:
    decoded = json.loads(gzip.decompress(base64.b64decode(payload)).decode("utf-8"))
    return tuple((str(old), str(new)) for old, new in decoded)


def patch_file(path: Path, original: str, patched: str, payload: str, label: str) -> str:
    raw = path.read_bytes()
    current = digest(raw)
    if current == patched:
        return "already-applied"
    if current != original:
        raise RuntimeError(f"refusing unexpected {label}: expected {original} or {patched}, got {current}")
    text = raw.decode("utf-8")
    for index, (old, new) in enumerate(replacements(payload)):
        count = text.count(old)
        if count != 1:
            raise RuntimeError(f"{label} patch target {index} expected once, found {count}")
        text = text.replace(old, new, 1)
    output = text.encode("utf-8")
    actual = digest(output)
    if actual != patched:
        raise RuntimeError(f"patched {label} digest mismatch: expected {patched}, got {actual}")
    path.write_bytes(output)
    return "applied"


def patch_materialized_contracts(root: Path) -> None:
    for relative, changes in MATERIALIZED_REPLACEMENTS.items():
        path = root / relative
        text = path.read_text(encoding="utf-8")
        changed = False
        for old, new, expected_count in changes:
            old_count = text.count(old)
            new_count = text.count(new)
            if old_count == expected_count:
                text = text.replace(old, new)
                changed = True
            elif old_count == 0 and new_count >= expected_count:
                continue
            else:
                raise RuntimeError(
                    f"unexpected visibility patch count in {relative}: "
                    f"old={old_count} new={new_count} expected={expected_count}"
                )
        if changed:
            path.write_text(text, encoding="utf-8")


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: patch_elenkos_fleet_payload_20260819.py ROOT")
    root = Path(sys.argv[1]).resolve()
    augment = root / "scripts/ops/augment_elenkos_fleet_20260819.py"
    generator = root / "scripts/ops/elenkos_fleet_spec_20260819.py"
    augment_status = patch_file(
        augment, AUGMENT_ORIGINAL_SHA256, AUGMENT_PATCHED_SHA256,
        AUGMENT_PATCH_GZIP_B64, "augmentation source",
    )
    generator_status = patch_file(
        generator, GENERATOR_ORIGINAL_SHA256, GENERATOR_PATCHED_SHA256,
        GENERATOR_PATCH_GZIP_B64, "fleet generator",
    )
    patch_materialized_contracts(root)
    print(
        "ELENKOS_PAYLOAD_PATCHED "
        f"generator_sha256={digest(generator.read_bytes())} generator_status={generator_status} "
        f"augment_sha256={digest(augment.read_bytes())} augment_status={augment_status} "
        "visibility=private"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
