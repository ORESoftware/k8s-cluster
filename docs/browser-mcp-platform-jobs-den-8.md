# DEN-8 Browser MCP platform-jobs workflow

This change set introduces a dedicated `platform-jobs` workflow for authorized job-application automation without broadening the existing `fiducia-applications` workflow.

## Required ATS hosts

```text
boards.greenhouse.io
job-boards.greenhouse.io
greenhouse.io
jobs.lever.co
lever.co
jobs.ashbyhq.com
ashbyhq.com
jobs.smartrecruiters.com
smartrecruiters.com
myworkdayjobs.com
workday.com
apply.workable.com
workable.com
jobs.jobvite.com
jobvite.com
careers-page.com
click.appcast.io
to.indeed.com
www.indeed.com
www.ziprecruiter.com
```

## Invariants

- `fiducia-applications` remains unchanged.
- `BROWSER_MCP_DEFAULT_WORKFLOW` remains `fiducia-applications`.
- `platform-jobs` must be explicitly selected by callers.
- `BROWSER_MCP_ALLOWED_DOMAINS` and `BROWSER_AGENT_ALLOWED_DOMAINS` remain byte-for-byte aligned.
- CAPTCHA detection stays enabled while auto-solving remains disabled.
- Private-network access, URL credentials, sensitive headers, and arbitrary domains remain blocked.
- MFA, payments, signatures, legal attestations, compensation commitments, and consequential final submissions remain manual boundaries.

## Validation plan

1. Add the ATS hosts to both process-level ceilings.
2. Add `platform-jobs` to `BROWSER_MCP_WORKFLOW_ALLOWLISTS_JSON` with only those ATS hosts.
3. Add tests that parse both manifests and assert aligned ceilings.
4. Assert the existing `fiducia-applications` array is unchanged.
5. Assert `platform-jobs` excludes unrelated domains.
6. Verify `initialize`, `tools/list`, `browser_state`, and harmless `browser_act` navigation after rollout.
7. Verify an off-profile domain is denied.
8. Reconcile AWS and Hetzner ArgoCD deployments.
9. Refresh the ChatGPT custom app so `browser_act` and `browser_state` are visible.

References DEN-8
