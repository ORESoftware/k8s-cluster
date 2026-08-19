#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import sys
from pathlib import Path

ORIGINAL_SHA256 = "de511f13abd079437860a826c4e0dea50bfea90d15c76014432acb4b926e4016"
PATCHED_SHA256 = "fa1c194e62dccf0867aa8dd251acf868f887756e7706ce50c4e422ad8e774a2e"
AUGMENT_SHA256 = "58ea2870c136160847e65e864388e4f85be92954fbdede356d90178eceb36c90"

REPLACEMENTS: tuple[tuple[str, str], ...] = (
    (
        """        use rand::prelude::{IndexedRandom, StdRng};
        use rand::SeedableRng;
""",
        """        use rand::SeedableRng;
        use rand::prelude::{IndexedRandom, StdRng};
""",
    ),
    (
        """        use elenkos_lib_core::{
            AssignmentCandidate, ConsensusPolicy, choose_reviewer, consensus_credit_awards,
            resolve_scores,
        };
""",
        """        use elenkos_lib_core::{
            AssignmentCandidate, ConsensusPolicy, choose_reviewer, consensus_credit_awards, resolve_scores,
        };
""",
    ),
    (
        """            assert!(consensus_credit_awards(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                result,
                policy,
            )
            .is_empty());
""",
        """            assert!(
                consensus_credit_awards(
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    result,
                    policy,
                )
                .is_empty()
            );
""",
    ),
    (
        """            assert_eq!(choose_reviewer(report, reporter, &candidates, 7).unwrap(), eligible);
""",
        """            assert_eq!(
                choose_reviewer(report, reporter, &candidates, 7).unwrap(),
                eligible
            );
""",
    ),
    (
        "from typing import Mapping\n",
        "from typing import Mapping\n\nfrom augment_elenkos_fleet_20260819 import augment_repository_files\n",
    ),
    (
        "    files = BUILDERS[spec.kind](spec, pins or {}, mode)\n",
        "    files = augment_repository_files(spec, BUILDERS[spec.kind](spec, pins or {}, mode), mode)\n",
    ),
)

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


def patch_materialized_contracts(root: Path) -> None:
    for relative, replacements in MATERIALIZED_REPLACEMENTS.items():
        path = root / relative
        text = path.read_text(encoding="utf-8")
        changed = False
        for old, new, expected_count in replacements:
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
    target = root / "scripts/ops/elenkos_fleet_spec_20260819.py"
    augment = root / "scripts/ops/augment_elenkos_fleet_20260819.py"

    augment_raw = augment.read_bytes()
    augment_digest = digest(augment_raw)
    if augment_digest != AUGMENT_SHA256:
        raise RuntimeError(
            f"refusing unreviewed augmentation source: expected {AUGMENT_SHA256}, got {augment_digest}"
        )

    raw = target.read_bytes()
    current = digest(raw)
    if current == PATCHED_SHA256:
        status = "already-applied"
    elif current == ORIGINAL_SHA256:
        text = raw.decode("utf-8")
        for old, new in REPLACEMENTS:
            count = text.count(old)
            if count != 1:
                raise RuntimeError(f"expected one generator patch target, found {count}")
            text = text.replace(old, new)

        patched = text.encode("utf-8")
        actual = digest(patched)
        if actual != PATCHED_SHA256:
            raise RuntimeError(f"patched generator digest mismatch: {actual}")
        target.write_bytes(patched)
        current = actual
        status = "applied"
    else:
        raise RuntimeError(
            f"refusing to patch unexpected generator: expected {ORIGINAL_SHA256} or "
            f"{PATCHED_SHA256}, got {current}"
        )

    patch_materialized_contracts(root)
    print(
        f"ELENKOS_PAYLOAD_PATCHED sha256={current} augment_sha256={augment_digest} "
        f"visibility=private status={status}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
