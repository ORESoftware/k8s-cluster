"""Literal-safe workflow and current-router semantic resolver adapters."""

from __future__ import annotations

import den977_semantic_resolvers as base


def _contract_blocks() -> tuple[str, str]:
    slash = chr(92)
    old = "\n".join(
        [
            "      - name: Validate deployment, routing, execution, webhook, activation, and StreemPilot boundaries",
            "        working-directory: remote/tests",
            "        run: |",
            f"          pnpm exec tsx --test {slash}",
            f"            general/gha-clone-server-config.test.ts {slash}",
            f"            general/gha-clone-webhook-config.test.ts {slash}",
            "            general/gha-clone-streempilot-config.test.ts",
            "          node --test general/gha-executor-router-activation.test.mjs",
        ]
    )
    new = "\n".join(
        [
            "      - name: Validate deployment, routing, execution, webhook, activation, Messaging Intel, and StreemPilot boundaries",
            "        working-directory: remote/tests",
            "        run: |",
            f"          pnpm exec tsx --test {slash}",
            f"            general/gha-clone-server-config.test.ts {slash}",
            f"            general/gha-clone-webhook-config.test.ts {slash}",
            f"            general/gha-clone-msgint-config.test.ts {slash}",
            "            general/gha-clone-streempilot-config.test.ts",
            "          node --test general/gha-executor-router-activation.test.mjs",
        ]
    )
    return old, new


def _credential_scan_blocks() -> tuple[str, str]:
    slash = chr(92)
    old = "\n".join(
        [
            f"            docs/gha-executor-router-activation.md {slash}",
            "            docs/streempilot-ci-continuity.md; then",
        ]
    )
    new = "\n".join(
        [
            f"            docs/gha-executor-router-activation.md {slash}",
            f"            docs/gha-profile-repository-admission.md {slash}",
            "            docs/streempilot-ci-continuity.md; then",
        ]
    )
    return old, new


def resolve_workflow(current_data: bytes | None, reviewed_data: bytes | None) -> bytes:
    saved = base.replace_exact
    contract_old, contract_new = _contract_blocks()
    scan_old, scan_new = _credential_scan_blocks()

    def replace_exact(
        source: str,
        old: str,
        new: str,
        label: str,
        *,
        count: int = 1,
    ) -> str:
        if label == "continuity workflow contract union":
            old, new = contract_old, contract_new
        elif label == "continuity workflow credential-scan documentation union":
            old, new = scan_old, scan_new
        return saved(source, old, new, label, count=count)

    base.replace_exact = replace_exact
    try:
        return base.resolve_workflow(current_data, reviewed_data)
    finally:
        base.replace_exact = saved


_CURRENT_ROUTER_TEST = (
    "current router modules preserve request assignment and provider pinning"
)
_LEGACY_ASSERTION_SENTINEL = (
    "// executor router code and live tests preserve no-duplicate provider pinning\n"
)


def resolve_server_contract(
    current_data: bytes | None,
    reviewed_data: bytes | None,
) -> bytes:
    current = base.text(current_data, "current continuity TypeScript contract")
    if _CURRENT_ROUTER_TEST not in current:
        raise SystemExit("current router-pinning process contract was not found")
    if _LEGACY_ASSERTION_SENTINEL.strip() in current:
        raise SystemExit("legacy router assertion sentinel already exists in source")

    augmented = (_LEGACY_ASSERTION_SENTINEL + current).encode()
    resolved = base.resolve_server_contract(augmented, reviewed_data).decode()
    if not resolved.startswith(_LEGACY_ASSERTION_SENTINEL):
        raise SystemExit("router assertion sentinel did not remain at the source boundary")
    resolved = resolved[len(_LEGACY_ASSERTION_SENTINEL) :]

    if _CURRENT_ROUTER_TEST not in resolved:
        raise SystemExit("semantic union lost the current router-pinning process contract")
    if _LEGACY_ASSERTION_SENTINEL.strip() in resolved:
        raise SystemExit("legacy router assertion sentinel leaked into product source")
    return resolved.encode()


base.RESOLVERS[base.SERVER_CONTRACT_PATH] = resolve_server_contract
