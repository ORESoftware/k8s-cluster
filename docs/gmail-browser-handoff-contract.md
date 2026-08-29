# Gmail-to-Browser MCP handoff contract

Linear: DEN-258

Gmail remains a connected-data boundary. ChatGPT reads, searches, drafts, replies, and sends through the Gmail connector; Browser MCP must never automate Gmail or Google-account login.

When an approved external email link needs browser work, `browser_act` accepts a bounded `source_context` that contains only:

- mailbox alias (`personal` or `fiducia`), never mailbox credentials;
- opaque Gmail message and thread IDs, hashed before logging;
- sender and optional reply-to domains, not raw addresses;
- a complete enumerated risk assessment;
- the exact user-approved external URL;
- a maximum 15-minute provenance lifetime; and
- explicit user approval to open the link.

The Rust MCP validates the context before it contacts the private worker. It removes `source_context`, emits only hashed/redacted audit references, and requires the first browser navigation to equal the approved URL. The worker still independently enforces its hostname ceiling, HTTPS/public-address policy, sensitive-field classifier, CAPTCHA/MFA stops, and revision-bound final-submit confirmation.

## Fail-closed decisions

Navigation is denied for malformed or expired provenance, incomplete risk assessment, off-profile hosts, IP literals, URL credentials, non-default HTTPS ports, punycode hostnames, webmail/login hosts, and generic short-link hosts.

These signals are always denied even after link approval:

- lookalike domain;
- password, credential, OTP, or MFA request;
- remote-access software request;
- crypto or gift-card request;
- payment, bank, or card request;
- SSN or tax-ID request; and
- identity-document upload request.

Sender/reply-to mismatch, artificial urgency, and an unexpected attachment require a separate reviewed-risk confirmation. The supplied mismatch signal must agree with the supplied domains.

## Audit linkage

Audit events link the hashed Gmail message/thread references, hashed browser request reference, mailbox alias, sender/reply-to domains, selected workflow, target host, bounded expiry, and enumerated non-secret risk signals. Email subject/body text, recipient addresses, credentials, query contents, field values, and attachments are not logged.

## Required calling sequence

1. Read the complete Gmail thread through the connector.
2. Verify sender, reply-to, requested action, and target domain.
3. Complete the enumerated risk assessment.
4. Obtain explicit approval to open the exact external URL.
5. Call `browser_act` with a reviewed server-defined workflow and the bounded `source_context`.
6. Continue state/act iterations while respecting upload and consequential-action checkpoints.
7. Record the resulting confirmation reference and draft/send Gmail follow-up through the connector.
