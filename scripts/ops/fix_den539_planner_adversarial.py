#!/usr/bin/env python3
"""Refresh stale planner-adversarial assertions without weakening production policy."""

from __future__ import annotations

import subprocess
from pathlib import Path

PATH = Path("remote/deployments/gha-clone-server-rs/tests/planner_adversarial.rs")
EXPECTED_BLOB = "6af8e5f565de8ed4dd165e670e34c248694a790e"
CHECKOUT_SHA = "3d3c42e5aac5ba805825da76410c181273ba90b1"
SETUP_NODE_SHA = "820762786026740c76f36085b0efc47a31fe5020"


def replace_once(source: str, old: str, new: str, label: str) -> str:
    if source.count(old) != 1:
        raise SystemExit(f"{label} anchor was not unique")
    return source.replace(old, new, 1)


def main() -> None:
    source = PATH.read_text(encoding="utf-8")
    if (
        f"ACTIONS/CHECKOUT@{CHECKOUT_SHA}" in source
        and "secret-bearing setup inputs are unsupported" in source
        and "secret-bearing step environments are unsupported" in source
    ):
        print("planner adversarial assertions are already current")
        return

    observed = subprocess.check_output(
        ["git", "hash-object", str(PATH)], text=True
    ).strip()
    if observed != EXPECTED_BLOB:
        raise SystemExit(
            f"refusing drifted {PATH}: expected {EXPECTED_BLOB}, observed {observed}"
        )

    source = replace_once(
        source,
        "      - uses: actions/setup-node@abc\n        env:\n",
        f"      - uses: actions/setup-node@{SETUP_NODE_SHA}\n        env:\n",
        "unsupported-step setup action",
    )
    source = replace_once(
        source,
        '''        "shell is unsupported",
        "secret-bearing env/with values are unsupported",
''',
        '''        "shell is unsupported",
        "secret-bearing step environments are unsupported",
        "secret-bearing setup inputs are unsupported",
''',
        "specific step-secret reasons",
    )

    source = replace_once(
        source,
        "      - uses: actions/setup-node@abc\n        with:\n          token: '${{secrets.NPM_TOKEN}}'\n",
        f"      - uses: actions/setup-node@{SETUP_NODE_SHA}\n        with:\n          token: '${{{{secrets.NPM_TOKEN}}}}'\n",
        "compact-secret immutable action",
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
        "specific secret-context assertions",
    )

    source = replace_once(
        source,
        '''      - uses: ACTIONS/CHECKOUT@abc
      - uses: Actions/Setup-Node@abc
''',
        f'''      - uses: ACTIONS/CHECKOUT@{CHECKOUT_SHA}
      - uses: Actions/Setup-Node@{SETUP_NODE_SHA}
''',
        "case-insensitive immutable actions",
    )

    PATH.write_text(source, encoding="utf-8")
    print("planner adversarial assertions refreshed for immutable-action policy")


if __name__ == "__main__":
    main()
