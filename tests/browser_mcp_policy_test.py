#!/usr/bin/env python3
from __future__ import annotations

import ast
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MCP_PATCH = ROOT / "remote/deployments/browser-mcp-rs/k8s/ec2/dd-browser-mcp-rs.platform-jobs.patch.yaml"
WORKER_PATCH = ROOT / "remote/argocd/dd-next-runtime/dd-web-scraper.platform-jobs.patch.yaml"


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


def main() -> None:
    mcp_text = MCP_PATCH.read_text(encoding="utf-8")
    worker_text = WORKER_PATCH.read_text(encoding="utf-8")

    mcp_ceiling = quoted_value(mcp_text, "BROWSER_MCP_ALLOWED_DOMAINS").split(",")
    worker_ceiling = quoted_value(worker_text, "BROWSER_AGENT_ALLOWED_DOMAINS").split(",")
    assert mcp_ceiling == worker_ceiling, "MCP and worker domain ceilings must stay byte-for-byte aligned"
    assert len(mcp_ceiling) == len(set(mcp_ceiling)), "domain ceiling contains duplicates"

    workflows = folded_json(mcp_text, "BROWSER_MCP_WORKFLOW_ALLOWLISTS_JSON")
    assert "platform-jobs" in workflows, "platform-jobs workflow is required"
    assert "fiducia-applications" in workflows, "existing Fiducia workflow must be preserved"

    ceiling = set(mcp_ceiling)
    for name, domains in workflows.items():
        assert set(domains) <= ceiling, f"workflow {name} exceeds the process-level ceiling"
        assert len(domains) == len(set(domains)), f"workflow {name} contains duplicate domains"

    ats_domains = {
        "boards.greenhouse.io",
        "job-boards.greenhouse.io",
        "greenhouse.io",
        "jobs.lever.co",
        "lever.co",
        "jobs.ashbyhq.com",
        "ashbyhq.com",
        "jobs.smartrecruiters.com",
        "smartrecruiters.com",
        "myworkdayjobs.com",
        "workday.com",
        "apply.workable.com",
        "workable.com",
        "jobs.jobvite.com",
        "jobvite.com",
        "careers-page.com",
        "click.appcast.io",
        "to.indeed.com",
        "www.indeed.com",
        "www.ziprecruiter.com",
    }
    assert ats_domains <= set(workflows["platform-jobs"]), "platform-jobs is missing an approved ATS host"
    assert not (ats_domains & set(workflows["fiducia-applications"])), (
        "ATS hosts must not leak into fiducia-applications"
    )

    forbidden = {
        "accounts.google.com",
        "mail.google.com",
        "login.microsoftonline.com",
        "paypal.com",
        "stripe.com",
    }
    assert not (forbidden & ceiling), "authentication, webmail, or payment hosts must remain blocked"

    print("Browser MCP policy alignment and profile isolation checks passed")


if __name__ == "__main__":
    main()
