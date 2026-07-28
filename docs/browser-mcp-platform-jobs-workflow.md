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

Root hostnames permit their own subdomains. The Rippling and CEIPAL entries are exact candidate-facing hosts; Breezy and JobScore use reviewed provider roots. Broad job marketplaces, webmail, identity-provider login hosts, arbitrary company sites, filing sites, and payment hosts are deliberately excluded. A company-hosted careers page requires a separate reviewed profile change rather than caller-supplied navigation permission.

## Safety boundary

- Search and email triage stay outside Browser MCP.
- The worker blocks SSN/tax identifiers, bank and card fields, MFA/OTP/PIN fields, and literal credentials.
- CAPTCHA and MFA remain human completion points.
- Uploads accept only bounded inline content or operator-staged opaque tokens.
- Explicit submit actions require the revision-bound action digest and `user_explicitly_approved: true`.
- No demographic, disability, signature, legal-attestation, compensation-commitment, or final-submit decision is inferred from a page.
- The Rust MCP ceiling and the Playwright/Selenium worker ceiling must remain byte-for-byte identical.

## Validation contract

`remote/tests/general/browser-mcp-exposure.test.ts` parses the deployed workflow JSON, compares `platform-jobs` with the reviewed domain constant, verifies the Rust MCP and worker ceilings remain identical, and asserts that broad marketplaces and Google/mail login hosts remain excluded.

## Adding another ATS

1. Verify the official ATS hostname and redirect chain.
2. Add only the minimum hostname roots to both process ceilings and `platform-jobs`.
3. Update the reviewed-domain contract.
4. Run the Browser MCP exposure test and an inert sandbox flow.
5. Do not add Gmail, Google account login, LinkedIn, Indeed, ZipRecruiter, arbitrary redirectors, or URL shorteners.
