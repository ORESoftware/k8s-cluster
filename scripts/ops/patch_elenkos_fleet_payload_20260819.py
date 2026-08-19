#!/usr/bin/env python3
from __future__ import annotations

import base64
import gzip
import hashlib
import json
import re
import sys
from pathlib import Path

PAYLOAD = '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'
HASHES = {
    "augment": ("58ea2870c136160847e65e864388e4f85be92954fbdede356d90178eceb36c90", "385cf3f31555e97650eb934e50b71347f2bc4b862974730887d87acb28dc0e55"),
    "generator_original": "de511f13abd079437860a826c4e0dea50bfea90d15c76014432acb4b926e4016",
    "generator_base": "fa1c194e62dccf0867aa8dd251acf868f887756e7706ce50c4e422ad8e774a2e",
    "generator_final": "4f1b7008f6258c2edc78fbee86400d3294a425931c6bad3b9d8dc732c4ee092c",
}


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def decode_payload() -> dict[str, object]:
    return json.loads(gzip.decompress(base64.b64decode(PAYLOAD)).decode("utf-8"))


def apply(text: str, changes: list[list[str]], label: str) -> str:
    for index, (old, new) in enumerate(changes):
        count = text.count(old)
        if count != 1:
            raise RuntimeError(f"{label} replacement {index} expected once, found {count}")
        text = text.replace(old, new, 1)
    return text


def patch_file(path: Path, source: str, target: str, changes: list[list[str]], label: str) -> str:
    current = digest(path)
    if current == target:
        return "already-applied"
    if current != source:
        raise RuntimeError(f"refusing unexpected {label}: {current}")
    path.write_text(apply(path.read_text(encoding="utf-8"), changes, label), encoding="utf-8")
    actual = digest(path)
    if actual != target:
        raise RuntimeError(f"{label} digest mismatch: expected {target}, got {actual}")
    return "applied"


def patch_materialized(root: Path, changes: dict[str, list[list[object]]]) -> None:
    for relative, replacements in changes.items():
        path = root / relative
        text = path.read_text(encoding="utf-8")
        changed = False
        for old, new, expected in replacements:
            old_count = text.count(old)
            new_count = text.count(new)
            if old_count == expected:
                text = text.replace(old, new)
                changed = True
            elif old_count == 0 and new_count >= expected:
                continue
            else:
                raise RuntimeError(
                    f"unexpected contract count in {relative}: old={old_count} new={new_count} expected={expected}"
                )
        if changed:
            path.write_text(text, encoding="utf-8")


def patch_docs(root: Path) -> None:
    path = root / "docs/den-3786-elenkos-fleet.md"
    text = path.read_text(encoding="utf-8")
    for old, new in (
        ("public GitHub organizations", "private GitHub organizations"),
        ("Repository creation is public", "Repository creation is private"),
        ("public/no-auto-init creation", "private/no-auto-init creation"),
    ):
        text = text.replace(old, new)
    forbidden = re.compile(
        r"public GitHub organizations|Repository creation is public|public/no-auto-init creation",
        re.IGNORECASE,
    )
    if forbidden.search(text):
        raise RuntimeError("Elenkos documentation still advertises public repository creation")
    path.write_text(text, encoding="utf-8")


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: patch_elenkos_fleet_payload_20260819.py ROOT")
    root = Path(sys.argv[1]).resolve()
    data = decode_payload()

    augment = root / "scripts/ops/augment_elenkos_fleet_20260819.py"
    augment_status = patch_file(
        augment, HASHES["augment"][0], HASHES["augment"][1], data["augment"], "augmentation source"
    )

    generator = root / "scripts/ops/elenkos_fleet_spec_20260819.py"
    current = digest(generator)
    if current == HASHES["generator_final"]:
        generator_status = "already-applied"
    else:
        text = generator.read_text(encoding="utf-8")
        if current == HASHES["generator_original"]:
            text = apply(text, data["generator_base"], "base generator")
            if hashlib.sha256(text.encode()).hexdigest() != HASHES["generator_base"]:
                raise RuntimeError("base generator digest mismatch")
        elif current != HASHES["generator_base"]:
            raise RuntimeError(f"refusing unexpected fleet generator: {current}")
        text = apply(text, data["generator_harden"], "hardened generator")
        generator.write_text(text, encoding="utf-8")
        if digest(generator) != HASHES["generator_final"]:
            raise RuntimeError("final generator digest mismatch")
        generator_status = "applied"

    patch_materialized(root, data["materialized"])
    patch_docs(root)
    print(
        "ELENKOS_PAYLOAD_PATCHED "
        f"generator_sha256={digest(generator)} generator_status={generator_status} "
        f"augment_sha256={digest(augment)} augment_status={augment_status} "
        "visibility=private zed_aliases=slug-safe"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
