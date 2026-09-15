# Browser MCP `platform-jobs` contract

Tracking: Linear `DEN-256`.

This directory keeps two independently authored, same-level authorities for the Browser MCP job-application policy:

- `main.tsp` — TypeSpec authority;
- `authored.schema.json` — JSON Schema Draft 2020-12 authority.

Neither source is generated from the other. CI pins `ORESoftware/typespec-json-schema-validator` to an exact Git commit and runs `tjsv check` so the TypeSpec-generated JSON Schema witness is comparison evidence only.

`instances/BrowserWorkflowPolicy/valid/platform-jobs.json` is the canonical reviewed runtime policy instance consumed by both Python and TypeScript tests. It fixes the approved ATS roots, blocked marketplace/redirector hosts, global auth/payment exclusions, protected field classes, explicit-submit approval boundary, revision-bound action digest, and human CAPTCHA/MFA completion points.

The `invalid/` corpus proves that both authorities reject marketplace widening, disabled explicit approval, and CAPTCHA automation.

Runtime rules remain fail-closed:

- caller-supplied domains cannot widen `platform-jobs`;
- root ATS entries cover that ATS vendor's subdomains through the existing runtime hostname matcher;
- broad marketplaces, redirectors, arbitrary company sites, webmail, identity login, and payment hosts are not part of `platform-jobs`;
- CAPTCHA and MFA are human completion points;
- sensitive identity/payment/authentication fields and consequential attestations are not inferred;
- a final submit requires the revision-bound digest and explicit user approval.
