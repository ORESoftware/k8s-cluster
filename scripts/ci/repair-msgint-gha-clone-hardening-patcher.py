from pathlib import Path

# Touch point for the exact-branch one-shot finalizer. The workflow deletes
# this helper after the reviewed patch and all validation gates succeed.
path = Path("scripts/ci/apply-msgint-gha-clone-hardening.py")
source = path.read_text(encoding="utf-8")

# Python would otherwise interpret the Rust character literal '\n' inside the
# patcher's non-raw triple-quoted strings. Double the source backslash so the
# generated Rust still contains exactly `combined.push('\n');`.
needle = "combined.push('\\n');"
replacement = "combined.push('\\\\n');"
count = source.count(needle)
if count != 4:
    raise RuntimeError(f"expected four Rust newline anchors, found {count}")
source = source.replace(needle, replacement)

for marker in [
    "run_msgint_profile_smoke",
    "node-hardened-test",
    "=https://github.com/messaging-intel/msgint-connectors.git",
    "exact reviewed command sequence",
]:
    if marker not in source:
        raise RuntimeError(f"required hardening marker is missing: {marker}")

path.write_text(source, encoding="utf-8")
