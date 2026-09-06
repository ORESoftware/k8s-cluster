#!/usr/bin/env bash
# Re-apply chmod a-w on frozen generated files.
# Git only stores the executable bit, so clones come back writable (644).
# Generators should freeze after write; this script does the same after checkout.
set -euo pipefail
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
exec python3 "$ROOT/scripts/check-generated-contract.py" --root "$ROOT" --freeze --require-readonly "$@"
