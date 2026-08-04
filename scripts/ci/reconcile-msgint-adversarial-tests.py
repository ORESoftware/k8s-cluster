#!/usr/bin/env python3
"""Update the current-dev adversarial tests to the hardened planner contract.

This helper is consumed by the one-shot branch finalizer and removed before the
product-only commit. Every replacement is exact and cardinality checked so a
future source drift fails closed instead of silently rewriting another test.
"""

from pathlib import Path

PATH = Path("remote/deployments/gha-clone-server-rs/tests/planner_adversarial.rs")
CHECKOUT_SHA = "3d3c42e5aac5ba805825da76410c181273ba90b1"
SETUP_NODE_SHA = "820762786026740c76f36085b0efc47a31fe5020"


def replace_once(source: str, old: str, new: str, label: str) -> str:
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return source.replace(old, new, 1)


def main() -> None:
    source = PATH.read_text(encoding="utf-8")

    source = replace_once(
        source,
        '        "secret-bearing env/with values are unsupported",\n',
        '        "secret-bearing step environments are unsupported",\n'
        '        "secret-bearing setup inputs are unsupported",\n',
        "unsupported-step diagnostics",
    )

    source = replace_once(
        source,
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
        .any(|reason| reason.contains("secret-bearing setup inputs are unsupported")));
    for id in ["github_token", "oidc"] {
        assert!(job(&plan, id)
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("secret-bearing step environments are unsupported")));
    }
''',
        "secret-context diagnostics",
    )

    source = replace_once(
        source,
        "      - uses: ACTIONS/CHECKOUT@abc\n"
        "      - uses: Actions/Setup-Node@abc\n",
        f"      - uses: ACTIONS/CHECKOUT@{CHECKOUT_SHA}\n"
        f"      - uses: Actions/Setup-Node@{SETUP_NODE_SHA}\n",
        "immutable setup action fixtures",
    )

    PATH.write_text(source, encoding="utf-8")


if __name__ == "__main__":
    main()
