from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


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

# The hardening intentionally tightened three planner contracts. Keep the
# adversarial tests aligned with the stricter behavior rather than weakening the
# implementation to satisfy stale expectations.
test_path = Path("remote/deployments/gha-clone-server-rs/tests/planner_adversarial.rs")
tests = test_path.read_text(encoding="utf-8")
tests = replace_once(
    tests,
    '''        "shell is unsupported",
        "secret-bearing env/with values are unsupported",
''',
    '''        "shell is unsupported",
        "secret-bearing step environments are unsupported",
        "secret-bearing setup inputs are unsupported",
        "must use an exact 40-hex commit SHA",
''',
    "specific step rejection expectations",
)
tests = replace_once(
    tests,
    '''    for id in ["compact_secret", "github_token", "oidc"] {
        assert!(job(&plan, id)
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("secret-bearing env/with")));
    }
''',
    '''    assert!(job(&plan, "compact_secret")
        .independent_reasons
        .iter()
        .any(|reason| reason.contains("secret-bearing setup inputs")));
    for id in ["github_token", "oidc"] {
        assert!(job(&plan, id)
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("secret-bearing step environments")));
    }
''',
    "specific secret context expectations",
)
tests = replace_once(
    tests,
    '''      - uses: ACTIONS/CHECKOUT@abc
      - uses: Actions/Setup-Node@abc
''',
    '''      - uses: ACTIONS/CHECKOUT@0123456789abcdef0123456789abcdef01234567
      - uses: Actions/Setup-Node@0123456789abcdef0123456789abcdef01234567
''',
    "immutable mixed-case setup actions",
)
test_path.write_text(tests, encoding="utf-8")
