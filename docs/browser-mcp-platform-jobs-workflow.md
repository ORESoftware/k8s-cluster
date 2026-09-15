# Browser MCP platform-jobs workflow

Linear: DEN-256

`platform-jobs` is a server-defined Browser MCP workflow for approved applicant-tracking systems. The caller selects the profile by sending `workflow_id: "platform-jobs"`; it cannot add or widen domains.

## Reviewed hostname set

- `greenhouse.io`
- `lever.co`
- `ashbyhq.com`
- `myworkdayjobs.com`
- `workday.com`
- `smartrecruiters.com`
- `icims.com`
- `jobvite.com`
- `workable.com`
- `bamboohr.com`
- `recruitee.com`
- `applytojob.com`
- `ats.rippling.com`
- `breezy.hr`
- `jobscore.com`
- `candidateportalin.ceipal.com`
- `candidateportalnew.ceipal.com`

Root hostnames permit their own subdomains. The Rippling and CEIPAL entries are exact candidate-facing hosts; Breezy and JobScore use reviewed provider roots. Broad job marketplaces, webmail, identity-provider login hosts, arbitrary company sites, filing sites, redirectors, and payment hosts are deliberately excluded. A company-hosted careers page requires a separate reviewed profile change rather than caller-supplied navigation permission.

## Safety boundary

- Search and email triage stay outside Browser MCP.
- The worker blocks SSN/tax identifiers, bank and card fields, MFA/OTP/PIN fields, and literal credentials.
- CAPTCHA and MFA remain human completion points.
- Uploads accept only bounded inline content or operator-staged opaque tokens.
- Explicit submit actions require the revision-bound action digest and `user_explicitly_approved: true`.
- No demographic, disability, signature, legal-attestation, compensation-commitment, or final-submit decision is inferred from a page.
- The Rust MCP ceiling and the Playwright/Selenium worker ceiling must remain byte-for-byte identical.

## Contract authorities

`contracts/browser-mcp-platform-jobs/main.tsp` and `contracts/browser-mcp-platform-jobs/authored.schema.json` are independently authored peer authorities. Neither is generated from the other. CI pins `ORESoftware/typespec-json-schema-validator` to an exact Git commit and uses the TypeSpec-emitted JSON Schema only as comparison evidence.

The valid `BrowserWorkflowPolicy` instance is shared by the Python manifest/overlay test and a TypeScript consumer test. Negative instances require both authorities to reject marketplace widening, disabled explicit approval, and CAPTCHA automation.

## Validation contract

`tests/browser_mcp_policy_test.py` compares the base Rust MCP and browser-worker ceilings with both Kustomize overlay files, verifies that the overlay cannot widen or stale the reviewed base, and reads the canonical policy instance for the exact `platform-jobs` roots and safety flags.

`remote/tests/general/browser-mcp-platform-jobs-contract.test.ts` independently consumes the same policy instance and checks the Rust/TypeScript runtime boundary. The existing `remote/tests/general/browser-mcp-exposure.test.ts` continues to ratchet the public OAuth deployment, GitOps registration, and browser safety posture.

The Browser MCP policy workflow runs all of those focused contract checks plus pinned TJSV parity and differential validation.

## Adding another ATS

1. Verify the official ATS hostname and redirect chain.
2. Update both independent contract authorities and the reviewed valid instance.
3. Add only the minimum hostname roots to both process ceilings and `platform-jobs`.
4. Update negative/positive corpus evidence where the safety boundary changes.
5. Run pinned TJSV, Python manifest/overlay checks, the TypeScript consumer check, Browser MCP exposure checks, and an inert sandbox flow.
6. Do not add Gmail, Google account login, LinkedIn, Indeed, ZipRecruiter, arbitrary redirectors, or URL shorteners.
