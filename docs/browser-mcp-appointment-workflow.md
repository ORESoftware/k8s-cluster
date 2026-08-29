# Browser MCP appointment workflow

Linear: DEN-522; parent DEN-258

`appointments` is a server-defined Browser MCP workflow for explicitly approved booking links discovered through the connected Gmail boundary.

## Reviewed host set

- `cal.com`
- `calendly.com`

The roots were selected from actual recent booking links in the connected inbox without persisting message content, message identifiers, or participant details in source control. Root entries permit their own subdomains. Google Meet is a meeting destination, not a booking-form host, and is not part of this profile.

## Safety boundary

- Gmail search, reading, drafting, and sending remain in the Gmail connector.
- DEN-258 provenance and risk assessment must approve the exact external link before Browser MCP opens it.
- Gmail, Google account login, LinkedIn, Outlook, and generic URL shorteners remain outside the navigation ceiling.
- Ordinary name, email, timezone, date, and time controls may be completed.
- CAPTCHA, MFA, payment, signatures, legal attestations, and sensitive fields remain hard stops.
- Final booking confirmation is consequential and requires the current page revision, pending action digest, and explicit user approval.
- Off-profile scheduling hosts require a reviewed code change; callers cannot widen the profile.

## Contract

`remote/tests/general/browser-mcp-appointment-profile.test.ts` parses the deployed workflow JSON, requires `appointments` to equal the reviewed two-host set, proves prohibited hosts remain absent, and requires the Rust MCP and Playwright/Selenium process ceilings to be byte-for-byte identical.
