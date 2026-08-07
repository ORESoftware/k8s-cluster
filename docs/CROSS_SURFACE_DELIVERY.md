# Cross-surface delivery

Verified **2026-08-06**.

## Surfaces

- Rust web/store application: `athlet-o/athleto-app-rs`
- Flutter Android/iOS, Flutter Web, and Flutter desktop: `athlet-o/athleto-flutter` — proposed/planned
- Rust desktop: `athlet-o/athleto-desktop.rs` — proposed/planned coach/facility/team/analytics app
- Shared contracts: Athlet-O interfaces, generated clients, catalog/product/cart/order/payment/fulfillment schemas, routes, and conformance fixtures

Repository names are allocation targets until their remotes and builds are verified.

## Judgment-based propagation

Evaluate mobile, Flutter Web, Flutter desktop, Rust desktop, and shared contracts for every user-visible or contract-changing web change. Storefront SEO, browser-only hosted checkout, and provider-return pages may remain web-specific. Native coach/facility analytics, local device imports, secure storage, notifications, and offline workflows may be native-specific. Catalog/product semantics, cart and order state, account/B2B approval, payment status, fulfillment, permissions, errors, notifications, and navigation normally propagate or require an explicit rationale and parity issue.

## Deep links

```text
https://<verified-athlet-o-owned-host>/open/<route>?<bounded-query>
```

The host must be verified. A custom-scheme fallback requires a reviewed ADR and must not be guessed. All surfaces share versioned route types and fixtures and support cold start, already-running delivery, authentication resume, replay/expiry rejection, browser fallback, and explicit confirmation before checkout, payment, reorder, approval, or destructive actions.

Never put payment credentials, provider tokens, API keys, cart/session cookies, private order data, health/fitness data, or personally sensitive information in URLs. Use bounded identifiers or short-lived, single-use, audience-bound codes and validate product/order/payment/approval IDs, route version, action, authorization, limits, and user intent.

## Review checklist

- [ ] Flutter Android/iOS impact evaluated.
- [ ] Flutter Web/mobile-web impact evaluated.
- [ ] Flutter desktop impact evaluated.
- [ ] Rust desktop impact evaluated.
- [ ] Shared commerce/client/route/fixture impact evaluated.
- [ ] Deep-link and hosted-payment return compatibility tested where relevant.
- [ ] Omitted surfaces have a rationale and follow-up when needed.

## Routing

- GitHub Project: [`athlet-o-project` — Project 1](https://github.com/orgs/athlet-o/projects/1)
- Linear project: [`github.com/athlet-o`](https://linear.app/denman/project/githubcomathlet-o-b5a995fed9bb)
- Central policy: [`cross-surface-delivery.md`](https://github.com/ORESoftware/project-registry/blob/main/docs/cross-surface-delivery.md)
- Desktop registry: [`desktop-applications.json`](https://github.com/ORESoftware/project-registry/blob/main/registry/desktop-applications.json)
