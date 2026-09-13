#!/usr/bin/env python3
"""Update current-dev tests to the hardened Messaging Intel contract.

This helper is consumed by the one-shot branch finalizer and removed before the
product-only commit. Every replacement is exact and cardinality checked so a
future source drift fails closed instead of silently rewriting another test.
"""

from pathlib import Path

ADVERSARIAL_PATH = Path(
    "remote/deployments/gha-clone-server-rs/tests/planner_adversarial.rs"
)
CONTRACT_PATH = Path("remote/tests/general/gha-clone-server-config.test.ts")
CHECKOUT_SHA = "3d3c42e5aac5ba805825da76410c181273ba90b1"
SETUP_NODE_SHA = "820762786026740c76f36085b0efc47a31fe5020"
CREATE_APP_ACTION_REVISION = "bcd2ba49218906704ab6c1aa796996da409d3eb1"


def replace_once(source: str, old: str, new: str, label: str) -> str:
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return source.replace(old, new, 1)


def reconcile_adversarial_tests() -> None:
    source = ADVERSARIAL_PATH.read_text(encoding="utf-8")

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

    ADVERSARIAL_PATH.write_text(source, encoding="utf-8")


def reconcile_workflow_contract() -> None:
    source = CONTRACT_PATH.read_text(encoding="utf-8")

    source = replace_once(
        source,
        "  assert.match(workflow, /run_msgint_profile_smoke/);\n",
        "  assert.match(workflow, /run_msgint_private_smoke/);\n"
        "  assert.match(workflow, /node-profile-hermetic-smoke:/);\n"
        "  assert.match(workflow, /msgint-private-profile-smoke:/);\n",
        "split Messaging Intel smoke names",
    )

    source = replace_once(
        source,
        "  assert.match(workflow, /create-github-app-token@/);\n",
        "  assert.match(\n"
        "    workflow,\n"
        f"    /actions\\/create-github-app-token@{CREATE_APP_ACTION_REVISION}/,\n"
        "  );\n"
        "  assert.match(workflow, /owner: messaging-intel/);\n"
        "  assert.match(workflow, /repositories: msgint-connectors/);\n"
        "  assert.match(workflow, /permission-contents: read/);\n",
        "repository-scoped App token contract",
    )

    source = replace_once(
        source,
        "  assert.doesNotMatch(workflow, /rm -rf|ghp_|github_pat_/);\n",
        "  assert.doesNotMatch(\n"
        "    workflow,\n"
        "    /rm -rf\\s+[\"']?\\$GITHUB_WORKSPACE|ghp_|github_pat_/,\n"
        "  );\n",
        "bounded cleanup and credential rejection",
    )

    CONTRACT_PATH.write_text(source, encoding="utf-8")


def main() -> None:
    reconcile_adversarial_tests()
    reconcile_workflow_contract()


if __name__ == "__main__":
    main()
