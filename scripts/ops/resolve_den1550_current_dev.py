#!/usr/bin/env python3
"""Resolve the two reviewed DEN-1550 merge conflicts by semantic union."""

from __future__ import annotations

import re
from pathlib import Path

CONFLICT = re.compile(
    r"<<<<<<< HEAD\n(.*?)=======\n(.*?)>>>>>>> [^\n]+\n",
    re.DOTALL,
)


def union_lines(left: str, right: str) -> str:
    result: list[str] = []
    seen: set[str] = set()
    for line in (left + right).splitlines(keepends=True):
        if line not in seen:
            result.append(line)
            seen.add(line)
    return "".join(result)


def resolve_workflow() -> None:
    path = Path(".github/workflows/gha-clone-server.yml")
    source = path.read_text(encoding="utf-8")
    blocks = list(CONFLICT.finditer(source))
    if len(blocks) != 5:
        raise SystemExit(f"expected five workflow conflicts, found {len(blocks)}")

    contract_name = (
        "      - name: Validate deployment, routing, execution, webhook, "
        "activation, and Messaging Intel boundaries\n"
    )
    contract_body = "\n".join(
        [
            "            general/gha-clone-webhook-config.test.ts \\",
            "            general/gha-clone-msgint-config.test.ts",
            "          node --test general/gha-executor-router-activation.test.mjs",
            "      - name: Install pinned kubectl renderer",
            "        uses: azure/setup-kubectl@829323503d1be3d00ca8346e5391ca0b07a9ab0d # v5",
            "        with:",
            "          version: v1.32.2",
            "      - name: Render the complete continuity overlay",
            "        run: |",
            "          set -euo pipefail",
            '          rendered="${RUNNER_TEMP}/dd-next-runtime.yaml"',
            '          kubectl kustomize remote/argocd/dd-next-runtime >"$rendered"',
            '          test -s "$rendered"',
            "          grep -F 'name: dd-gha-clone-server' \"$rendered\"",
            "          grep -F 'name: dd-gha-executor-router' \"$rendered\"",
            "          test \"$(grep -c 'replicas: 0' \"$rendered\")\" -ge 2",
        ]
    ) + "\n"
    credential_scan = (
        "            docs/gha-executor-router-activation.md \\\n"
        "            docs/gha-profile-repository-admission.md; then\n"
    )

    def replacement(match: re.Match[str]) -> str:
        index = replacement.index
        replacement.index += 1
        left, right = match.group(1), match.group(2)
        if index in (0, 1):
            return union_lines(left, right)
        if index == 2:
            return contract_name
        if index == 3:
            return contract_body
        if index == 4:
            return credential_scan
        raise AssertionError(index)

    replacement.index = 0  # type: ignore[attr-defined]
    resolved = CONFLICT.sub(replacement, source)
    if any(marker in resolved for marker in ("<<<<<<<", "=======", ">>>>>>>")):
        raise SystemExit("workflow conflict markers remain")
    for required in (
        "general/gha-executor-router-activation.test.mjs",
        "general/gha-clone-msgint-config.test.ts",
        "docs/gha-executor-router-activation.md",
        "docs/gha-profile-repository-admission.md",
    ):
        if required not in resolved:
            raise SystemExit(f"resolved workflow omitted {required}")
    path.write_text(resolved, encoding="utf-8")


def resolve_typescript_contract() -> None:
    path = Path("remote/tests/general/gha-clone-server-config.test.ts")
    source = path.read_text(encoding="utf-8")
    blocks = list(CONFLICT.finditer(source))
    if len(blocks) != 1:
        raise SystemExit(f"expected one TypeScript conflict, found {len(blocks)}")

    # Current dev already contains its newer observability/router arrays and the
    # added genericPlannerPath declaration outside this conflict. Select only
    # the stable branch's test title and generic-planner read inside the marker.
    resolved = CONFLICT.sub(lambda match: match.group(2), source, count=1)
    if any(marker in resolved for marker in ("<<<<<<<", "=======", ">>>>>>>")):
        raise SystemExit("TypeScript conflict markers remain")
    required = (
        "const genericPlannerPath =",
        "const planner = read(genericPlannerPath);",
        "const observabilityPaths = [",
        "const routerSourcePaths = [",
        "const routerTestPaths = [",
    )
    for marker in required:
        if marker not in resolved:
            raise SystemExit(f"resolved TypeScript contract omitted {marker}")
    path.write_text(resolved, encoding="utf-8")


def main() -> None:
    resolve_workflow()
    resolve_typescript_contract()
    print("resolved DEN-1550 current-dev conflict set")


if __name__ == "__main__":
    main()
