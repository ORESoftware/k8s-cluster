#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import sys
from pathlib import Path

ORIGINAL_SHA256 = "de511f13abd079437860a826c4e0dea50bfea90d15c76014432acb4b926e4016"
PATCHED_SHA256 = "3307d64be18e35aa12a408725f343d62741921a95c789408f2d6b9c5db3618d7"

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
)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: patch_elenkos_fleet_payload_20260819.py ROOT")
    root = Path(sys.argv[1]).resolve()
    target = root / "scripts/ops/elenkos_fleet_spec_20260819.py"
    raw = target.read_bytes()
    current = digest(raw)
    if current == PATCHED_SHA256:
        print(f"ELENKOS_PAYLOAD_PATCHED sha256={current} status=already-applied")
        return 0
    if current != ORIGINAL_SHA256:
        raise RuntimeError(
            f"refusing to patch unexpected generator: expected {ORIGINAL_SHA256}, got {current}"
        )

    text = raw.decode("utf-8")
    for old, new in REPLACEMENTS:
        count = text.count(old)
        if count != 1:
            raise RuntimeError(f"expected one formatting target, found {count}")
        text = text.replace(old, new)

    patched = text.encode("utf-8")
    actual = digest(patched)
    if actual != PATCHED_SHA256:
        raise RuntimeError(f"patched generator digest mismatch: {actual}")
    target.write_bytes(patched)
    print(f"ELENKOS_PAYLOAD_PATCHED sha256={actual} status=applied")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
