#!/usr/bin/env python3
"""Execute the account-token preflight with permission-aware Markdown evidence."""
from __future__ import annotations

import importlib.util
from pathlib import Path
from typing import Any

ADAPTER_PATH = Path(__file__).with_name("canonical-account-token-preflight.py")
SPEC = importlib.util.spec_from_file_location(
    "canonical_account_token_preflight_adapter", ADAPTER_PATH
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("failed to load Canonical account-token adapter")
ADAPTER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ADAPTER)


def route_state(route: dict[str, Any]) -> str:
    if route.get("readable") is False:
        return "unreadable (missing Workers Routes Read)"
    if route.get("conflict") is True:
        return f"conflict (owned by `{route.get('script')}`)"
    if route.get("exists") is True:
        return f"present (owned by `{route.get('script')}`)"
    if route.get("exists") is False:
        return "absent"
    return "unknown"


def dns_state(item: dict[str, Any]) -> str:
    if item.get("readable") is False:
        return "unreadable (missing DNS Read)"
    if item.get("exists") is False:
        return "absent"
    if item.get("exists") is True:
        record = item.get("record") or {}
        return (
            f"`{record.get('type')}`, proxied=`{record.get('proxied')}`, "
            "origin redacted"
        )
    return "unknown"


def markdown_report(evidence: dict[str, Any]) -> str:
    lines = [
        "# Canonical control-plane read-only preflight",
        "",
        f"Generated: `{evidence['generated_at']}`",
        "",
        "## Safety",
        "",
        "- Production Canonical GitHub source writes: `false`",
        "- Cloudflare writes: `false`",
        "- DNS writes: `false`",
        "- R2 access or writes: `false`",
        "- Kubernetes, database, secret-store, or Google-model writes: `false`",
        "- `canonical-cloud-test` repository writes: `true` (exact allowlist only)",
        "",
    ]

    cloudflare = evidence.get("cloudflare") or {}
    if cloudflare:
        worker = cloudflare.get("worker") or {}
        lines.extend(
            [
                "## Cloudflare inventory",
                "",
                f"- Token family: `{(cloudflare.get('token') or {}).get('family')}`",
                f"- Token active: `{(cloudflare.get('token') or {}).get('status') == 'active'}`",
                f"- Reviewed account hash: `{(cloudflare.get('account') or {}).get('id_sha256')}`",
                f"- Zone: `{(cloudflare.get('zone') or {}).get('name')}` "
                f"(`{(cloudflare.get('zone') or {}).get('status')}`)",
                f"- Worker script inventory readable: `{worker.get('script_inventory_readable')}`",
                f"- Exact Worker present: `{worker.get('exists')}`",
                "",
                "### Exact routes",
                "",
            ]
        )
        for route in cloudflare.get("routes", []):
            lines.append(f"- `{route.get('pattern')}` — {route_state(route)}")
        lines.extend(["", "### Exact DNS names", ""])
        for item in cloudflare.get("dns", []):
            lines.append(f"- `{item.get('name')}` — {dns_state(item)}")
        lines.append("")

    github = evidence.get("github") or {}
    if github:
        lines.extend(
            [
                "## GitHub isolated staging",
                "",
                f"- Test organization: `{github.get('test_org')}`",
                f"- Membership: `{(github.get('membership') or {}).get('state')}` / "
                f"`{(github.get('membership') or {}).get('role')}`",
                f"- Exact repositories verified: `{len(github.get('repositories', []))}`",
                "",
                "### Workflow results",
                "",
            ]
        )
        for run in github.get("workflow_runs", []):
            lines.append(
                f"- `{run.get('repository')}` — `{run.get('conclusion')}` "
                f"({run.get('url')})"
            )
        lines.append("")

    lines.extend(["## Blocking gates", ""])
    blockers = evidence.get("blockers") or []
    if blockers:
        lines.extend(f"- {blocker}" for blocker in blockers)
    else:
        lines.append("- None recorded by this preflight.")

    errors = evidence.get("errors") or []
    if errors:
        lines.extend(["", "## Execution errors", ""])
        lines.extend(f"- {error}" for error in errors)
    return "\n".join(lines).rstrip() + "\n"


ADAPTER.CORE.markdown_report = markdown_report


def main() -> int:
    return ADAPTER.main()


if __name__ == "__main__":
    raise SystemExit(main())
