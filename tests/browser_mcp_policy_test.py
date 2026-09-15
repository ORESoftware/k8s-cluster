#!/usr/bin/env python3
from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MCP_BASE = ROOT / "remote/deployments/browser-mcp-rs/k8s/ec2/dd-browser-mcp-rs.deployment.yaml"
MCP_PATCH = ROOT / "remote/deployments/browser-mcp-rs/k8s/ec2/dd-browser-mcp-rs.platform-jobs.patch.yaml"
WORKER_BASE = ROOT / "remote/argocd/dd-next-runtime/dd-web-scraper.deployment.yaml"
WORKER_PATCH = ROOT / "remote/argocd/dd-next-runtime/dd-web-scraper.platform-jobs.patch.yaml"
POLICY_INSTANCE = (
    ROOT
    / "contracts/browser-mcp-platform-jobs/instances/BrowserWorkflowPolicy/valid/platform-jobs.json"
)


def quoted_value(text: str, env_name: str) -> str:
    pattern = rf"- name: {re.escape(env_name)}\n\s+value: '([^']+)'"
    match = re.search(pattern, text)
    if not match:
        raise AssertionError(f"missing quoted value for {env_name}")
    return match.group(1)


def folded_json(text: str, env_name: str) -> dict[str, list[str]]:
    pattern = rf"- name: {re.escape(env_name)}\n\s+value: >-\n\s+(\{{.*\}})"
    match = re.search(pattern, text)
    if not match:
        raise AssertionError(f"missing JSON value for {env_name}")
    return json.loads(match.group(1))


def admitted(host: str, roots: list[str]) -> bool:
    return any(host == root or host.endswith(f".{root}") for root in roots)


def main() -> None:
    mcp_base_text = MCP_BASE.read_text(encoding="utf-8")
    mcp_patch_text = MCP_PATCH.read_text(encoding="utf-8")
    worker_base_text = WORKER_BASE.read_text(encoding="utf-8")
    worker_patch_text = WORKER_PATCH.read_text(encoding="utf-8")
    policy = json.loads(POLICY_INSTANCE.read_text(encoding="utf-8"))

    assert policy["workflowId"] == "platform-jobs"
    assert policy["requireExplicitApproval"] is True
    assert policy["requireRevisionBoundActionDigest"] is True
    assert policy["allowArbitraryCompanySites"] is False
    assert policy["allowMarketplaceNavigation"] is False
    assert policy["allowRedirectorNavigation"] is False
    assert policy["permitCaptchaAutomation"] is False
    assert set(policy["humanCompletionPoints"]) == {"captcha", "mfa"}

    required_blocked_fields = {
        "ssn",
        "tax_identifier",
        "bank_account",
        "payment_card",
        "mfa",
        "otp",
        "pin",
        "credential",
        "demographic",
        "disability",
        "signature",
        "legal_attestation",
        "compensation_commitment",
    }
    assert required_blocked_fields <= set(policy["blockedFieldClasses"])

    mcp_base_ceiling = quoted_value(mcp_base_text, "BROWSER_MCP_ALLOWED_DOMAINS").split(",")
    mcp_patch_ceiling = quoted_value(mcp_patch_text, "BROWSER_MCP_ALLOWED_DOMAINS").split(",")
    worker_base_ceiling = quoted_value(worker_base_text, "BROWSER_AGENT_ALLOWED_DOMAINS").split(",")
    worker_patch_ceiling = quoted_value(worker_patch_text, "BROWSER_AGENT_ALLOWED_DOMAINS").split(",")

    assert mcp_base_ceiling == worker_base_ceiling, "base MCP and worker ceilings must stay byte-for-byte aligned"
    assert mcp_patch_ceiling == worker_patch_ceiling, "overlay MCP and worker ceilings must stay byte-for-byte aligned"
    assert mcp_patch_ceiling == mcp_base_ceiling, "platform-jobs overlay must not widen or stale the reviewed base ceiling"
    assert len(mcp_base_ceiling) == len(set(mcp_base_ceiling)), "domain ceiling contains duplicates"

    base_workflows = folded_json(mcp_base_text, "BROWSER_MCP_WORKFLOW_ALLOWLISTS_JSON")
    patch_workflows = folded_json(mcp_patch_text, "BROWSER_MCP_WORKFLOW_ALLOWLISTS_JSON")
    expected_jobs = policy["allowedDomains"]

    assert base_workflows["platform-jobs"] == expected_jobs, "base platform-jobs profile drifted from contract"
    assert patch_workflows["platform-jobs"] == expected_jobs, "overlay platform-jobs profile drifted from contract"
    assert patch_workflows["fiducia-applications"] == base_workflows["fiducia-applications"], (
        "platform-jobs overlay must preserve fiducia-applications"
    )
    assert patch_workflows["appointments"] == base_workflows["appointments"], (
        "platform-jobs overlay must preserve appointments"
    )
    assert patch_workflows["benefactor-site"] == base_workflows["benefactor-site"]
    assert patch_workflows["smoke-test"] == base_workflows["smoke-test"]

    ceiling = mcp_base_ceiling
    for name, domains in base_workflows.items():
        assert set(domains) <= set(ceiling), f"workflow {name} exceeds the process-level ceiling"
        assert len(domains) == len(set(domains)), f"workflow {name} contains duplicate domains"

    for blocked in policy["blockedPlatformJobDomains"]:
        assert not admitted(blocked, expected_jobs), f"platform-jobs admits blocked host {blocked}"
    for blocked in policy["globallyBlockedDomains"]:
        assert not admitted(blocked, ceiling), f"process ceiling admits globally blocked host {blocked}"

    assert not (set(expected_jobs) & set(base_workflows["fiducia-applications"])), (
        "ATS hosts must not leak into fiducia-applications"
    )

    print("Browser MCP policy contract, overlay alignment, and workflow isolation checks passed")


if __name__ == "__main__":
    main()
