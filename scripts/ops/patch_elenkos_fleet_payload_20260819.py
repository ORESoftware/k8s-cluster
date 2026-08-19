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
    "augment": ("58ea2870c136160847e65e864388e4f85be92954fbdede356d90178eceb36c90", "30eb8f6a95b6c66dc6eacacc3b4eebe3f688373b5945f145f0bbc46a06cba71e"),
    "generator_original": "de511f13abd079437860a826c4e0dea50bfea90d15c76014432acb4b926e4016",
    "generator_base": "fa1c194e62dccf0867aa8dd251acf868f887756e7706ce50c4e422ad8e774a2e",
    "generator_final": "39ccecd3cb25af39d752758fffdb85551c12e709f74c2ed94217638f4c5c9d29",
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
