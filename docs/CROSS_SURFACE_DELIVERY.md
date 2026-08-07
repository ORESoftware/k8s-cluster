# Cross-surface delivery

Verified **2026-08-06**.

## Surfaces

- Rust customer web/BFF: `fiducia-cloud/fiducia-customer.rs`
- Flutter Android/iOS, Flutter Web/mobile web, and Flutter desktop: `fiducia-cloud/fiducia-flutter` — planned
- Rust desktop: `fiducia-cloud/fiducia-desktop.rs` — planned native GPUI/no-WebView application
- Shared contracts: Fiducia interfaces, generated clients, account/org/API-key/activity/session schemas, route types, auth fixtures, and conformance tests

The customer and admin planes remain separate. A customer-surface change must not silently gain operator capabilities, accept the admin cookie, or depend on the admin database.

## Judgment-based propagation

Evaluate Flutter mobile, Flutter Web, Flutter desktop, GPUI desktop, and shared contracts for every user-visible or contract-changing customer-web change. Public marketing fallback, browser-only account presentation, and server-only persistence/observability can remain web-specific. Native secure storage, notifications, local diagnostics, background operation, and OS integration may be native-specific. Account and organization state, API-key lifecycle, session/activity semantics, preferences, permissions, errors, notifications, and navigation normally propagate or require an explicit rationale and parity issue.

Each issue and pull request records affected surfaces, omitted surfaces and rationale, accepted parity gaps, follow-up work, and separate platform/release status.

## Deep links

Canonical:

```text
https://<verified-fiducia-owned-host>/open/<route>?<bounded-query>
```

Fallback:

```text
fiducia://<route>?<bounded-query>
```

The exact HTTPS host must be verified before publication. All surfaces share versioned route types and golden fixtures and support cold start, already-running delivery, authentication resume, replay/expiry rejection, browser fallback, and explicit confirmation before API-key creation/rotation/revocation, session revocation, organization changes, or other security-sensitive actions.

Never put API-key plaintext, verifier hashes, session tokens, cookies, organization secrets, credentials, private activity records, bearer/refresh tokens, or database identifiers in URLs. Use bounded identifiers or short-lived, single-use, audience-bound codes and validate route version, user/org/key/session IDs, action, authorization, assurance level, limits, and user intent.

## Review checklist

- [ ] Flutter Android/iOS impact evaluated.
- [ ] Flutter Web/mobile-web impact evaluated.
- [ ] Flutter desktop impact evaluated.
- [ ] GPUI Rust desktop impact evaluated.
- [ ] Shared customer/auth/client/route/fixture impact evaluated.
- [ ] Deep-link and auth-resume compatibility tested where relevant.
- [ ] Customer/admin data-plane separation remains intact.
- [ ] Omitted surfaces have a rationale and follow-up when needed.

## Routing

- GitHub Project: [`fiducia-cloud-project` — Project 1](https://github.com/orgs/fiducia-cloud/projects/1)
- Linear project: [`github.com/fiducia-cloud`](https://linear.app/denman/project/githubcomfiducia-cloud-8fd5e1bec9d3)
- Central policy: [`cross-surface-delivery.md`](https://github.com/ORESoftware/project-registry/blob/main/docs/cross-surface-delivery.md)
- Desktop registry: [`desktop-applications.json`](https://github.com/ORESoftware/project-registry/blob/main/registry/desktop-applications.json)
