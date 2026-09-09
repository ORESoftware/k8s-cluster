# DEN-8 Browser MCP platform-jobs workflow

This change set introduced a dedicated `platform-jobs` workflow for authorized job-application automation without broadening the existing `fiducia-applications` workflow. Current hardening and ownership are tracked under DEN-256.

## Reviewed ATS roots

```text
greenhouse.io
lever.co
ashbyhq.com
myworkdayjobs.com
workday.com
smartrecruiters.com
icims.com
jobvite.com
workable.com
bamboohr.com
recruitee.com
applytojob.com
ats.rippling.com
breezy.hr
jobscore.com
candidateportalin.ceipal.com
candidateportalnew.ceipal.com
```

The Browser MCP hostname matcher admits subdomains of a reviewed root. Rippling and CEIPAL use exact candidate-facing hosts. Historical entries for Indeed, ZipRecruiter, Appcast redirectors, generic careers-page hosts, and Google document/static hosts are not part of `platform-jobs` and must not be reintroduced by an overlay.

## Invariants

- `fiducia-applications` remains unchanged.
- `BROWSER_MCP_DEFAULT_WORKFLOW` remains `fiducia-applications`.
- `platform-jobs` must be explicitly selected by callers.
- `BROWSER_MCP_ALLOWED_DOMAINS` and `BROWSER_AGENT_ALLOWED_DOMAINS` remain byte-for-byte aligned across base manifests and applied overlays.
- CAPTCHA detection stays enabled while auto-solving remains disabled.
- Private-network access, URL credentials, sensitive headers, and arbitrary domains remain blocked.
- MFA, payments, signatures, legal attestations, compensation commitments, and consequential final submissions remain manual boundaries.
- The TypeSpec and Draft-2020-12 JSON Schema policy sources remain independent peer authorities and must pass pinned TJSV parity/differential checks.

## Validation plan

1. Keep the reviewed ATS roots in both process-level ceilings.
2. Keep `platform-jobs` restricted to the exact reviewed roots.
3. Compare base manifests and Kustomize overlays so an overlay cannot widen or stale the production policy.
4. Validate the independently authored TypeSpec and JSON Schema authorities with `ORESoftware/typespec-json-schema-validator` pinned to an exact commit.
5. Make Python and TypeScript consumers read the same reviewed `BrowserWorkflowPolicy` instance.
6. Assert the existing `fiducia-applications` and `appointments` profiles are preserved by overlays.
7. Verify `initialize`, `tools/list`, `browser_state`, and harmless `browser_act` navigation after rollout.
8. Verify marketplace, redirector, off-profile, webmail, identity-login, and payment hosts are denied.
9. Reconcile AWS and Hetzner ArgoCD deployments.
10. Refresh the ChatGPT custom app only after the deployed exact revision passes the live canary.

References DEN-8 and DEN-256.
