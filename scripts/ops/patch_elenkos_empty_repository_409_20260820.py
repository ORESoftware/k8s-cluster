#!/usr/bin/env python3
"""Apply idempotent compatibility fixes needed by the Elenkos publisher.

The reviewed fleet materializer already invokes
`patch_elenkos_empty_repository_main_ref_20260820.py`. This wrapper remains for
older one-shot workflows and delegates to that fail-closed implementation.
It also repairs Python's dynamic-module registration boundary in the partial
bootstrap recovery tool: dataclasses require the module to exist in
``sys.modules`` while the generated specification is evaluated.
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

CANONICAL = Path("scripts/ops/patch_elenkos_empty_repository_main_ref_20260820.py")
PUBLISHER = Path("scripts/ops/publish_elenkos_fleet_20260819.py")
RECOVERY = Path("scripts/ops/recover_elenkos_partial_bootstrap_20260820.py")

RECOVERY_IMPORT_OLD = "import re\nimport stat\n"
RECOVERY_IMPORT_NEW = "import re\nimport stat\nimport sys\n"
RECOVERY_LOAD_OLD = """    module = importlib.util.module_from_spec(spec)\n    spec.loader.exec_module(module)\n    return module\n"""
RECOVERY_LOAD_NEW = """    module = importlib.util.module_from_spec(spec)\n    sys.modules[spec.name] = module\n    try:\n        spec.loader.exec_module(module)\n    except Exception:\n        sys.modules.pop(spec.name, None)\n        raise\n    return module\n"""


def patch_recovery(path: Path) -> str:
    if not path.is_file():
        raise RuntimeError(f"missing partial-bootstrap recovery tool: {RECOVERY}")
    source = path.read_text(encoding="utf-8")

    old_imports = source.count(RECOVERY_IMPORT_OLD)
    new_imports = source.count(RECOVERY_IMPORT_NEW)
    old_loads = source.count(RECOVERY_LOAD_OLD)
    new_loads = source.count(RECOVERY_LOAD_NEW)

    if old_imports == 0 and new_imports == 1 and old_loads == 0 and new_loads == 1:
        return "already-applied"
    if old_imports != 1 or new_imports != 0 or old_loads != 1 or new_loads != 0:
        raise RuntimeError(
            "refusing unexpected recovery source: "
            f"old_imports={old_imports} new_imports={new_imports} "
            f"old_loads={old_loads} new_loads={new_loads}"
        )

    source = source.replace(RECOVERY_IMPORT_OLD, RECOVERY_IMPORT_NEW, 1)
    source = source.replace(RECOVERY_LOAD_OLD, RECOVERY_LOAD_NEW, 1)
    path.write_text(source, encoding="utf-8")
    return "applied"


def main(argv: list[str]) -> int:
    if len(argv) > 2:
        raise SystemExit("usage: patch_elenkos_empty_repository_409_20260820.py [root]")
    root = Path(argv[1] if len(argv) == 2 else ".").resolve()
    canonical = root / CANONICAL
    publisher = root / PUBLISHER
    recovery = root / RECOVERY
    if not canonical.is_file():
        raise RuntimeError(f"missing canonical patcher: {CANONICAL}")
    if not publisher.is_file():
        raise RuntimeError(f"missing materialized publisher: {PUBLISHER}")

    subprocess.run(
        [sys.executable, str(canonical), "--publisher", str(publisher)],
        cwd=root,
        check=True,
    )
    recovery_status = patch_recovery(recovery)
    print(
        "ELENKOS_COMPAT_PATCHES_VERIFIED "
        f"empty_repository_main_ref=true recovery_module_registration={recovery_status}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
