# Cross-surface delivery

Verified **2026-08-06**.

## Product surfaces

| Surface | Repository | Current status |
|---|---|---|
| Rust web | `3FA-app/3fa-web-server.rs` | Current repository |
| Flutter mobile, mobile web, and desktop | current `ORESoftware/3fa-client-ui.dart`; target `3FA-app/3fa-flutter` | Current cross-owner implementation; planned migration target |
| Rust desktop | `3FA-app/3FA-desktop.rs` | Live native Slint application |
| Shared contracts | 3FA interfaces/clients repositories | Evaluate for every contract or route change |

The current Flutter repository remains in scope until a history-preserving migration and release cutover to `3FA-app/3fa-flutter` is verified.

## Delivery decision

For every user-visible or contract-changing web-server change, record the impact on:

1. Android and iOS Flutter;
2. Flutter Web/mobile web;
3. Flutter desktop;
4. Rust desktop; and
5. shared interfaces, generated clients, routes, and fixtures.

The surfaces do not need identical pages. Web-only SEO, public documentation, and server operations may remain web-only. Authentication semantics, enrollment and recovery flows, TOTP/HOTP behavior, device or vault state, permissions, errors, notifications, and navigation normally require coordinated updates or an explicit no-change rationale.

## Deep links

Canonical route family:

```text
https://<verified-3fa-owned-host>/open/<route>?<bounded-query>
```

Fallback:

```text
threefa://<route>?<bounded-query>
```

The host must be verified before publication. Route types and golden fixtures belong in the shared interfaces layer. Web, Android/iOS, Flutter Web, Flutter desktop, and Rust desktop must agree on route versioning, identifiers, actions, validation, authentication resume, replay/expiry behavior, and browser fallback.

Never put passwords, TOTP seeds, recovery secrets, vault contents, bearer or refresh tokens, credentials, or private account data in URLs. Use short-lived, single-use, audience-bound codes for enrollment, authentication, approval, recovery, or cross-app handoffs. Sensitive or destructive actions require explicit user confirmation.

## Pull-request checklist

- [ ] Flutter mobile impact evaluated.
- [ ] Flutter Web/mobile-web impact evaluated.
- [ ] Flutter desktop impact evaluated.
- [ ] Rust desktop impact evaluated.
- [ ] Shared interface/client/route/fixture impact evaluated.
- [ ] Deep-link compatibility tested when routes or navigation change.
- [ ] Every intentionally unchanged surface has a rationale and follow-up if needed.
- [ ] Platform and release status is reported separately.

## Routing

- GitHub Project: [`3FA-app-project` — Project 1](https://github.com/orgs/3FA-app/projects/1)
- Linear project: [`github.com/3FA-app`](https://linear.app/denman/project/githubcom3fa-app-c3db52220894)
- Central policy: [`cross-surface-delivery.md`](https://github.com/ORESoftware/project-registry/blob/main/docs/cross-surface-delivery.md)
- Desktop allocation: [`desktop-applications.json`](https://github.com/ORESoftware/project-registry/blob/main/registry/desktop-applications.json)
